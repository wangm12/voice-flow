#[cfg(test)]
use crate::queue::QuotaView;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const SETTINGS_SCHEMA_VERSION: u32 = 27;
const HISTORY_SCHEMA_VERSION: i32 = 19;
// Migration rollback copies contain historical text, so they have a finite
// lifetime even when the user elects to keep the live history forever.
const HISTORY_MIGRATION_BACKUP_MAX_DAYS: u64 = 7;
const LEARN_PAIRS_PENDING_CAP: i64 = 256;
const PENDING_HISTORY_JOB_CAP: usize = 16;
const PENDING_HISTORY_BYTE_CAP: usize = 40 * 1024 * 1024;
const PENDING_HISTORY_JOB_MAX_BYTES: usize = 4 * 1024 * 1024;
const DICTIONARY_SOURCES_VERSION: u8 = 1;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub(crate) enum DictionarySource {
    Manual,
    LegacyUnknown,
    Rule(String),
    Screen(String),
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct DictionarySources {
    version: u8,
    owners: std::collections::BTreeMap<String, std::collections::BTreeSet<DictionarySource>>,
}

impl Default for DictionarySources {
    fn default() -> Self {
        Self {
            version: DICTIONARY_SOURCES_VERSION,
            owners: std::collections::BTreeMap::new(),
        }
    }
}

fn legacy_dictionary_sources() -> DictionarySources {
    DictionarySources {
        version: 0,
        owners: std::collections::BTreeMap::new(),
    }
}

pub(crate) fn dictionary_words_equal(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

pub(crate) fn dictionary_contains_word(dictionary: &[String], word: &str) -> bool {
    dictionary
        .iter()
        .any(|candidate| dictionary_words_equal(candidate, word))
}

fn truncate_dictionary_word(word: &str) -> String {
    let mut end = word.len().min(256);
    while !word.is_char_boundary(end) {
        end -= 1;
    }
    word[..end].to_owned()
}

fn normalized_dictionary_sources(dictionary: &mut Vec<String>, sources: &mut DictionarySources) {
    let migrate_legacy = sources.version < DICTIONARY_SOURCES_VERSION;
    let old_dictionary = std::mem::take(dictionary);
    let old_owners = std::mem::take(&mut sources.owners);
    let mut canonical_words = Vec::with_capacity(256);
    for word in old_dictionary
        .iter()
        .map(|word| truncate_dictionary_word(word))
        .chain(old_owners.keys().map(|word| truncate_dictionary_word(word)))
    {
        if canonical_words.len() == 256 {
            break;
        }
        if !canonical_words
            .iter()
            .any(|known: &String| dictionary_words_equal(known, &word))
        {
            canonical_words.push(word);
        }
    }

    let mut owners =
        std::collections::BTreeMap::<String, std::collections::BTreeSet<DictionarySource>>::new();
    for (word, word_sources) in old_owners {
        let truncated = truncate_dictionary_word(&word);
        let Some(canonical) = canonical_words
            .iter()
            .find(|known| dictionary_words_equal(known, &truncated))
        else {
            continue;
        };
        owners
            .entry(canonical.clone())
            .or_default()
            .extend(word_sources);
    }
    for word in old_dictionary {
        let truncated = truncate_dictionary_word(&word);
        let Some(canonical) = canonical_words
            .iter()
            .find(|known| dictionary_words_equal(known, &truncated))
        else {
            continue;
        };
        let has_owner = owners
            .get(canonical)
            .is_some_and(|word_sources| !word_sources.is_empty());
        if migrate_legacy || !has_owner {
            owners
                .entry(canonical.clone())
                .or_default()
                .insert(DictionarySource::LegacyUnknown);
        }
    }
    owners.retain(|_, word_sources| !word_sources.is_empty());
    for word in &canonical_words {
        if owners.contains_key(word) {
            dictionary.push(word.clone());
        }
    }
    sources.owners = owners;
    sources.version = DICTIONARY_SOURCES_VERSION;
}

fn dictionary_sources_need_normalization(settings: &Settings) -> bool {
    let mut dictionary = settings.dictionary.clone();
    let mut sources = settings.dictionary_sources.clone();
    normalized_dictionary_sources(&mut dictionary, &mut sources);
    dictionary != settings.dictionary || sources != settings.dictionary_sources
}

#[derive(Default)]
struct ActiveAudioSpoolSessions {
    sessions: std::collections::HashMap<PathBuf, SpoolReservation>,
    root_mutations: std::collections::HashMap<
        PathBuf,
        std::collections::HashMap<u64, std::sync::Arc<AtomicBool>>,
    >,
    clearing_roots: std::collections::HashSet<PathBuf>,
    next_generation: u64,
}

struct SpoolReservation {
    generation: u64,
    delete_when_released: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpoolLeaseKind {
    Session,
    Artifact,
}

static ACTIVE_AUDIO_SPOOL_SESSIONS: OnceLock<Mutex<ActiveAudioSpoolSessions>> = OnceLock::new();

#[cfg(test)]
pub(crate) fn hold_root_admission_lock_for_test() -> (
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
static PENDING_HISTORY_BUDGET_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static PENDING_HISTORY_ACTIVE: OnceLock<Mutex<std::collections::HashSet<(PathBuf, String)>>> =
    OnceLock::new();

#[cfg(test)]
struct HistoryPromotionTestGate {
    data_root: PathBuf,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
struct HistoryDeleteSpoolWaitTestGate {
    session_dir: PathBuf,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
static HISTORY_PROMOTION_TEST_GATE: OnceLock<Mutex<Option<HistoryPromotionTestGate>>> =
    OnceLock::new();

#[cfg(test)]
static HISTORY_DELETE_SPOOL_WAIT_TEST_GATE: OnceLock<
    Mutex<Option<HistoryDeleteSpoolWaitTestGate>>,
> = OnceLock::new();

#[cfg(test)]
struct PendingHistoryWriteTestGate {
    data_root: PathBuf,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
struct SpoolQuotaWriteTestGate {
    session_dir: PathBuf,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
struct SpoolSizeWalkTestGate {
    path: PathBuf,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
#[derive(Clone, Copy)]
enum SpoolQuotaScanFault {
    Error,
    Panic,
}

#[cfg(test)]
struct SpoolQuotaScanFaultInjection {
    path: PathBuf,
    fault: SpoolQuotaScanFault,
}

#[cfg(test)]
struct AtomicRenameTestGate {
    temp_path: PathBuf,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
static PENDING_HISTORY_WRITE_TEST_GATE: OnceLock<Mutex<Option<PendingHistoryWriteTestGate>>> =
    OnceLock::new();

#[cfg(test)]
static SPOOL_QUOTA_WRITE_TEST_GATE: OnceLock<Mutex<Option<SpoolQuotaWriteTestGate>>> =
    OnceLock::new();

#[cfg(test)]
static SPOOL_SIZE_WALK_TEST_GATE: OnceLock<Mutex<Option<SpoolSizeWalkTestGate>>> = OnceLock::new();

#[cfg(test)]
static SPOOL_QUOTA_SCAN_FAULT: OnceLock<Mutex<Option<SpoolQuotaScanFaultInjection>>> =
    OnceLock::new();

#[cfg(test)]
static ATOMIC_RENAME_TEST_GATE: OnceLock<Mutex<Option<AtomicRenameTestGate>>> = OnceLock::new();

#[cfg(test)]
pub(crate) fn pause_next_history_promotion_for_test(
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
fn wait_for_history_promotion_test_gate(data_root: &Path) {
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
fn pause_history_delete_spool_wait_for_test(
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
fn wait_for_history_delete_spool_test_gate(session_dir: &Path) {
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
pub(crate) fn pause_next_pending_history_write_for_test(
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
pub(crate) fn pause_next_spool_write_after_quota_reservation_for_test(
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
fn pause_next_spool_size_walk_for_test(
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
fn inject_spool_quota_scan_fault_for_test(path: &Path, fault: SpoolQuotaScanFault) {
    *SPOOL_QUOTA_SCAN_FAULT
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(SpoolQuotaScanFaultInjection {
        path: normalized_data_root(path),
        fault,
    });
}

#[cfg(test)]
fn pause_next_atomic_rename_for_test(
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
fn wait_for_spool_write_after_quota_reservation_for_test(session_dir: &Path) {
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
fn wait_for_spool_size_walk_test_gate(path: &Path) {
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
fn take_spool_quota_scan_fault_for_test(path: &Path) -> Option<SpoolQuotaScanFault> {
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
fn wait_for_atomic_rename_test_gate(temp_path: &Path) {
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
fn wait_for_pending_history_write_test_gate(data_root: &Path) {
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
struct SpoolIoCommitState {
    generation: u64,
    roots: std::collections::HashMap<PathBuf, u64>,
    scopes: std::collections::HashMap<(PathBuf, SpoolIoScope), u64>,
}

static SPOOL_IO_COMMIT_STATE: OnceLock<Mutex<SpoolIoCommitState>> = OnceLock::new();
// `active_scans` fences same-root admissions while recursive filesystem
// counts run without holding this mutex. The start-of-scan reservation total
// remains charged through admission, even if a completed write releases its
// in-flight reservation during the walk.
#[derive(Default)]
struct SpoolQuotaLedger {
    reservations: std::collections::HashMap<PathBuf, u64>,
    active_scans: std::collections::HashSet<PathBuf>,
}

static SPOOL_QUOTA_RESERVATIONS: OnceLock<(Mutex<SpoolQuotaLedger>, std::sync::Condvar)> =
    OnceLock::new();

fn spool_quota_ledger() -> &'static (Mutex<SpoolQuotaLedger>, std::sync::Condvar) {
    SPOOL_QUOTA_RESERVATIONS.get_or_init(|| {
        (
            Mutex::new(SpoolQuotaLedger::default()),
            std::sync::Condvar::new(),
        )
    })
}

#[cfg(test)]
struct AtomicParentSyncFailureTest {
    parent: PathBuf,
}

#[cfg(test)]
static ATOMIC_PARENT_SYNC_FAILURE_TEST: OnceLock<Mutex<Option<AtomicParentSyncFailureTest>>> =
    OnceLock::new();

pub(crate) struct SpoolIoCommitPermit {
    region: SpoolIoCommitRegion,
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SpoolIoScope {
    RecoverySpool,
    GoldAudio,
    HistoryDatabase,
    PendingHistoryIntent,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum SpoolIoCommitRegion {
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

pub(crate) fn normalized_data_root(path: &Path) -> PathBuf {
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

fn session_data_root(session_dir: &Path) -> anyhow::Result<PathBuf> {
    session_dir
        .parent()
        .and_then(Path::parent)
        .map(normalized_data_root)
        .ok_or_else(|| anyhow::anyhow!("invalid spool session path"))
}

fn try_spool_io_commit(data_root: &Path) -> anyhow::Result<SpoolIoCommitPermit> {
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

fn spool_io_scope_root(data_root: &Path, scope: SpoolIoScope) -> PathBuf {
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

pub(crate) fn try_spool_io_commit_scope(
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
fn try_history_database_commit_with_wait(data_root: &Path) -> anyhow::Result<SpoolIoCommitPermit> {
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
fn try_spool_io_commit_audio_session(
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
fn try_spool_io_commit_audio_session_cleanup(
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

fn try_spool_io_commit_scope_at(
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

struct SpoolQuotaReservation {
    root: PathBuf,
    bytes: u64,
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

struct SpoolQuotaScanReservation {
    root: PathBuf,
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

fn reserve_recovery_spool_bytes(root: &Path, bytes: u64) -> anyhow::Result<SpoolQuotaReservation> {
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
pub(crate) fn fail_next_atomic_parent_sync_for_test(parent: &Path) {
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

fn try_spool_io_commit_pending_history(
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

fn acquire_pending_history_mutation_and_commit(
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
pub(crate) struct PendingHistoryActiveLease {
    inner: std::sync::Arc<PendingHistoryActiveLeaseInner>,
}

struct PendingHistoryActiveLeaseInner {
    key: (PathBuf, String),
    cancelled: std::sync::atomic::AtomicBool,
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

    fn ensure_writable(&self, data_root: &Path, transaction_id: &str) -> anyhow::Result<()> {
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

    fn poison_for_known_no_effect_cancel(
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

fn new_pending_history_active_lease(key: (PathBuf, String)) -> PendingHistoryActiveLease {
    PendingHistoryActiveLease {
        inner: std::sync::Arc::new(PendingHistoryActiveLeaseInner {
            key,
            cancelled: std::sync::atomic::AtomicBool::new(false),
        }),
    }
}

pub(crate) fn register_pending_history_owner(
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

fn reserve_pending_history_startup_owner(
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

pub(crate) struct AudioSpoolSessionLease {
    inner: std::sync::Arc<AudioSpoolSessionLeaseInner>,
}

struct AudioSpoolSessionLeaseInner {
    session_dir: PathBuf,
    generation: u64,
    needs_initialization: bool,
    kind: SpoolLeaseKind,
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

    fn generation(&self) -> u64 {
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
pub(crate) struct SpoolRootMutationLease {
    inner: std::sync::Arc<SpoolRootMutationLeaseInner>,
}

struct SpoolRootMutationLeaseInner {
    data_root: PathBuf,
    active: std::sync::Arc<AtomicBool>,
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

fn prune_dead_root_mutations(active: &mut ActiveAudioSpoolSessions) {
    active.root_mutations.retain(|_, generations| {
        generations.retain(|_, is_active| is_active.load(Ordering::Acquire));
        !generations.is_empty()
    });
}

pub(crate) fn begin_spool_root_mutation(
    data_root: &Path,
) -> anyhow::Result<SpoolRootMutationLease> {
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

pub(crate) fn try_begin_spool_root_mutation(
    data_root: &Path,
) -> anyhow::Result<SpoolRootMutationLease> {
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

fn reserve_spool_session_path(
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

fn valid_spool_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id.len() <= 128
        && session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn root_is_clearing(active: &ActiveAudioSpoolSessions, data_root: &Path) -> bool {
    let data_root = normalized_data_root(data_root);
    active
        .clearing_roots
        .iter()
        .any(|clearing_root| roots_overlap(&data_root, clearing_root))
}

fn roots_overlap(left: &Path, right: &Path) -> bool {
    let left = normalized_data_root(left);
    let right = normalized_data_root(right);
    left.starts_with(&right) || right.starts_with(&left)
}

pub(crate) fn lease_spool_session(session_dir: &Path) -> anyhow::Result<AudioSpoolSessionLease> {
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
pub(crate) fn initialize_audio_spool_session(lease: &AudioSpoolSessionLease) -> anyhow::Result<()> {
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

fn spool_session_is_leased(session_dir: &Path) -> bool {
    ACTIVE_AUDIO_SPOOL_SESSIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .sessions
        .contains_key(session_dir)
}

fn spool_lease_is_current(lease: &AudioSpoolSessionLease) -> bool {
    ACTIVE_AUDIO_SPOOL_SESSIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .sessions
        .get(lease.session_dir())
        .is_some_and(|reservation| reservation.generation == lease.generation())
}

fn spool_lease_can_write(lease: &AudioSpoolSessionLease) -> bool {
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

fn spool_artifact_is_leased(path: &Path) -> bool {
    let path = normalized_data_root(path);
    ACTIVE_AUDIO_SPOOL_SESSIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .sessions
        .keys()
        .any(|session_dir| path.starts_with(session_dir))
}

fn has_spool_owner_under(path: &Path) -> bool {
    let path = normalized_data_root(path);
    ACTIVE_AUDIO_SPOOL_SESSIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .sessions
        .keys()
        .any(|session_dir| session_dir.starts_with(&path))
}

fn defer_spool_artifact_removal_until_release(path: &Path) -> bool {
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

pub(crate) struct SpoolClearGuard {
    data_root: PathBuf,
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

fn begin_spool_clear(data_root: &Path) -> anyhow::Result<SpoolClearGuard> {
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
pub(crate) fn begin_clear_all_data(data_root: &Path) -> anyhow::Result<SpoolClearGuard> {
    begin_spool_clear(data_root)
}

fn ensure_private_dir(path: &Path) -> anyhow::Result<()> {
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

fn sync_directory(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn sync_atomic_parent_directory(path: &Path) -> anyhow::Result<()> {
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

fn restrict_file_mode(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn restrict_history_sidecars(dir: &Path) -> anyhow::Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let path = dir.join(format!("history.sqlite{suffix}"));
        if path.exists() {
            restrict_file_mode(&path)?;
        }
    }
    Ok(())
}

fn current_settings_schema_version() -> u32 {
    SETTINGS_SCHEMA_VERSION
}

/// Wire value and model ID of the cleanup route retired in schema 27.
const RETIRED_LMSTUDIO_CLEANUP_PROVIDER: &str = "lmstudio";
const RETIRED_LMSTUDIO_CLEANUP_MODEL: &str = "qwen/qwen3.8-27b@6bit";

/// A saved LM Studio selection reads as the bundled MLX route, which runs the
/// same Qwen 3.8 27B MLX 6-bit model locally; `normalize` repairs its model ID.
fn deserialize_cleanup_provider<'de, D>(
    deserializer: D,
) -> Result<crate::engine::EngineProvider, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value == RETIRED_LMSTUDIO_CLEANUP_PROVIDER {
        return Ok(crate::engine::EngineProvider::OnDeviceMlx);
    }
    crate::engine::EngineProvider::deserialize(
        serde::de::value::StringDeserializer::<D::Error>::new(value),
    )
}

fn default_writing_modes() -> Vec<crate::context::WritingMode> {
    crate::context::builtin_writing_modes()
}

fn default_theme() -> String {
    "system".into()
}

fn default_ui_language() -> String {
    "system".into()
}

/// Teaching acknowledgement is not feature readiness or a permission grant.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OnDemandGuideId {
    DictionaryLearning,
    ContextScreen,
    SelectedTranslation,
    ScenePreferences,
}
impl OnDemandGuideId {
    const ALL: [Self; 4] = [
        Self::DictionaryLearning,
        Self::ContextScreen,
        Self::SelectedTranslation,
        Self::ScenePreferences,
    ];
    fn current_version(self) -> u32 {
        match self {
            Self::DictionaryLearning => 1,
            Self::ContextScreen => 1,
            Self::SelectedTranslation => 1,
            Self::ScenePreferences => 1,
        }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GuideAcknowledgements {
    dictionary_learning: Option<u32>,
    context_screen: Option<u32>,
    selected_translation: Option<u32>,
    scene_preferences: Option<u32>,
}
#[derive(Debug, Serialize)]
pub(crate) struct GuideAcknowledgementView {
    guide_id: OnDemandGuideId,
    current_version: u32,
    acknowledged_version: Option<u32>,
}
impl GuideAcknowledgements {
    fn slot(&mut self, id: OnDemandGuideId) -> &mut Option<u32> {
        match id {
            OnDemandGuideId::DictionaryLearning => &mut self.dictionary_learning,
            OnDemandGuideId::ContextScreen => &mut self.context_screen,
            OnDemandGuideId::SelectedTranslation => &mut self.selected_translation,
            OnDemandGuideId::ScenePreferences => &mut self.scene_preferences,
        }
    }
    pub(crate) fn acknowledge(
        &mut self,
        id: OnDemandGuideId,
        version: u32,
    ) -> Result<bool, &'static str> {
        if version != id.current_version() {
            return Err("guide_version_changed");
        }
        let slot = self.slot(id);
        let changed = *slot != Some(version);
        *slot = Some(version);
        Ok(changed)
    }
    pub(crate) fn view(&self) -> Vec<GuideAcknowledgementView> {
        let mut values = self.clone();
        OnDemandGuideId::ALL
            .into_iter()
            .map(|id| GuideAcknowledgementView {
                guide_id: id,
                current_version: id.current_version(),
                acknowledged_version: *values.slot(id),
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
/// Non-secret outcome of the last credential-store resolution. This is not a
/// provider connectivity or credential-validity result.
pub enum CredentialReadState {
    Configured,
    Missing,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomCredentialPurpose {
    Asr,
    Cleanup,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Written only by the explicit acknowledge_guide command.
    pub guide_acknowledgements: GuideAcknowledgements,
    #[serde(default = "current_settings_schema_version")]
    pub schema_version: u32,
    pub api_key: String,
    #[serde(default)]
    pub asr_api_key: String,
    #[serde(default)]
    pub asr_base_url: String,
    #[serde(default)]
    pub asr_provider: crate::engine::EngineProvider,
    #[serde(default = "default_asr_model")]
    pub asr_model: String,
    pub language: String,
    #[serde(default = "default_ui_language")]
    pub ui_language: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    pub dictionary: Vec<String>,
    #[serde(default = "legacy_dictionary_sources")]
    pub(crate) dictionary_sources: DictionarySources,
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    #[serde(default = "default_legacy_activation_mode")]
    pub activation_mode: String,
    pub chunk_threshold_secs: u64,
    pub chunk_length_secs: usize,
    #[serde(default = "default_long_output_mode")]
    pub long_output_mode: String,
    #[serde(default = "default_delivery_policy")]
    pub delivery_policy: String,
    pub keep_audio_days: u64,
    pub keep_history_days: u64,
    #[serde(default)]
    pub keep_success_audio: bool,
    pub onboarded: bool,
    pub cleanup_enabled: bool,
    #[serde(default = "default_cleanup_intensity")]
    pub cleanup_intensity: String,
    #[serde(default)]
    pub accurate_asr_provider: crate::engine::EngineProvider,
    #[serde(default)]
    pub accurate_asr_model: String,
    #[serde(default)]
    pub accurate_asr_base_url: String,
    #[serde(default = "default_cascade_timeout_ms")]
    pub cascade_timeout_ms: u64,
    #[serde(default = "default_cascade_proper_noun_threshold")]
    pub cascade_proper_noun_threshold: usize,
    #[serde(default)]
    pub window_ocr_enabled: bool,
    #[serde(default)]
    pub screen_action_hotkey: String,
    #[serde(default)]
    pub vision_provider: String,
    #[serde(default)]
    pub vision_model: String,
    #[serde(default = "default_cleanup_model")]
    pub cleanup_model: String,
    #[serde(default, deserialize_with = "deserialize_cleanup_provider")]
    pub cleanup_provider: crate::engine::EngineProvider,
    #[serde(default)]
    pub cleanup_base_url: String,
    #[serde(default)]
    pub cleanup_api_key: String,
    #[serde(default)]
    pub custom_base_url: String,
    #[serde(default = "default_true")]
    pub custom_asr: bool,
    #[serde(default = "default_true")]
    pub custom_llm: bool,
    #[serde(default)]
    pub custom_cleanup_parameters: crate::custom_cleanup_parameters::Parameters,
    #[serde(default)]
    pub ollama_base_url: String,
    #[serde(default)]
    pub strict_offline_enabled: bool,
    #[serde(default)]
    pub local_whisper_base_url: String,
    #[serde(default)]
    pub provider_api_keys: std::collections::BTreeMap<String, String>,
    #[serde(default = "default_show_tray_icon")]
    pub show_tray_icon: bool,
    pub context_enabled: bool,
    pub browser_access_enabled: bool,
    pub context_mappings: Vec<crate::context::AppMapping>,
    #[serde(default = "default_writing_modes")]
    pub writing_modes: Vec<crate::context::WritingMode>,
    #[serde(default)]
    pub snippets: Vec<crate::snippets::Snippet>,
    #[serde(default = "default_output_mode")]
    pub output_mode: String,
    #[serde(default = "default_translation_target_language")]
    pub translation_target_language: String,
    #[serde(default = "default_selected_action_hotkey")]
    pub selected_action_hotkey: String,
    #[serde(default = "default_selected_actions_enabled")]
    pub selected_actions_enabled: bool,
    #[serde(default = "default_dictionary_learn_enabled")]
    pub dictionary_learn_enabled: bool,
    /// Separate local-only opt-in for watching edits in the same verified
    /// field immediately after VoiceFlow inserts text. This never grants
    /// screen/context capture permissions.
    #[serde(default)]
    pub local_edit_learning_enabled: bool,
    #[serde(default)]
    pub input_device: String,
    #[serde(default = "default_input_gain")]
    pub input_gain: f32,
    #[serde(default)]
    pub verbatim_hotkey: String,
    #[serde(default)]
    pub translation_hotkey: String,
    #[serde(default = "default_extra_recording_buffer_ms")]
    pub extra_recording_buffer_ms: u64,
    #[serde(default)]
    pub audio_feedback_enabled: bool,
    #[serde(default = "default_audio_feedback_volume")]
    pub audio_feedback_volume: f32,
    #[serde(default)]
    pub vad_enabled: bool,
    #[serde(default)]
    pub always_on_microphone: bool,
    #[serde(default)]
    pub clamshell_microphone: String,
    #[serde(default)]
    pub autostart_enabled: bool,
    #[serde(default = "default_whats_new_last_seen_version")]
    pub whats_new_last_seen_version: String,
    #[serde(default)]
    pub debug_mode: bool,
    #[serde(default)]
    pub fuzzy_dictionary_enabled: bool,
    /// Legacy credential values that could not yet be verified in the OS
    /// credential store. These stay in memory only to preserve compatibility;
    /// save_settings keeps their source data intact until migration succeeds.
    #[serde(skip)]
    pub(crate) unverified_credential_sources: std::collections::BTreeMap<String, String>,
    /// Secure values observed while loading, used to distinguish ordinary
    /// settings saves from an explicit key replacement.
    #[serde(skip)]
    pub(crate) credential_baselines: std::collections::BTreeMap<String, String>,
    /// Non-secret result of the last secure-store resolution for each fixed
    /// account. This remains process-only and is intentionally not persisted.
    #[serde(skip)]
    pub(crate) credential_read_states: std::collections::BTreeMap<String, CredentialReadState>,
    /// A foreground reread is session-only. Keep it out of later implicit
    /// credential fan-out/persistence until the user explicitly saves a key.
    #[serde(skip)]
    pub(crate) session_only_custom_credential: bool,
    /// The active Custom origin and purposes for which a foreground-only
    /// credential was recovered. This is process-only routing metadata.
    #[serde(skip)]
    pub(crate) session_custom_credential_scope: Option<CustomCredentialRecoveryScope>,
    /// Prevents a revoked foreground-only value from falling through to an
    /// unrelated legacy alias at a later Custom route.
    #[serde(skip)]
    pub(crate) session_custom_credential_rejected: bool,
}

fn default_cleanup_intensity() -> String {
    "auto".into()
}

fn default_cascade_timeout_ms() -> u64 {
    5000
}

fn default_cascade_proper_noun_threshold() -> usize {
    3
}

fn default_cleanup_model() -> String {
    crate::llm::MODEL.to_owned()
}

fn default_show_tray_icon() -> bool {
    true
}

fn default_output_mode() -> String {
    "auto".into()
}

fn default_translation_target_language() -> String {
    "en".into()
}

fn default_long_output_mode() -> String {
    "paste".into()
}

fn default_delivery_policy() -> String {
    crate::delivery::DeliveryPolicy::default_value()
        .as_str()
        .into()
}

fn default_hotkey() -> String {
    "CmdOrControl+Alt+Space".into()
}

fn default_selected_action_hotkey() -> String {
    "CmdOrControl+Alt+Slash".into()
}

fn default_selected_actions_enabled() -> bool {
    true
}

fn default_dictionary_learn_enabled() -> bool {
    true
}

fn default_legacy_activation_mode() -> String {
    "tap".into()
}

fn default_extra_recording_buffer_ms() -> u64 {
    250
}

fn default_audio_feedback_volume() -> f32 {
    0.6
}

fn default_whats_new_last_seen_version() -> String {
    env!("CARGO_PKG_VERSION").into()
}

fn default_input_gain() -> f32 {
    1.0
}

fn default_asr_model() -> String {
    crate::asr::MODEL.to_owned()
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            guide_acknowledgements: GuideAcknowledgements::default(),
            schema_version: SETTINGS_SCHEMA_VERSION,
            api_key: String::new(),
            asr_api_key: String::new(),
            asr_base_url: String::new(),
            asr_provider: crate::engine::EngineProvider::Groq,
            asr_model: default_asr_model(),
            language: "auto".into(),
            ui_language: default_ui_language(),
            theme: default_theme(),
            dictionary: Vec::new(),
            dictionary_sources: DictionarySources::default(),
            hotkey: default_hotkey(),
            activation_mode: "tap".into(),
            chunk_threshold_secs: 25,
            chunk_length_secs: 35,
            long_output_mode: default_long_output_mode(),
            delivery_policy: default_delivery_policy(),
            keep_audio_days: 7,
            keep_history_days: 365,
            keep_success_audio: false,
            onboarded: false,
            cleanup_enabled: true,
            cleanup_intensity: default_cleanup_intensity(),
            accurate_asr_provider: crate::engine::EngineProvider::Groq,
            accurate_asr_model: String::new(),
            accurate_asr_base_url: String::new(),
            cascade_timeout_ms: default_cascade_timeout_ms(),
            cascade_proper_noun_threshold: default_cascade_proper_noun_threshold(),
            window_ocr_enabled: false,
            screen_action_hotkey: String::new(),
            vision_provider: String::new(),
            vision_model: String::new(),
            cleanup_model: default_cleanup_model(),
            cleanup_provider: crate::engine::EngineProvider::Groq,
            cleanup_base_url: String::new(),
            cleanup_api_key: String::new(),
            custom_base_url: String::new(),
            custom_asr: true,
            custom_llm: true,
            custom_cleanup_parameters: Default::default(),
            ollama_base_url: crate::providers::EngineProvider::Ollama
                .default_base_url()
                .to_owned(),
            strict_offline_enabled: false,
            local_whisper_base_url: crate::providers::EngineProvider::LocalWhisper
                .default_base_url()
                .to_owned(),
            provider_api_keys: std::collections::BTreeMap::new(),
            show_tray_icon: true,
            context_enabled: true,
            browser_access_enabled: false,
            context_mappings: Vec::new(),
            writing_modes: default_writing_modes(),
            snippets: Vec::new(),
            output_mode: default_output_mode(),
            translation_target_language: default_translation_target_language(),
            selected_action_hotkey: default_selected_action_hotkey(),
            selected_actions_enabled: default_selected_actions_enabled(),
            dictionary_learn_enabled: default_dictionary_learn_enabled(),
            local_edit_learning_enabled: false,
            input_device: String::new(),
            input_gain: default_input_gain(),
            verbatim_hotkey: String::new(),
            translation_hotkey: String::new(),
            extra_recording_buffer_ms: 250,
            audio_feedback_enabled: false,
            audio_feedback_volume: 0.6,
            vad_enabled: false,
            always_on_microphone: false,
            clamshell_microphone: String::new(),
            autostart_enabled: false,
            whats_new_last_seen_version: default_whats_new_last_seen_version(),
            debug_mode: false,
            fuzzy_dictionary_enabled: false,
            unverified_credential_sources: std::collections::BTreeMap::new(),
            credential_baselines: std::collections::BTreeMap::new(),
            credential_read_states: std::collections::BTreeMap::new(),
            session_only_custom_credential: false,
            session_custom_credential_scope: None,
            session_custom_credential_rejected: false,
        }
    }
}

impl Settings {
    fn normalize_dictionary_sources(&mut self) {
        normalized_dictionary_sources(&mut self.dictionary, &mut self.dictionary_sources);
    }

    pub(crate) fn can_add_dictionary_source(&self, word: &str) -> bool {
        let word = word.trim();
        if word.is_empty() || word.len() > 256 {
            return false;
        }
        if dictionary_contains_word(&self.dictionary, word)
            || self
                .dictionary_sources
                .owners
                .keys()
                .any(|existing| dictionary_words_equal(existing, word))
        {
            return true;
        }
        let mut canonical_terms: Vec<&str> = Vec::new();
        for existing in self
            .dictionary
            .iter()
            .map(String::as_str)
            .chain(self.dictionary_sources.owners.keys().map(String::as_str))
        {
            if !canonical_terms
                .iter()
                .any(|known| dictionary_words_equal(known, existing))
            {
                canonical_terms.push(existing);
            }
        }
        canonical_terms.len() < 256
    }

    pub(crate) fn add_dictionary_source(&mut self, word: &str, source: DictionarySource) -> bool {
        let word = word.trim().to_owned();
        if word.is_empty() || word.len() > 256 {
            return false;
        }
        self.normalize_dictionary_sources();
        if !self.can_add_dictionary_source(&word) {
            return false;
        }
        let word = self
            .dictionary
            .iter()
            .find(|existing| dictionary_words_equal(existing, &word))
            .cloned()
            .or_else(|| {
                self.dictionary_sources
                    .owners
                    .keys()
                    .find(|existing| dictionary_words_equal(existing, &word))
                    .cloned()
            })
            .unwrap_or(word);
        let inserted = self
            .dictionary_sources
            .owners
            .entry(word.to_owned())
            .or_default()
            .insert(source);
        if inserted && !dictionary_contains_word(&self.dictionary, &word) {
            self.dictionary.push(word);
        }
        inserted
    }

    pub(crate) fn has_dictionary_source(&self, word: &str, source: &DictionarySource) -> bool {
        let word = word.trim();
        self.dictionary_sources
            .owners
            .iter()
            .find(|(existing, _)| dictionary_words_equal(existing, word))
            .map(|(_, owners)| owners)
            .is_some_and(|owners| owners.contains(source))
    }

    pub(crate) fn remove_dictionary_source(
        &mut self,
        word: &str,
        source: &DictionarySource,
    ) -> bool {
        self.normalize_dictionary_sources();
        let Some(canonical_word) = self
            .dictionary_sources
            .owners
            .keys()
            .find(|existing| dictionary_words_equal(existing, word))
            .cloned()
        else {
            return false;
        };
        let Some(owners) = self.dictionary_sources.owners.get_mut(&canonical_word) else {
            return false;
        };
        let removed = owners.remove(source);
        if owners.is_empty() {
            self.dictionary_sources.owners.remove(&canonical_word);
            self.dictionary.retain(|entry| entry != &canonical_word);
        }
        removed
    }

    pub(crate) fn remove_dictionary_word(&mut self, word: &str) -> bool {
        self.normalize_dictionary_sources();
        let canonical_word = self
            .dictionary_sources
            .owners
            .keys()
            .find(|existing| dictionary_words_equal(existing, word))
            .cloned();
        let removed = canonical_word
            .as_ref()
            .and_then(|canonical| self.dictionary_sources.owners.remove(canonical))
            .is_some();
        let before = self.dictionary.len();
        self.dictionary
            .retain(|entry| !dictionary_words_equal(entry, word));
        removed || self.dictionary.len() != before
    }

    pub(crate) fn reconcile_dictionary_patch(&mut self, requested: Vec<String>) -> Vec<String> {
        let old_terms = self
            .dictionary_sources
            .owners
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut deduplicated = Vec::with_capacity(requested.len().min(256));
        for word in requested {
            let word = truncate_dictionary_word(word.trim());
            if word.is_empty() {
                continue;
            }
            let canonical = old_terms
                .iter()
                .chain(deduplicated.iter())
                .find(|existing| dictionary_words_equal(existing, &word))
                .cloned()
                .unwrap_or(word);
            if !deduplicated
                .iter()
                .any(|existing| dictionary_words_equal(existing, &canonical))
            {
                if deduplicated.len() == 256 {
                    break;
                }
                deduplicated.push(canonical);
            }
        }
        let removed = old_terms
            .iter()
            .filter(|word| {
                !deduplicated
                    .iter()
                    .any(|requested| dictionary_words_equal(requested, word))
            })
            .cloned()
            .collect::<Vec<_>>();
        for word in &removed {
            self.dictionary_sources.owners.remove(word);
        }
        for word in &deduplicated {
            if !old_terms
                .iter()
                .any(|existing| dictionary_words_equal(existing, word))
            {
                self.dictionary_sources
                    .owners
                    .entry(word.clone())
                    .or_default()
                    .insert(DictionarySource::Manual);
            }
        }
        self.dictionary = deduplicated;
        self.normalize_dictionary_sources();
        removed
    }

    pub fn normalize(&mut self) {
        self.normalize_dictionary_sources();
        if self.schema_version < SETTINGS_SCHEMA_VERSION {
            // Before schema 7, a new install was silently stored as Chinese.
            // Preserve an explicit choice for onboarded users, but let an
            // unfinished setup follow the operating system language.
            if self.schema_version < 7 && !self.onboarded && self.ui_language == "zh" {
                self.ui_language = default_ui_language();
            }
            if self.schema_version < 9 && self.selected_action_hotkey.trim().is_empty() {
                self.selected_action_hotkey = default_selected_action_hotkey();
                self.selected_actions_enabled = true;
            }
            if self.schema_version < 11 && self.output_mode == "plain" {
                self.output_mode = default_output_mode();
            }
            // Schema 11 used clipboard as the long-recording default. Treat
            // that legacy default as a migration to the new default, while
            // preserving explicit choices made in the current schema.
            if self.schema_version < 12 && self.long_output_mode == "clipboard" {
                self.long_output_mode = default_long_output_mode();
            }
            if self.schema_version < 13 {
                let migrated_policy = match self.long_output_mode.as_str() {
                    "clipboard" => "clipboard_only",
                    "history" => "history_only",
                    _ => "auto",
                };
                self.delivery_policy = migrated_policy.into();
            }
            if self.schema_version < 14 {
                if self.hotkey == "CmdOrControl+Shift+Space" {
                    self.hotkey = default_hotkey();
                }
                if self.selected_action_hotkey == "CmdOrControl+Shift+Slash" {
                    self.selected_action_hotkey = default_selected_action_hotkey();
                }
            }
            if self.schema_version < 15
                && self.selected_action_hotkey == "CmdOrControl+Alt+Backslash"
            {
                self.selected_action_hotkey = default_selected_action_hotkey();
            }
            if self.schema_version < 16 {
                self.asr_provider = if self.asr_base_url.trim().is_empty()
                    || crate::asr::groq_key_fallback_allowed(&self.asr_base_url)
                {
                    crate::engine::EngineProvider::Groq
                } else {
                    crate::engine::EngineProvider::Custom
                };
                if self.asr_provider.is_groq() {
                    self.asr_base_url.clear();
                }
                self.cleanup_provider = crate::engine::EngineProvider::Groq;
                self.cleanup_base_url.clear();
                self.cleanup_api_key.clear();
            }
            if self.schema_version < 17 {
                self.migrate_provider_pool();
            }
            if self.schema_version < 21 && self.window_ocr_enabled {
                // Before per-App OCR grants existed, enabling the global OCR
                // switch together with an App mapping was the user's explicit
                // local OCR opt-in. Preserve that choice for those mappings;
                // no AX text, provider text, or cloud vision grant is inferred.
                for mapping in &mut self.context_mappings {
                    mapping.source_permissions.local_ocr = true;
                }
            }
            if self.schema_version < 27
                && self.cleanup_provider == crate::engine::EngineProvider::OnDeviceMlx
                && self.cleanup_model.trim() == RETIRED_LMSTUDIO_CLEANUP_MODEL
            {
                // Retired LM Studio selections, and older MLX selections that
                // stored the LM Studio variant, keep the same model locally.
                self.cleanup_model = crate::modelstore::MODEL_ID.to_owned();
            }
            self.schema_version = SETTINGS_SCHEMA_VERSION;
        }
        self.chunk_threshold_secs = self.chunk_threshold_secs.clamp(5, 3_600);
        self.chunk_length_secs = self.chunk_length_secs.clamp(15, 60);
        self.keep_audio_days = self.keep_audio_days.min(365);
        self.keep_history_days = self.keep_history_days.min(3650);
        if !matches!(self.language.as_str(), "auto" | "zh" | "en") {
            self.language = "auto".into();
        }
        if !matches!(self.ui_language.as_str(), "system" | "zh" | "en") {
            self.ui_language = default_ui_language();
        }
        if !matches!(self.theme.as_str(), "system" | "light" | "dark") {
            self.theme = default_theme();
        }
        if !matches!(
            self.long_output_mode.as_str(),
            "clipboard" | "paste" | "history"
        ) {
            self.long_output_mode = default_long_output_mode();
        }
        if !crate::delivery::DeliveryPolicy::is_valid(&self.delivery_policy) {
            self.delivery_policy = default_delivery_policy();
        }
        if self.cleanup_provider.is_groq() && !crate::llm::is_supported_model(&self.cleanup_model) {
            self.cleanup_model = default_cleanup_model();
        }
        if self.hotkey.is_empty() || self.hotkey.len() > 128 {
            self.hotkey = Self::default().hotkey;
        }
        self.hotkey = crate::hotkey::canonicalize_hotkey(&self.hotkey);
        self.selected_action_hotkey =
            crate::hotkey::canonicalize_hotkey(&self.selected_action_hotkey);
        self.screen_action_hotkey = crate::hotkey::canonicalize_hotkey(&self.screen_action_hotkey);
        // Legacy hold meant the hybrid gesture, not pure hold-to-talk. All
        // retired gestures migrate to tap; an explicit v25 hold choice survives.
        if !matches!(self.activation_mode.as_str(), "tap" | "hold_to_talk") {
            self.activation_mode = "tap".into();
        }

        self.dictionary.truncate(256);
        self.dictionary
            .iter_mut()
            .for_each(|word| word.truncate(256));
        self.context_mappings
            .retain(|mapping| mapping.validate().is_ok());
        for mapping in &mut self.context_mappings {
            if let Some(host) = &mapping.browser_host {
                mapping.browser_host = crate::context::normalize_host(host);
            }
            if let Some(path) = &mapping.browser_path_prefix {
                mapping.browser_path_prefix = crate::context::normalize_path_prefix(path);
            }
            mapping.style_example_input = mapping.style_example_input.take().and_then(|value| {
                let trimmed: String = value.trim().chars().take(2_000).collect();
                (!trimmed.is_empty()).then_some(trimmed)
            });
            mapping.style_example_output = mapping.style_example_output.take().and_then(|value| {
                let trimmed: String = value.trim().chars().take(2_000).collect();
                (!trimmed.is_empty()).then_some(trimmed)
            });
            if mapping.style_example_pairs.is_empty() {
                if let (Some(input), Some(output)) = (
                    mapping.style_example_input.clone(),
                    mapping.style_example_output.clone(),
                ) {
                    mapping
                        .style_example_pairs
                        .push(crate::context::StyleExamplePair { input, output });
                }
            } else {
                mapping.style_example_pairs.truncate(3);
                for pair in &mut mapping.style_example_pairs {
                    pair.input = pair.input.trim().chars().take(2_000).collect();
                    pair.output = pair.output.trim().chars().take(2_000).collect();
                }
                mapping
                    .style_example_pairs
                    .retain(|pair| !pair.input.is_empty() && !pair.output.is_empty());
                if let Some(first) = mapping.style_example_pairs.first() {
                    mapping.style_example_input = Some(first.input.clone());
                    mapping.style_example_output = Some(first.output.clone());
                }
            }
        }
        crate::context::normalize_writing_modes(&mut self.writing_modes);
        crate::snippets::normalize_snippets(&mut self.snippets);
        if !matches!(
            self.output_mode.as_str(),
            "auto" | "email" | "bullets" | "meeting_notes" | "code" | "translation"
        ) {
            self.output_mode = default_output_mode();
        }
        if !matches!(
            self.translation_target_language.as_str(),
            "en" | "zh" | "ja" | "ko" | "es" | "fr" | "de"
        ) {
            self.translation_target_language = default_translation_target_language();
        }
        if self.selected_action_hotkey.len() > 128 {
            self.selected_action_hotkey.clear();
        }
        if self.screen_action_hotkey.len() > 128 {
            self.screen_action_hotkey.clear();
        }
        self.vision_provider = self.vision_provider.trim().chars().take(64).collect();
        if !self.vision_provider.is_empty()
            && crate::engine::EngineProvider::parse(&self.vision_provider).is_none()
        {
            self.vision_provider.clear();
        }
        self.vision_model = self.vision_model.trim().chars().take(256).collect();
        self.input_device = self.input_device.trim().chars().take(512).collect();
        self.input_gain = if self.input_gain.is_finite() {
            self.input_gain.clamp(0.5, 4.0)
        } else {
            default_input_gain()
        };
        if self.verbatim_hotkey.len() > 128 {
            self.verbatim_hotkey.clear();
        }
        self.verbatim_hotkey = crate::hotkey::canonicalize_hotkey(&self.verbatim_hotkey);
        if self.translation_hotkey.len() > 128 {
            self.translation_hotkey.clear();
        }
        self.translation_hotkey = crate::hotkey::canonicalize_hotkey(&self.translation_hotkey);
        self.extra_recording_buffer_ms = self.extra_recording_buffer_ms.clamp(0, 2000);
        self.audio_feedback_volume = if self.audio_feedback_volume.is_finite() {
            self.audio_feedback_volume.clamp(0.0, 1.0)
        } else {
            default_audio_feedback_volume()
        };
        self.clamshell_microphone = self.clamshell_microphone.trim().chars().take(512).collect();
        self.whats_new_last_seen_version = self
            .whats_new_last_seen_version
            .trim()
            .chars()
            .take(64)
            .collect();
        if self.whats_new_last_seen_version.is_empty() {
            self.whats_new_last_seen_version = env!("CARGO_PKG_VERSION").into();
        }
        if crate::llm::CleanupIntensity::parse(&self.cleanup_intensity).is_none() {
            self.cleanup_intensity = default_cleanup_intensity();
        }
        if self.cascade_timeout_ms == 0 {
            self.cascade_timeout_ms = default_cascade_timeout_ms();
        }
        if self.cascade_proper_noun_threshold == 0 {
            self.cascade_proper_noun_threshold = default_cascade_proper_noun_threshold();
        }
        self.accurate_asr_model = self.accurate_asr_model.trim().chars().take(256).collect();
        self.accurate_asr_base_url = self
            .accurate_asr_base_url
            .trim()
            .chars()
            .take(2_048)
            .collect();
        self.asr_base_url = self.asr_base_url.trim().chars().take(2_048).collect();
        self.cleanup_base_url = self.cleanup_base_url.trim().chars().take(2_048).collect();
        self.asr_model = crate::asr::resolve_asr_model(&self.asr_model).to_owned();
        if self.asr_model.len() > 256 {
            self.asr_model.truncate(256);
        }
        if self.cleanup_model.len() > 256 {
            self.cleanup_model.truncate(256);
        }
        if self.asr_api_key.len() > 512 {
            self.asr_api_key.truncate(512);
        }
        if self.cleanup_api_key.len() > 512 {
            self.cleanup_api_key.truncate(512);
        }
        // Only the URL can be trusted here. Keys live in the keychain and are
        // blank on disk, so treating an empty key as "incomplete custom" would
        // wipe a working custom URL during load_settings, before credentials
        // are rebound. Empty-key repair happens in
        // the load-side repair helpers only after keys are confirmed absent.
        if self.asr_provider.is_custom()
            && self.custom_base_url.trim().is_empty()
            && self.asr_base_url.trim().is_empty()
        {
            self.asr_provider = crate::engine::EngineProvider::Groq;
        }
        if self.cleanup_provider.is_custom()
            && self.custom_base_url.trim().is_empty()
            && self.cleanup_base_url.trim().is_empty()
        {
            self.cleanup_provider = crate::engine::EngineProvider::Groq;
        }
        if self.asr_provider.is_groq() {
            self.asr_base_url.clear();
            if !crate::asr::is_groq_asr_model(&self.asr_model) {
                self.asr_model = default_asr_model();
            }
        } else if self.asr_provider == crate::engine::EngineProvider::Soniox {
            self.asr_model = crate::asr::SONIOX_MODEL.to_owned();
        } else if matches!(
            self.asr_provider,
            crate::engine::EngineProvider::AssemblyAi | crate::engine::EngineProvider::DashScope
        ) || self.asr_model.trim().is_empty()
        {
            self.asr_model = self.asr_provider.default_asr_model().to_owned();
        }
        if self.cleanup_provider.is_groq() {
            self.cleanup_base_url.clear();
            if !crate::llm::is_supported_model(&self.cleanup_model) {
                self.cleanup_model = default_cleanup_model();
            }
        } else if self.cleanup_model.trim().is_empty() {
            self.cleanup_model = self.cleanup_provider.default_llm_model().to_owned();
        }
        if self.ollama_base_url.trim().is_empty() {
            self.ollama_base_url = crate::providers::EngineProvider::Ollama
                .default_base_url()
                .to_owned();
        }
        if self.local_whisper_base_url.trim().is_empty() {
            self.local_whisper_base_url = crate::providers::EngineProvider::LocalWhisper
                .default_base_url()
                .to_owned();
        }
    }

    fn migrate_provider_pool(&mut self) {
        if self.asr_provider.is_custom() {
            if let Some(named) = crate::providers::infer_provider_from_host(&self.asr_base_url) {
                self.asr_provider = named;
                if !self.asr_api_key.trim().is_empty() {
                    self.provider_api_keys
                        .insert(named.as_str().to_owned(), self.asr_api_key.clone());
                }
                if named == crate::providers::EngineProvider::Ollama {
                    self.ollama_base_url = self.asr_base_url.clone();
                }
                if named == crate::providers::EngineProvider::LocalWhisper {
                    self.local_whisper_base_url = self.asr_base_url.clone();
                }
                if !named.is_custom() {
                    self.asr_base_url.clear();
                }
            } else if !self.asr_base_url.trim().is_empty() {
                self.custom_base_url = self.asr_base_url.clone();
                self.custom_asr = true;
                if !self.asr_api_key.trim().is_empty() {
                    self.provider_api_keys
                        .insert("custom".into(), self.asr_api_key.clone());
                }
            }
        }
        if self.cleanup_provider.is_custom() {
            if let Some(named) = crate::providers::infer_provider_from_host(&self.cleanup_base_url)
            {
                self.cleanup_provider = named;
                if !self.cleanup_api_key.trim().is_empty() {
                    self.provider_api_keys
                        .insert(named.as_str().to_owned(), self.cleanup_api_key.clone());
                }
                if named == crate::providers::EngineProvider::Ollama {
                    self.ollama_base_url = self.cleanup_base_url.clone();
                }
                if !named.is_custom() {
                    self.cleanup_base_url.clear();
                }
            } else if !self.cleanup_base_url.trim().is_empty() {
                if self.custom_base_url.trim().is_empty() {
                    self.custom_base_url = self.cleanup_base_url.clone();
                }
                self.custom_llm = true;
                if !self.cleanup_api_key.trim().is_empty() {
                    self.provider_api_keys
                        .entry("custom".into())
                        .or_insert_with(|| self.cleanup_api_key.clone());
                }
            }
        }
        if !self.api_key.trim().is_empty() {
            self.provider_api_keys
                .entry("groq".into())
                .or_insert_with(|| self.api_key.clone());
        }
    }

    fn repair_incomplete_asr(&mut self) -> bool {
        if !self.asr_provider.is_custom() {
            return false;
        }
        let url = if self.custom_base_url.trim().is_empty() {
            self.asr_base_url.trim()
        } else {
            self.custom_base_url.trim()
        };
        let key = self.asr_credential();
        if !url.is_empty() && (!key.is_empty() || crate::providers::is_loopback_url(url)) {
            return false;
        }
        self.asr_provider = crate::engine::EngineProvider::Groq;
        self.asr_base_url.clear();
        if !crate::asr::is_groq_asr_model(&self.asr_model) {
            self.asr_model = default_asr_model();
        }
        true
    }

    fn repair_incomplete_cleanup(&mut self) -> bool {
        if !self.cleanup_provider.is_custom() {
            return false;
        }
        let url = if self.custom_base_url.trim().is_empty() {
            self.cleanup_base_url.trim()
        } else {
            self.custom_base_url.trim()
        };
        let key = self.cleanup_credential();
        if !url.is_empty() && (!key.is_empty() || crate::providers::is_loopback_url(url)) {
            return false;
        }
        self.cleanup_provider = crate::engine::EngineProvider::Groq;
        self.cleanup_base_url.clear();
        if !crate::llm::is_supported_model(&self.cleanup_model) {
            self.cleanup_model = default_cleanup_model();
        }
        true
    }

    #[cfg(test)]
    pub fn validate(&self) -> anyhow::Result<()> {
        self.validate_with_models_root(None)
    }

    pub fn validate_with_models_root(&self, models_root: Option<&Path>) -> anyhow::Result<()> {
        self.validate_internal(models_root, true)
    }

    /// Structural validation for a binding transaction or schema migration.
    /// Service edits, onboarding completion and recording still check readiness.
    pub fn validate_configuration(&self) -> anyhow::Result<()> {
        self.validate_internal(None, false)
    }

    fn validate_internal(
        &self,
        models_root: Option<&Path>,
        check_readiness: bool,
    ) -> anyhow::Result<()> {
        crate::custom_cleanup_parameters::validate(&self.custom_cleanup_parameters)
            .map_err(|error| anyhow::anyhow!(error))?;
        if !matches!(self.language.as_str(), "auto" | "zh" | "en") {
            anyhow::bail!("unsupported recognition language");
        }
        if !matches!(self.ui_language.as_str(), "system" | "zh" | "en") {
            anyhow::bail!("unsupported interface language");
        }
        if !matches!(self.theme.as_str(), "system" | "light" | "dark") {
            anyhow::bail!("unsupported theme");
        }
        if !(5..=3_600).contains(&self.chunk_threshold_secs) {
            anyhow::bail!("chunk threshold must be between 5 and 3600 seconds");
        }
        if !(15..=60).contains(&self.chunk_length_secs) {
            anyhow::bail!("chunk length must be between 15 and 60 seconds");
        }
        if self.keep_audio_days > 365 {
            anyhow::bail!("audio retention must be 365 days or less");
        }
        if self.keep_history_days > 3_650 {
            anyhow::bail!("history retention must be 3650 days or less");
        }
        if !matches!(
            self.long_output_mode.as_str(),
            "clipboard" | "paste" | "history"
        ) {
            anyhow::bail!("unsupported long output mode");
        }
        if !matches!(
            self.delivery_policy.as_str(),
            "auto" | "paste_shortcut" | "clipboard_only" | "history_only"
        ) {
            anyhow::bail!("unsupported delivery policy");
        }
        if crate::llm::CleanupIntensity::parse(&self.cleanup_intensity).is_none() {
            anyhow::bail!("unsupported cleanup intensity");
        }
        if self.cleanup_provider.is_groq() {
            if !crate::llm::is_supported_model(&self.cleanup_model) {
                anyhow::bail!("unsupported cleanup model");
            }
        } else if self.cleanup_provider == crate::engine::EngineProvider::OnDeviceMlx {
            if self.cleanup_model != crate::modelstore::MODEL_ID
                && !crate::modelstore::is_supported_mlx_model(&self.cleanup_model)
            {
                anyhow::bail!("unsupported local MLX cleanup model");
            }
        } else if self.cleanup_provider == crate::engine::EngineProvider::OnDeviceLlamaCpp {
            if !crate::gguf_cleanup_store::is_supported_model(&self.cleanup_model) {
                anyhow::bail!("unsupported local cleanup model");
            }
        } else if self.cleanup_model.trim().is_empty() {
            anyhow::bail!("自定义整理需要填写模型名。");
        }
        self.validate_bindings()?;
        if self.dictionary.len() > 256 || self.dictionary.iter().any(|word| word.len() > 256) {
            anyhow::bail!("dictionary is too large");
        }
        if self.api_key.len() > 512 {
            anyhow::bail!("API key is too long");
        }
        if self.asr_api_key.len() > 512 {
            anyhow::bail!("ASR API key is too long");
        }
        if self.cleanup_api_key.len() > 512 {
            anyhow::bail!("cleanup API key is too long");
        }
        if self.asr_base_url.len() > 2_048 {
            anyhow::bail!("ASR base URL is too long");
        }
        if self.cleanup_base_url.len() > 2_048 {
            anyhow::bail!("cleanup base URL is too long");
        }
        if self.asr_model.len() > 256 {
            anyhow::bail!("ASR model name is too long");
        }
        crate::asr::validate_asr_base_url(&self.asr_base_url)
            .map_err(|error| anyhow::anyhow!(error))?;
        crate::asr::validate_asr_base_url(&self.cleanup_base_url)
            .map_err(|error| anyhow::anyhow!(error))?;
        crate::asr::validate_asr_base_url(&self.custom_base_url)
            .map_err(|error| anyhow::anyhow!(error))?;
        crate::asr::validate_asr_base_url(&self.ollama_base_url)
            .map_err(|error| anyhow::anyhow!(error))?;
        crate::asr::validate_asr_base_url(&self.local_whisper_base_url)
            .map_err(|error| anyhow::anyhow!(error))?;
        self.validate_provider_side(self.asr_provider, true, models_root, check_readiness)?;
        if self.cleanup_enabled && self.asr_provider != crate::engine::EngineProvider::AssemblyAi {
            self.validate_provider_side(
                self.cleanup_provider,
                false,
                models_root,
                check_readiness,
            )?;
        }
        for mapping in &self.context_mappings {
            mapping.validate().map_err(|error| anyhow::anyhow!(error))?;
        }
        crate::context::validate_writing_modes(&self.writing_modes)
            .map_err(|error| anyhow::anyhow!(error))?;
        crate::snippets::validate_snippets(&self.snippets)
            .map_err(|error| anyhow::anyhow!(error))?;
        if !matches!(
            self.output_mode.as_str(),
            "auto" | "email" | "bullets" | "meeting_notes" | "code" | "translation"
        ) {
            anyhow::bail!("unsupported output mode");
        }
        if !matches!(
            self.translation_target_language.as_str(),
            "en" | "zh" | "ja" | "ko" | "es" | "fr" | "de"
        ) {
            anyhow::bail!("unsupported translation target language");
        }
        if self.selected_action_hotkey.len() > 128 {
            anyhow::bail!("selected action hotkey is too long");
        }

        if self.screen_action_hotkey.len() > 128 {
            anyhow::bail!("look-at-screen hotkey is too long");
        }

        if self.verbatim_hotkey.len() > 128 {
            anyhow::bail!("verbatim hotkey is too long");
        }
        if !self.vision_provider.is_empty()
            && crate::engine::EngineProvider::parse(&self.vision_provider).is_none()
        {
            anyhow::bail!("unsupported vision provider");
        }
        if self.vision_model.len() > 256 {
            anyhow::bail!("vision model name is too long");
        }
        if self.input_device.len() > 512 {
            anyhow::bail!("input device name is too long");
        }
        if !self.input_gain.is_finite() || !(0.5..=4.0).contains(&self.input_gain) {
            anyhow::bail!("input gain must be between 0.5 and 4.0");
        }
        Ok(())
    }

    pub fn validate_bindings(&self) -> anyhow::Result<()> {
        if !matches!(self.activation_mode.as_str(), "tap" | "hold_to_talk") {
            anyhow::bail!("unsupported recording mode");
        }
        let bindings = [
            &self.hotkey,
            &self.selected_action_hotkey,
            &self.screen_action_hotkey,
            &self.verbatim_hotkey,
            &self.translation_hotkey,
        ];
        if self.hotkey.trim().is_empty() {
            anyhow::bail!("hotkey is required");
        }
        for (index, binding) in bindings.iter().enumerate() {
            if binding.len() > 128 {
                anyhow::bail!("hotkey must contain at most 128 characters");
            }
            if binding.trim().is_empty() {
                continue;
            }
            // Preserve deprecated single modifiers so users can see and replace
            // their old bindings. New bindings are checked at the transaction boundary.
            if !crate::modifier_hotkey::is_modifier_only(binding) {
                binding
                    .parse::<tauri_plugin_global_shortcut::Shortcut>()
                    .map_err(|_| anyhow::anyhow!("invalid hotkey: {binding}"))?;
            }
            for other in bindings
                .iter()
                .skip(index + 1)
                .filter(|key| !key.trim().is_empty())
            {
                if crate::hotkey::bindings_equal(binding, other) {
                    anyhow::bail!("hotkey conflicts with another shortcut");
                }
            }
        }
        Ok(())
    }

    pub fn validate_binding_changes(&self, previous: &Self) -> anyhow::Result<()> {
        self.validate_bindings()?;
        for (new, old, recording) in [
            (&self.hotkey, &previous.hotkey, true),
            (&self.verbatim_hotkey, &previous.verbatim_hotkey, true),
            (&self.translation_hotkey, &previous.translation_hotkey, true),
            (
                &self.selected_action_hotkey,
                &previous.selected_action_hotkey,
                false,
            ),
            (
                &self.screen_action_hotkey,
                &previous.screen_action_hotkey,
                false,
            ),
        ] {
            if new != old
                && crate::modifier_hotkey::is_modifier_only(new)
                && !(recording && crate::modifier_hotkey::is_fn_only(new))
            {
                anyhow::bail!(if recording {
                    "请再按一个键组成快捷键；单键录音可选择 Fn。"
                } else {
                    "请再按一个键组成快捷键。"
                });
            }
            if new != old
                && !new.trim().is_empty()
                && !crate::modifier_hotkey::is_modifier_only(new)
            {
                use tauri_plugin_global_shortcut::{Modifiers, Shortcut};
                let shortcut: Shortcut = new
                    .parse()
                    .map_err(|_| anyhow::anyhow!("invalid hotkey: {new}"))?;
                let key = shortcut.key.to_string();
                let function_key = key
                    .strip_prefix('F')
                    .and_then(|number| number.parse::<u8>().ok())
                    .is_some_and(|number| (1..=24).contains(&number));
                if !shortcut
                    .mods
                    .intersects(Modifiers::SUPER | Modifiers::CONTROL | Modifiers::ALT)
                    && !function_key
                {
                    anyhow::bail!(if recording {
                        "请使用 ⌘、⌥ 或 ⌃ 组成快捷键，或选择 Fn。"
                    } else {
                        "请使用 ⌘、⌥ 或 ⌃ 加上另一个键。"
                    });
                }
            }
        }
        Ok(())
    }

    pub fn on_device_asr_ready(&self, models_root: Option<&Path>) -> bool {
        self.asr_provider == crate::engine::EngineProvider::OnDevice
            && models_root.is_some_and(|root| {
                crate::ondevice_asr::model_setup_is_ready(root, &self.asr_model)
            })
    }

    fn validate_provider_side(
        &self,
        provider: crate::engine::EngineProvider,
        asr: bool,
        models_root: Option<&Path>,
        check_readiness: bool,
    ) -> anyhow::Result<()> {
        if asr && !provider.has_asr() {
            anyhow::bail!("所选服务商不支持转写。");
        }
        if !asr && !provider.has_llm() {
            anyhow::bail!("OnDevice 不能用于文字整理。");
        }
        if provider == crate::engine::EngineProvider::OnDevice && asr {
            if check_readiness && self.onboarded && !self.on_device_asr_ready(models_root) {
                anyhow::bail!("本机语音识别尚未就绪，不能完成设置。");
            }
            return Ok(());
        }
        if !provider.has_http_asr()
            && asr
            && !matches!(
                provider,
                crate::engine::EngineProvider::Soniox | crate::engine::EngineProvider::DashScope
            )
        {
            return Ok(());
        }
        let url = self.resolved_provider_base(provider);
        if provider == crate::engine::EngineProvider::DashScope
            && crate::providers::resolve_asr_endpoint(provider, &url).is_none()
        {
            anyhow::bail!("请选择有效的 DashScope 区域端点。");
        }
        if provider.is_custom() && url.trim().is_empty() {
            anyhow::bail!(if asr {
                "自定义 ASR 需要填写兼容地址。"
            } else {
                "自定义整理需要填写兼容地址。"
            });
        }
        let key = if provider.is_custom() && self.session_only_custom_credential {
            let purpose = if asr {
                CustomCredentialPurpose::Asr
            } else {
                CustomCredentialPurpose::Cleanup
            };
            self.session_custom_credential_for(purpose, &url)
        } else {
            self.provider_secret(provider)
        };
        let empty_ok = matches!(
            provider,
            crate::engine::EngineProvider::OnDeviceMlx
                | crate::engine::EngineProvider::OnDeviceLlamaCpp
        ) || provider.allows_empty_key() && crate::providers::is_loopback_url(&url);
        if key.is_empty() && !empty_ok {
            if !check_readiness || !self.onboarded {
                return Ok(());
            }
            let read_state = if provider.is_custom() {
                self.custom_credential_read_state()
            } else {
                self.credential_read_state_for_slot(provider.keychain_account(), key)
            };
            if read_state == CredentialReadState::Unavailable {
                anyhow::bail!("钥匙串暂时不可读取；当前设置未更改，请在前台设置中重试读取。")
            }
            if provider.is_custom() {
                anyhow::bail!(if asr {
                    "自定义 ASR 地址需要填写 ASR 密钥。"
                } else {
                    "自定义整理地址需要填写整理密钥。"
                });
            }
            anyhow::bail!("缺少所选服务商的密钥。");
        }
        Ok(())
    }

    pub fn provider_secret(&self, provider: crate::engine::EngineProvider) -> &str {
        if provider.is_custom()
            && (self.session_custom_credential_rejected || self.session_only_custom_credential)
        {
            return "";
        }
        if let Some(key) = self
            .provider_api_keys
            .get(provider.as_str())
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return key;
        }
        match provider {
            crate::engine::EngineProvider::Groq => self.api_key.trim(),
            crate::engine::EngineProvider::Custom => {
                let asr = self.asr_api_key.trim();
                if !asr.is_empty() {
                    asr
                } else {
                    self.cleanup_api_key.trim()
                }
            }
            _ => "",
        }
    }

    fn credential_read_state_for_slot(
        &self,
        slot: &str,
        runtime_value: &str,
    ) -> CredentialReadState {
        self.credential_read_states
            .get(slot)
            .copied()
            .unwrap_or_else(|| {
                if runtime_value.trim().is_empty() {
                    CredentialReadState::Missing
                } else {
                    CredentialReadState::Configured
                }
            })
    }

    pub fn custom_credential_read_state(&self) -> CredentialReadState {
        let scope = custom_credential_recovery_scope(self, CustomCredentialPurpose::Asr)
            .or_else(|| custom_credential_recovery_scope(self, CustomCredentialPurpose::Cleanup));
        let Some(scope) = scope else {
            return self.credential_read_state_for_slot(
                "provider_custom",
                self.provider_secret(crate::engine::EngineProvider::Custom),
            );
        };

        let mut slots = vec!["provider_custom"];
        if scope.include_asr_legacy {
            slots.push("asr_api_key");
        }
        if scope.include_cleanup_legacy {
            slots.push("cleanup_api_key");
        }
        for slot in slots {
            let runtime_value = match slot {
                "provider_custom" => self
                    .provider_api_keys
                    .get("custom")
                    .map(String::as_str)
                    .unwrap_or_default(),
                "asr_api_key" => self.asr_api_key.as_str(),
                "cleanup_api_key" => self.cleanup_api_key.as_str(),
                _ => "",
            };
            match self.credential_read_state_for_slot(slot, runtime_value) {
                CredentialReadState::Configured => return CredentialReadState::Configured,
                CredentialReadState::Unavailable => return CredentialReadState::Unavailable,
                CredentialReadState::Missing => {}
            }
        }
        CredentialReadState::Missing
    }

    pub fn asr_credential_read_state(&self) -> CredentialReadState {
        if self.asr_provider.is_custom() {
            self.custom_credential_read_state()
        } else {
            self.credential_read_state_for_slot(
                self.asr_provider.keychain_account(),
                self.asr_credential(),
            )
        }
    }

    /// Mark non-empty runtime credentials configured after a successful
    /// settings commit, while preserving unavailable legacy values until a
    /// verified replacement has removed their unverified-source marker.
    pub(crate) fn refresh_verified_credential_read_states(&mut self) {
        let mut values = vec![
            ("groq_api_key", self.api_key.as_str()),
            ("asr_api_key", self.asr_api_key.as_str()),
            ("cleanup_api_key", self.cleanup_api_key.as_str()),
        ];
        for (provider_id, key) in &self.provider_api_keys {
            if let Some(provider) = crate::providers::EngineProvider::parse(provider_id) {
                values.push((provider.keychain_account(), key.as_str()));
            }
        }
        for (slot, value) in values {
            if value.trim().is_empty() || has_unverified_source(self, slot, value) {
                continue;
            }
            self.credential_read_states
                .insert(slot.to_owned(), CredentialReadState::Configured);
        }
    }

    pub(crate) fn preserve_credential_runtime_metadata_from(&mut self, previous: &Settings) {
        self.unverified_credential_sources = previous.unverified_credential_sources.clone();
        self.credential_baselines = previous.credential_baselines.clone();
        self.credential_read_states = previous.credential_read_states.clone();
        self.session_only_custom_credential = previous.session_only_custom_credential;
        self.session_custom_credential_scope = previous.session_custom_credential_scope.clone();
        self.session_custom_credential_rejected = previous.session_custom_credential_rejected;
    }

    fn session_custom_credential_scope_matches(&self) -> bool {
        let Some(captured) = self.session_custom_credential_scope.as_ref() else {
            return false;
        };
        self.provider_api_keys
            .get("custom")
            .is_some_and(|key| !key.trim().is_empty())
            && custom_credential_recovery_scope(self, captured.purpose).as_ref() == Some(captured)
    }

    fn session_custom_credential_matches_base(
        &self,
        purpose: CustomCredentialPurpose,
        base_url: &str,
    ) -> bool {
        let Some(captured) = self.session_custom_credential_scope.as_ref() else {
            return false;
        };
        let purpose_is_active = match purpose {
            CustomCredentialPurpose::Asr => captured.asr_active,
            CustomCredentialPurpose::Cleanup => captured.cleanup_active,
        };
        if !self.session_only_custom_credential || !purpose_is_active {
            return false;
        }
        let Some(current) = custom_credential_recovery_scope(self, purpose) else {
            return false;
        };
        let mut expected = captured.clone();
        expected.purpose = purpose;
        current == expected
            && canonical_http_origin(base_url).as_deref() == Some(captured.origin.as_str())
    }

    /// Return a foreground-only Custom value only to a currently active
    /// ASR/cleanup purpose whose destination still has the captured origin.
    /// Generic Custom consumers must use `provider_secret`, which deliberately
    /// hides session-only values.
    pub(crate) fn session_custom_credential_for(
        &self,
        purpose: CustomCredentialPurpose,
        base_url: &str,
    ) -> &str {
        if !self.session_custom_credential_matches_base(purpose, base_url) {
            return "";
        }
        self.provider_api_keys
            .get("custom")
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("")
    }

    /// Drop a foreground-only Custom value when the active purpose set or
    /// effective service origin no longer matches the read that adopted it.
    /// This only clears process memory and never touches Keychain.
    pub(crate) fn clear_stale_session_custom_credential(&mut self) -> bool {
        if !self.session_only_custom_credential {
            return false;
        }
        if self.session_custom_credential_scope_matches() {
            return false;
        }
        self.clear_session_custom_credential();
        true
    }

    pub(crate) fn clear_session_custom_credential(&mut self) {
        if self.session_only_custom_credential {
            self.provider_api_keys.remove("custom");
            self.credential_baselines.remove("provider_custom");
            self.session_custom_credential_rejected = true;
        }
        self.session_only_custom_credential = false;
        self.session_custom_credential_scope = None;
    }

    pub fn resolved_provider_base(&self, provider: crate::engine::EngineProvider) -> String {
        match provider {
            crate::engine::EngineProvider::Groq => String::new(),
            crate::engine::EngineProvider::Ollama => {
                let url = self.ollama_base_url.trim();
                if url.is_empty() {
                    provider.default_base_url().to_owned()
                } else {
                    url.to_owned()
                }
            }
            crate::engine::EngineProvider::LocalWhisper => {
                let url = self.local_whisper_base_url.trim();
                if url.is_empty() {
                    provider.default_base_url().to_owned()
                } else {
                    url.to_owned()
                }
            }
            crate::engine::EngineProvider::Custom => {
                let custom = self.custom_base_url.trim();
                if !custom.is_empty() {
                    custom.to_owned()
                } else if !self.asr_base_url.trim().is_empty() {
                    self.asr_base_url.trim().to_owned()
                } else {
                    self.cleanup_base_url.trim().to_owned()
                }
            }
            crate::engine::EngineProvider::AssemblyAi => crate::engine::EngineProvider::AssemblyAi
                .default_base_url()
                .to_owned(),
            crate::engine::EngineProvider::DashScope => {
                let url = self.asr_base_url.trim();
                if url.is_empty() {
                    provider.default_base_url().to_owned()
                } else {
                    url.to_owned()
                }
            }
            crate::engine::EngineProvider::OnDevice => String::new(),
            other => other.default_base_url().to_owned(),
        }
    }

    pub fn asr_endpoint(&self) -> Option<String> {
        crate::providers::resolve_asr_endpoint(
            self.asr_provider,
            &self.resolved_provider_base(self.asr_provider),
        )
    }

    /// Prefer a dedicated ASR key when set. Reuse the Groq key only for the
    /// Groq default or `api.groq.com`; custom hosts must supply `asr_api_key`.
    pub fn asr_credential(&self) -> &str {
        if self.asr_provider.is_custom() && self.session_only_custom_credential {
            return self.session_custom_credential_for(
                CustomCredentialPurpose::Asr,
                &self.resolved_provider_base(crate::engine::EngineProvider::Custom),
            );
        }
        self.provider_secret(self.asr_provider)
    }

    pub fn accurate_asr_configured(&self) -> bool {
        self.accurate_asr_provider.has_http_asr()
            && !self.accurate_asr_model.trim().is_empty()
            && !self.accurate_asr_credential().trim().is_empty()
    }

    pub fn accurate_asr_credential(&self) -> &str {
        let base = if !self.accurate_asr_base_url.trim().is_empty() {
            self.accurate_asr_base_url.trim().to_owned()
        } else {
            self.resolved_provider_base(self.accurate_asr_provider)
        };
        if self.accurate_asr_provider.is_custom() && self.session_only_custom_credential {
            return self.session_custom_credential_for(CustomCredentialPurpose::Asr, &base);
        }
        self.provider_secret(self.accurate_asr_provider)
    }

    pub fn accurate_asr_endpoint(&self) -> Option<String> {
        if !self.accurate_asr_provider.has_http_asr() {
            return None;
        }
        let base = if !self.accurate_asr_base_url.trim().is_empty() {
            self.accurate_asr_base_url.trim().to_owned()
        } else {
            self.resolved_provider_base(self.accurate_asr_provider)
        };
        crate::providers::resolve_asr_endpoint(self.accurate_asr_provider, &base)
    }

    pub fn vision_configured(&self) -> bool {
        if !crate::screen_action::vision_settings_ready(&self.vision_provider, &self.vision_model) {
            return false;
        }
        let Some(provider) = crate::engine::EngineProvider::parse(&self.vision_provider) else {
            return false;
        };
        if !provider.has_vision() {
            return false;
        }
        let key = self.provider_secret(provider);
        !key.is_empty() || provider.allows_empty_key()
    }

    pub fn vision_credential(&self) -> &str {
        match crate::engine::EngineProvider::parse(&self.vision_provider) {
            Some(provider) => self.provider_secret(provider),
            None => "",
        }
    }

    pub fn vision_endpoint(&self) -> Option<String> {
        let provider = crate::engine::EngineProvider::parse(&self.vision_provider)?;
        if !provider.has_vision() {
            return None;
        }
        Some(crate::llm::resolve_chat_url(
            &self.resolved_provider_base(provider),
        ))
    }

    pub fn cleanup_credential(&self) -> &str {
        if self.cleanup_provider.is_custom() && self.session_only_custom_credential {
            return self.session_custom_credential_for(
                CustomCredentialPurpose::Cleanup,
                &self.resolved_provider_base(crate::engine::EngineProvider::Custom),
            );
        }
        self.provider_secret(self.cleanup_provider)
    }

    pub fn cleanup_endpoint(&self) -> String {
        if self.cleanup_provider == crate::engine::EngineProvider::Ollama {
            return crate::ollama_local::chat_endpoint(&self.ollama_base_url);
        }
        crate::providers::resolve_llm_endpoint(
            self.cleanup_provider,
            &self.resolved_provider_base(self.cleanup_provider),
        )
    }

    pub fn cleanup_request_model(&self) -> String {
        if self.cleanup_provider.is_groq() {
            if crate::llm::is_supported_model(&self.cleanup_model) {
                self.cleanup_model.clone()
            } else {
                crate::llm::MODEL.to_owned()
            }
        } else {
            let model = self.cleanup_model.trim();
            if model.is_empty() {
                self.cleanup_provider.default_llm_model().to_owned()
            } else {
                model.to_owned()
            }
        }
    }
}

pub(crate) fn bind_asr_key_to_host(
    provider: crate::engine::EngineProvider,
    previous_url: &str,
    next_url: &str,
    incoming_key: &str,
    previous_key: &str,
) -> String {
    if !incoming_key.trim().is_empty() {
        return incoming_key.to_string();
    }
    let host_changed = if provider == crate::engine::EngineProvider::Custom {
        crate::llm::custom_connection_origin_changed(previous_url, next_url)
    } else {
        crate::asr::asr_host_changed(previous_url, next_url)
    };
    if host_changed {
        String::new()
    } else {
        previous_key.to_string()
    }
}

pub(crate) fn bind_cleanup_key_to_host(
    provider: crate::engine::EngineProvider,
    previous_url: &str,
    next_url: &str,
    incoming_key: &str,
    previous_key: &str,
) -> String {
    if !incoming_key.trim().is_empty() {
        return incoming_key.to_string();
    }
    let host_changed = if provider == crate::engine::EngineProvider::Custom {
        crate::llm::custom_connection_origin_changed(previous_url, next_url)
    } else {
        crate::llm::chat_host_changed(previous_url, next_url)
    };
    if host_changed {
        String::new()
    } else {
        previous_key.to_string()
    }
}

/// Settings exposed to the webview. The backend keeps the real credential in
/// memory/keychain, while the UI only receives whether one is configured and a
/// non-sensitive hint for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomCredentialRecoveryScope {
    pub origin: String,
    pub purpose: CustomCredentialPurpose,
    pub asr_active: bool,
    pub cleanup_active: bool,
    pub include_asr_legacy: bool,
    pub include_cleanup_legacy: bool,
}

fn canonical_http_origin(base_url: &str) -> Option<String> {
    let base_url = base_url.trim();
    if base_url.is_empty() {
        return None;
    }
    let candidate = if base_url.contains("://") {
        base_url.to_owned()
    } else {
        format!("https://{base_url}")
    };
    let url = reqwest::Url::parse(&candidate).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

/// Build the only safe legacy-alias set for a foreground Custom reread.
/// The primary Custom account remains first. Legacy ASR/cleanup accounts are
/// eligible only while their own route currently uses Custom, and both active
/// Custom purposes must resolve to the same valid origin.
pub fn custom_credential_recovery_scope(
    settings: &Settings,
    purpose: CustomCredentialPurpose,
) -> Option<CustomCredentialRecoveryScope> {
    let asr_custom_active = settings.asr_provider.is_custom();
    let cleanup_custom_active = settings.cleanup_enabled && settings.cleanup_provider.is_custom();
    let requested_purpose_is_active = match purpose {
        CustomCredentialPurpose::Asr => asr_custom_active,
        CustomCredentialPurpose::Cleanup => cleanup_custom_active,
    };
    if !requested_purpose_is_active {
        return None;
    }

    let origin = canonical_http_origin(
        &settings.resolved_provider_base(crate::engine::EngineProvider::Custom),
    )?;
    // Legacy account names do not identify an origin. Only consult each one
    // when that purpose's saved endpoint independently proves the same origin.
    let include_asr_legacy = asr_custom_active
        && canonical_http_origin(&settings.asr_base_url).as_deref() == Some(origin.as_str());
    let include_cleanup_legacy = cleanup_custom_active
        && canonical_http_origin(&settings.cleanup_base_url).as_deref() == Some(origin.as_str());
    Some(CustomCredentialRecoveryScope {
        origin,
        purpose,
        asr_active: asr_custom_active,
        cleanup_active: cleanup_custom_active,
        include_asr_legacy,
        include_cleanup_legacy,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderKeyView {
    /// A credential string is currently available in this process.
    pub configured: bool,
    pub hint: Option<String>,
    /// Last Keychain resolution, kept separate from current session memory.
    pub keychain_state: CredentialReadState,
}

fn credential_hint(key: &str) -> Option<String> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    if key.chars().count() <= 5 {
        return Some("••••".into());
    }
    let tail: String = key.chars().rev().take(5).collect();
    Some(format!("••••{}", tail.chars().rev().collect::<String>()))
}

#[cfg(test)]
pub(crate) fn add_manual_dictionary_words(settings: &mut Settings, words: &[String]) -> usize {
    words
        .iter()
        .filter(|word| settings.add_dictionary_source(word, DictionarySource::Manual))
        .count()
}

pub(crate) fn reconcile_dictionary_patch(
    settings: &mut Settings,
    requested: Vec<String>,
) -> Vec<String> {
    settings.reconcile_dictionary_patch(requested)
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsView {
    pub schema_version: u32,
    pub api_key_configured: bool,
    pub api_key_hint: Option<String>,
    pub api_key_keychain_state: CredentialReadState,
    pub asr_api_key_configured: bool,
    pub asr_api_key_hint: Option<String>,
    pub asr_api_key_keychain_state: CredentialReadState,
    pub asr_base_url: String,
    pub asr_provider: crate::engine::EngineProvider,
    pub asr_model: String,
    pub cleanup_model: String,
    pub cleanup_provider: crate::engine::EngineProvider,
    pub cleanup_base_url: String,
    pub cleanup_api_key_configured: bool,
    pub cleanup_api_key_hint: Option<String>,
    pub cleanup_api_key_keychain_state: CredentialReadState,
    pub custom_base_url: String,
    pub custom_asr: bool,
    pub custom_llm: bool,
    pub custom_cleanup_parameters: crate::custom_cleanup_parameters::Parameters,
    pub ollama_base_url: String,
    pub strict_offline_enabled: bool,
    pub local_whisper_base_url: String,
    pub provider_keys: std::collections::BTreeMap<String, ProviderKeyView>,
    pub language: String,
    pub ui_language: String,
    pub theme: String,
    pub dictionary: Vec<String>,
    pub hotkey: String,
    pub activation_mode: String,
    pub hotkey_error: Option<String>,
    pub chunk_threshold_secs: u64,
    pub chunk_length_secs: usize,
    pub long_output_mode: String,
    pub delivery_policy: String,
    pub keep_audio_days: u64,
    pub keep_history_days: u64,
    pub keep_success_audio: bool,
    pub onboarded: bool,
    pub cleanup_enabled: bool,
    pub cleanup_intensity: String,
    pub accurate_asr_provider: crate::engine::EngineProvider,
    pub accurate_asr_model: String,
    pub accurate_asr_base_url: String,
    pub cascade_timeout_ms: u64,
    pub cascade_proper_noun_threshold: usize,
    pub window_ocr_enabled: bool,
    pub screen_action_hotkey: String,
    pub vision_provider: String,
    pub vision_model: String,
    pub show_tray_icon: bool,
    pub context_enabled: bool,
    pub browser_access_enabled: bool,
    pub context_mappings: Vec<crate::context::AppMapping>,
    pub writing_modes: Vec<crate::context::WritingMode>,
    pub snippets: Vec<crate::snippets::Snippet>,
    pub output_mode: String,
    pub translation_target_language: String,
    pub selected_action_hotkey: String,
    pub selected_actions_enabled: bool,
    pub dictionary_learn_enabled: bool,
    pub local_edit_learning_enabled: bool,
    pub input_device: String,
    pub input_gain: f32,
    pub verbatim_hotkey: String,
    pub translation_hotkey: String,
    pub extra_recording_buffer_ms: u64,
    pub audio_feedback_enabled: bool,
    pub audio_feedback_volume: f32,
    pub vad_enabled: bool,
    pub always_on_microphone: bool,
    pub clamshell_microphone: String,
    pub autostart_enabled: bool,
    pub whats_new_last_seen_version: String,
    pub debug_mode: bool,
    pub fuzzy_dictionary_enabled: bool,
}

fn provider_key_views(settings: &Settings) -> std::collections::BTreeMap<String, ProviderKeyView> {
    crate::providers::EngineProvider::ALL
        .into_iter()
        .map(|provider| {
            let secret = if provider.is_custom() && settings.session_only_custom_credential {
                let asr = settings.asr_credential();
                if !asr.is_empty() {
                    asr
                } else {
                    settings.cleanup_credential()
                }
            } else {
                settings.provider_secret(provider)
            };
            (
                provider.as_str().to_owned(),
                ProviderKeyView {
                    configured: !secret.is_empty(),
                    hint: credential_hint(secret),
                    keychain_state: if provider.is_custom() {
                        settings.custom_credential_read_state()
                    } else {
                        settings.credential_read_state_for_slot(provider.keychain_account(), secret)
                    },
                },
            )
        })
        .collect()
}

impl From<&Settings> for SettingsView {
    fn from(settings: &Settings) -> Self {
        Self {
            schema_version: settings.schema_version,
            api_key_configured: !settings.api_key.is_empty(),
            api_key_hint: credential_hint(&settings.api_key),
            api_key_keychain_state: settings
                .credential_read_state_for_slot("groq_api_key", &settings.api_key),
            asr_api_key_configured: !settings.asr_api_key.trim().is_empty(),
            asr_api_key_hint: credential_hint(&settings.asr_api_key),
            asr_api_key_keychain_state: settings
                .credential_read_state_for_slot("asr_api_key", &settings.asr_api_key),
            asr_base_url: settings.asr_base_url.clone(),
            asr_provider: settings.asr_provider,
            asr_model: crate::asr::resolve_asr_model(&settings.asr_model).to_owned(),
            cleanup_model: settings.cleanup_model.clone(),
            cleanup_provider: settings.cleanup_provider,
            cleanup_base_url: settings.cleanup_base_url.clone(),
            cleanup_api_key_configured: !settings.cleanup_api_key.trim().is_empty(),
            cleanup_api_key_hint: credential_hint(&settings.cleanup_api_key),
            cleanup_api_key_keychain_state: settings
                .credential_read_state_for_slot("cleanup_api_key", &settings.cleanup_api_key),
            custom_base_url: settings.custom_base_url.clone(),
            custom_asr: settings.custom_asr,
            custom_llm: settings.custom_llm,
            custom_cleanup_parameters: settings.custom_cleanup_parameters.clone(),
            ollama_base_url: settings.ollama_base_url.clone(),
            strict_offline_enabled: settings.strict_offline_enabled,
            local_whisper_base_url: settings.local_whisper_base_url.clone(),
            provider_keys: provider_key_views(settings),
            language: settings.language.clone(),
            ui_language: settings.ui_language.clone(),
            theme: settings.theme.clone(),
            dictionary: settings.dictionary.clone(),
            hotkey: settings.hotkey.clone(),
            activation_mode: settings.activation_mode.clone(),
            hotkey_error: crate::hotkey::registration_error(),
            chunk_threshold_secs: settings.chunk_threshold_secs,
            chunk_length_secs: settings.chunk_length_secs,
            long_output_mode: settings.long_output_mode.clone(),
            delivery_policy: settings.delivery_policy.clone(),
            keep_audio_days: settings.keep_audio_days,
            keep_history_days: settings.keep_history_days,
            keep_success_audio: settings.keep_success_audio,
            onboarded: settings.onboarded,
            cleanup_enabled: settings.cleanup_enabled,
            cleanup_intensity: settings.cleanup_intensity.clone(),
            accurate_asr_provider: settings.accurate_asr_provider,
            accurate_asr_model: settings.accurate_asr_model.clone(),
            accurate_asr_base_url: settings.accurate_asr_base_url.clone(),
            cascade_timeout_ms: settings.cascade_timeout_ms,
            cascade_proper_noun_threshold: settings.cascade_proper_noun_threshold,
            window_ocr_enabled: settings.window_ocr_enabled,
            screen_action_hotkey: settings.screen_action_hotkey.clone(),
            vision_provider: settings.vision_provider.clone(),
            vision_model: settings.vision_model.clone(),
            show_tray_icon: settings.show_tray_icon,
            context_enabled: settings.context_enabled,
            browser_access_enabled: settings.browser_access_enabled,
            context_mappings: settings.context_mappings.clone(),
            writing_modes: settings.writing_modes.clone(),
            snippets: settings.snippets.clone(),
            output_mode: settings.output_mode.clone(),
            translation_target_language: settings.translation_target_language.clone(),
            selected_action_hotkey: settings.selected_action_hotkey.clone(),
            selected_actions_enabled: settings.selected_actions_enabled,
            dictionary_learn_enabled: settings.dictionary_learn_enabled,
            local_edit_learning_enabled: settings.local_edit_learning_enabled,
            input_device: settings.input_device.clone(),
            input_gain: settings.input_gain,
            verbatim_hotkey: settings.verbatim_hotkey.clone(),
            translation_hotkey: settings.translation_hotkey.clone(),
            extra_recording_buffer_ms: settings.extra_recording_buffer_ms,
            audio_feedback_enabled: settings.audio_feedback_enabled,
            audio_feedback_volume: settings.audio_feedback_volume,
            vad_enabled: settings.vad_enabled,
            always_on_microphone: settings.always_on_microphone,
            clamshell_microphone: settings.clamshell_microphone.clone(),
            autostart_enabled: settings.autostart_enabled,
            whats_new_last_seen_version: settings.whats_new_last_seen_version.clone(),
            debug_mode: settings.debug_mode,
            fuzzy_dictionary_enabled: settings.fuzzy_dictionary_enabled,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct HistoryItem {
    pub id: i64,
    pub created_at: String,
    pub raw_text: String,
    /// Provider-returned transcript before local filtering, revisions, or
    /// dictionary replacement. Null on rows written before this field existed.
    pub asr_text: Option<String>,
    /// Provider-supplied cleanup candidate kept distinct from the ASR text and
    /// VoiceFlow's final guarded output. Null when the provider supplied none.
    pub provider_cleaned_candidate: Option<String>,
    /// Complete cleanup text rejected by a semantic preservation guard.
    /// Separate from provider_cleaned_candidate, which can belong to ASR.
    pub cleanup_rejected_candidate: Option<String>,
    /// Stage-qualified, non-sensitive preservation reason code.
    pub cleanup_guard_reason: Option<String>,
    /// Nullable provenance; None means this legacy/edited row has unknown provenance.
    #[serde(default)]
    pub cleanup_attempt_kind: Option<String>,
    /// None is unknown for legacy rows; Some([]) is an observed no-transform result.
    #[serde(default)]
    pub cleanup_transformations: Option<Vec<String>>,
    #[serde(default)]
    pub cleanup_final_guard_reason: Option<String>,
    #[serde(default)]
    pub cleanup_failure_reason: Option<String>,
    /// ASR provider/model that produced the selected transcript, when known.
    pub engine: Option<String>,
    pub final_text: String,
    pub cleanup_status: String,
    pub duration: f64,
    pub degraded: bool,
    pub degraded_reason: Option<String>,
    pub status: String,
    pub delivery_method: Option<String>,
    pub fallback_reason: Option<String>,
    /// Stable reason code for a paste delivery failure or unverified result.
    pub delivery_error_code: Option<String>,
    /// Safe, transcript-free next step shown in History.
    pub delivery_user_reason: Option<String>,
    pub context_profile_id: Option<String>,
    pub retryable: bool,
    pub revision_count: usize,
    pub has_audio: bool,
    pub verbatim_text: Option<String>,
    pub verbatim_reviewed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryRevision {
    pub revision_id: i64,
    pub dictation_id: i64,
    pub created_at: String,
    pub final_text: String,
    pub cleanup_status: Option<String>,
    pub intent: Option<serde_json::Value>,
    pub model: Option<String>,
    pub context_policy: Option<serde_json::Value>,
    pub revision_reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryPage {
    pub items: Vec<HistoryItem>,
    pub has_more: bool,
}
#[derive(Debug, Clone, Serialize)]
#[cfg(test)]
pub struct Usage {
    pub asr_requests: i64,
    pub llm_requests: i64,
    pub audio_seconds: f64,
    pub quota: QuotaView,
}
pub const MAX_SPOOL_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_GOLD_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const SPOOL_MANIFEST_VERSION: u32 = 1;
const AUDIO_SAMPLE_RATE: usize = 16_000;
/// Prefix of the retired app-layer spool encryption envelope. Such files are
/// rejected instead of being returned as audio.
const SPOOL_ENVELOPE_MAGIC: &[u8; 8] = b"VFSPOOL1";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SpoolChunkManifest {
    index: usize,
    start_secs: f32,
    end_secs: f32,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_start_sample: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sample_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SpoolManifest {
    schema_version: u32,
    session_id: String,
    created_at_ms: u64,
    status: String,
    chunks: Vec<SpoolChunkManifest>,
}

#[derive(Debug, Clone)]
pub struct RecoveredSpool {
    pub audio_path: PathBuf,
    pub duration_secs: f64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn spool_manifest_age_secs(manifest: &SpoolManifest, now_ms: u64) -> Option<u64> {
    if manifest.created_at_ms == 0 || manifest.created_at_ms > now_ms {
        return None;
    }
    Some((now_ms - manifest.created_at_ms) / 1_000)
}

fn spool_session_expired(
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

fn spool_flat_artifact_expired(
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
pub(crate) fn expire_spool(
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

fn is_safe_spool_path(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

fn gold_root(dir: &Path) -> PathBuf {
    dir.join("gold")
}

fn is_safe_managed_audio_path(dir: &Path, path: &Path) -> bool {
    is_safe_spool_path(&dir.join("spool"), path) || is_safe_spool_path(&gold_root(dir), path)
}

fn managed_audio_exists(dir: &Path, path: &Path) -> bool {
    is_safe_managed_audio_path(dir, path) && path.is_file()
}

fn spool_size(path: &Path) -> anyhow::Result<u64> {
    if spool_session_is_leased(path) {
        return spool_size_live_owner_snapshot(path);
    }
    spool_size_walk(path)
}

fn spool_size_live_owner_snapshot(path: &Path) -> anyhow::Result<u64> {
    // Quota checks read lengths only. Audio chunks and manifests become
    // visible by atomic rename, and any in-progress temp file is counted
    // conservatively; recovery and content readers use the typed busy guard.
    spool_size_walk(path)
}

fn spool_size_walk(path: &Path) -> anyhow::Result<u64> {
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

pub(crate) fn read_audio_spool_file(
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

fn reject_retired_spool_envelope(bytes: &[u8]) -> anyhow::Result<()> {
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
pub(crate) fn write_audio_spool_file(
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
pub(crate) fn write_audio_spool_chunk_with_samples(
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

fn validated_spool_target(dir: &Path, relative: &Path) -> anyhow::Result<PathBuf> {
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

fn spool_session_key_for_relative(dir: &Path, relative: &Path) -> anyhow::Result<PathBuf> {
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

fn spool_session_key_for_artifact(path: &Path) -> Option<PathBuf> {
    let managed_root = path.ancestors().find(|ancestor| {
        ancestor
            .file_name()
            .is_some_and(|name| name == "spool" || name == "gold")
    })?;
    let relative = path.strip_prefix(managed_root).ok()?;
    let first = relative.components().next()?;
    Some(managed_root.join(first.as_os_str()))
}

fn spool_lease_kind_for_artifact(path: &Path, owner_path: &Path) -> SpoolLeaseKind {
    if path == owner_path && !path.is_dir() {
        SpoolLeaseKind::Artifact
    } else {
        SpoolLeaseKind::Session
    }
}

fn write_spool_file_internal(
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

fn write_spool_file_after_admission(
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

pub(crate) fn delete_audio_spool_session(lease: &AudioSpoolSessionLease) -> anyhow::Result<()> {
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
fn settle_capture_spool_after_history_ack(
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

fn write_atomic_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    write_atomic_bytes_with_parent_mode(path, bytes, true)
}

fn write_atomic_bytes_with_parent_mode(
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

fn manifest_path(session_dir: &Path) -> PathBuf {
    session_dir.join("manifest.json")
}

fn load_manifest(session_dir: &Path) -> anyhow::Result<SpoolManifest> {
    Ok(serde_json::from_slice(&fs::read(manifest_path(
        session_dir,
    ))?)?)
}

fn save_manifest(session_dir: &Path, manifest: &SpoolManifest) -> anyhow::Result<()> {
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
pub(crate) fn begin_audio_spool_session(
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
pub(crate) fn persist_audio_spool_recovery_wav(
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

fn record_spool_chunk_inner(
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

pub(crate) fn record_audio_spool_chunk_status(
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

pub(crate) fn mark_audio_spool_status(
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

fn read_recovery_samples(
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

fn write_spool_session_file(
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

fn rebuild_recovery_wav(
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

fn read_existing_recovery_wav(session_dir: &Path) -> anyhow::Result<RecoveredSpool> {
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

fn is_safe_secret_sidecar_slot(slot: &str) -> bool {
    !slot.is_empty()
        && slot.len() <= 64
        && slot
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

fn secret_sidecar_slot(slot: &str) -> Option<&str> {
    match slot {
        "api_key" | "groq_api_key" => Some("api_key"),
        "asr_api_key" => Some("asr_api_key"),
        "cleanup_api_key" => Some("cleanup_api_key"),
        other if is_safe_secret_sidecar_slot(other) => Some(other),
        _ => None,
    }
}

fn secret_sidecar_path(dir: &Path, slot: &str) -> PathBuf {
    let slot = secret_sidecar_slot(slot).unwrap_or("api_key");
    dir.join("secrets").join(slot)
}

#[cfg(test)]
fn write_secret_sidecar(dir: &Path, slot: &str, key: &str) -> anyhow::Result<()> {
    let slot = secret_sidecar_slot(slot)
        .ok_or_else(|| anyhow::anyhow!("unsupported secret sidecar slot"))?;
    write_atomic_bytes(&secret_sidecar_path(dir, slot), key.as_bytes())
}

fn read_secret_sidecar(dir: &Path, slot: &str) -> Option<String> {
    let bytes = fs::read(secret_sidecar_path(dir, slot)).ok()?;
    let key = String::from_utf8(bytes).ok()?;
    (!key.trim().is_empty()).then_some(key)
}

fn clear_secret_sidecar(dir: &Path, slot: &str) {
    let _ = fs::remove_file(secret_sidecar_path(dir, slot));
}

pub fn clear_provider_key_sidecars(dir: &Path, provider: crate::engine::EngineProvider) {
    clear_secret_sidecar(dir, provider.keychain_account());
    if provider == crate::engine::EngineProvider::Groq {
        clear_secret_sidecar(dir, "api_key");
    }
    if provider == crate::engine::EngineProvider::Custom {
        clear_secret_sidecar(dir, "asr_api_key");
        clear_secret_sidecar(dir, "cleanup_api_key");
    }
}

const CUSTOM_HOST_CHANGE_CREDENTIAL_SLOTS: [&str; 3] =
    ["provider_custom", "asr_api_key", "cleanup_api_key"];

/// Remove the Custom credential and its legacy aliases before a keyless host
/// change can be persisted. The Custom read API falls back to both legacy
/// slots, so verify it only after every slot has been cleared.
pub(crate) fn clear_custom_credentials_before_host_change(dir: &Path) -> anyhow::Result<()> {
    clear_custom_credentials_before_host_change_with_backend(dir, &OsCredentialBackend)
}

fn clear_custom_credentials_before_host_change_with_backend(
    dir: &Path,
    backend: &dyn CredentialBackend,
) -> anyhow::Result<()> {
    let mut failure = None;
    for slot in CUSTOM_HOST_CHANGE_CREDENTIAL_SLOTS {
        if let Err(error) = backend.write(slot, "") {
            failure.get_or_insert_with(|| {
                anyhow::anyhow!("credential_storage: failed to clear custom {slot}: {error}")
            });
        }
    }

    for slot in CUSTOM_HOST_CHANGE_CREDENTIAL_SLOTS {
        match backend.read(slot) {
            crate::keychain::ApiKeyState::Missing => {}
            crate::keychain::ApiKeyState::Configured(_) => {
                failure.get_or_insert_with(|| {
                    anyhow::anyhow!("credential_storage: custom {slot} remains after clearing")
                });
            }
            crate::keychain::ApiKeyState::Unavailable(error) => {
                failure.get_or_insert_with(|| {
                    anyhow::anyhow!(
                        "credential_storage: cannot verify cleared custom {slot}: {error}"
                    )
                });
            }
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }

    let sidecar_slots = ["provider_custom", "asr_api_key", "cleanup_api_key"];
    for slot in sidecar_slots {
        remove_secret_sidecar_for_host_change(dir, slot)?;
    }
    for slot in sidecar_slots {
        verify_secret_sidecar_absent(dir, slot)?;
    }
    Ok(())
}

fn remove_secret_sidecar_for_host_change(dir: &Path, slot: &str) -> anyhow::Result<()> {
    let path = secret_sidecar_path(dir, slot);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(anyhow::anyhow!(
            "credential_storage: failed to remove custom {slot} sidecar: {error}"
        )),
    }
}

fn verify_secret_sidecar_absent(dir: &Path, slot: &str) -> anyhow::Result<()> {
    let path = secret_sidecar_path(dir, slot);
    match fs::symlink_metadata(&path) {
        Ok(_) => Err(anyhow::anyhow!(
            "credential_storage: custom {slot} sidecar remains after clearing"
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(anyhow::anyhow!(
            "credential_storage: cannot verify cleared custom {slot} sidecar: {error}"
        )),
    }
}

pub fn clear_asr_key_sidecar(dir: &Path) {
    clear_secret_sidecar(dir, "asr_api_key");
}

pub fn clear_cleanup_key_sidecar(dir: &Path) {
    clear_secret_sidecar(dir, "cleanup_api_key");
}

trait CredentialBackend {
    fn read(&self, slot: &str) -> crate::keychain::ApiKeyState;
    fn write(&self, slot: &str, key: &str) -> Result<(), String>;
}

struct OsCredentialBackend;

impl CredentialBackend for OsCredentialBackend {
    fn read(&self, slot: &str) -> crate::keychain::ApiKeyState {
        use crate::engine::EngineProvider;
        match slot {
            "api_key" | "groq_api_key" => crate::keychain::get_api_key_state(),
            "asr_api_key" => crate::keychain::get_asr_api_key_state(),
            "cleanup_api_key" => crate::keychain::get_cleanup_api_key_state(),
            account => EngineProvider::ALL
                .into_iter()
                .find(|provider| provider.keychain_account() == account)
                .map(crate::keychain::get_provider_api_key_state)
                .unwrap_or_else(|| {
                    crate::keychain::ApiKeyState::Unavailable(format!(
                        "unsupported credential account {account}"
                    ))
                }),
        }
    }

    fn write(&self, slot: &str, key: &str) -> Result<(), String> {
        use crate::engine::EngineProvider;
        match slot {
            "api_key" | "groq_api_key" => crate::keychain::set_api_key(key),
            "asr_api_key" => crate::keychain::set_asr_api_key(key),
            "cleanup_api_key" => crate::keychain::set_cleanup_api_key(key),
            account => EngineProvider::ALL
                .into_iter()
                .find(|provider| provider.keychain_account() == account)
                .ok_or_else(|| format!("unsupported credential account {account}"))
                .and_then(|provider| crate::keychain::set_provider_api_key(provider, key)),
        }
    }
}

fn clear_secret_sidecar_if_matches(dir: &Path, slot: &str, key: &str) {
    if read_secret_sidecar(dir, slot).as_deref() == Some(key) {
        clear_secret_sidecar(dir, slot);
    }
}

/// Persist to the OS credential store and confirm the exact value can be read
/// back before treating the operation as durable. `preserve_existing` is used
/// for legacy migration: it prevents a stale source from replacing a secure
/// credential that already exists or could not be read.
fn persist_secret_to_secure_store(
    dir: &Path,
    slot: &str,
    key: &str,
    backend: &dyn CredentialBackend,
    preserve_existing: bool,
    clear_replaced_sidecar: bool,
) -> anyhow::Result<()> {
    let before = backend.read(slot);
    match &before {
        crate::keychain::ApiKeyState::Configured(stored) if stored == key => {
            if clear_replaced_sidecar {
                clear_secret_sidecar(dir, slot);
            } else {
                clear_secret_sidecar_if_matches(dir, slot, key);
            }
            return Ok(());
        }
        crate::keychain::ApiKeyState::Configured(_) if preserve_existing => {
            anyhow::bail!("credential_storage: secure {slot} credential changed during migration")
        }
        crate::keychain::ApiKeyState::Unavailable(error)
            if preserve_existing || !clear_replaced_sidecar =>
        {
            anyhow::bail!("credential_storage: cannot verify existing {slot} credential: {error}")
        }
        crate::keychain::ApiKeyState::Missing
        | crate::keychain::ApiKeyState::Configured(_)
        | crate::keychain::ApiKeyState::Unavailable(_) => {}
    }

    backend.write(slot, key).map_err(|error| {
        anyhow::anyhow!("credential_storage: failed to store {slot} securely: {error}")
    })?;
    match backend.read(slot) {
        crate::keychain::ApiKeyState::Configured(stored) if stored == key => {
            if clear_replaced_sidecar {
                clear_secret_sidecar(dir, slot);
            } else {
                clear_secret_sidecar_if_matches(dir, slot, key);
            }
            Ok(())
        }
        crate::keychain::ApiKeyState::Configured(_) => {
            anyhow::bail!("credential_storage: secure {slot} write verification failed")
        }
        crate::keychain::ApiKeyState::Missing => {
            anyhow::bail!("credential_storage: secure {slot} write was not readable")
        }
        crate::keychain::ApiKeyState::Unavailable(error) => {
            anyhow::bail!("credential_storage: cannot verify secure {slot} write: {error}")
        }
    }
}

/// Adopt a value read directly from the active Groq Keychain namespace into
/// this process only. The caller never persists this Settings snapshot.
pub(crate) fn adopt_interactive_groq_credential(settings: &mut Settings, key: String) {
    settings.api_key = key.clone();
    settings
        .provider_api_keys
        .insert("groq".into(), key.clone());
    settings
        .credential_baselines
        .insert("groq_api_key".into(), key);
    settings
        .unverified_credential_sources
        .remove("settings:api_key");
    settings
        .unverified_credential_sources
        .remove("sidecar:groq_api_key");
    settings
        .credential_read_states
        .insert("groq_api_key".into(), CredentialReadState::Configured);
}

/// Adopt a verified Custom credential into this process only. The caller
/// restricts reads to currently active Custom purposes sharing the captured
/// origin; this function deliberately does not write settings or Keychain.
pub(crate) fn adopt_interactive_custom_credential(
    settings: &mut Settings,
    key: String,
    scope: CustomCredentialRecoveryScope,
) {
    settings
        .provider_api_keys
        .insert("custom".into(), key.clone());
    settings
        .credential_baselines
        .insert("provider_custom".into(), key);
    settings
        .unverified_credential_sources
        .remove("settings:provider:custom");
    settings
        .unverified_credential_sources
        .remove("sidecar:provider_custom");
    settings
        .credential_read_states
        .insert("provider_custom".into(), CredentialReadState::Configured);
    settings.session_only_custom_credential = true;
    settings.session_custom_credential_scope = Some(scope);
    settings.session_custom_credential_rejected = false;
}

fn credential_source_slot(source: &str) -> Option<String> {
    if let Some(field) = source.strip_prefix("settings:") {
        return match field {
            "api_key" => Some("groq_api_key".into()),
            "asr_api_key" => Some("asr_api_key".into()),
            "cleanup_api_key" => Some("cleanup_api_key".into()),
            provider_field if provider_field.starts_with("provider:") => {
                let id = provider_field.strip_prefix("provider:")?;
                let provider = crate::providers::EngineProvider::parse(id)?;
                Some(provider.keychain_account().into())
            }
            _ => None,
        };
    }
    let sidecar = source.strip_prefix("sidecar:")?;
    let normalized = secret_sidecar_slot(sidecar)?;
    if normalized == "api_key" {
        Some("groq_api_key".into())
    } else {
        Some(normalized.to_owned())
    }
}

fn has_unverified_source(settings: &Settings, slot: &str, key: &str) -> bool {
    settings
        .unverified_credential_sources
        .iter()
        .any(|(source, value)| {
            if value != key {
                return false;
            }
            let Some(source_slot) = credential_source_slot(source) else {
                return false;
            };
            source_slot == slot
                || (slot == crate::engine::EngineProvider::Custom.keychain_account()
                    && matches!(source_slot.as_str(), "asr_api_key" | "cleanup_api_key"))
        })
}

fn is_explicit_credential_replacement(settings: &Settings, slot: &str, key: &str) -> bool {
    if has_unverified_source(settings, slot, key) {
        return false;
    }
    settings
        .credential_baselines
        .get(slot)
        .is_none_or(|previous| previous != key)
}

struct CredentialResolution {
    value: String,
    state: crate::keychain::ApiKeyState,
    settings_source_verified: bool,
}

fn credential_read_state_from_resolution(
    state: &crate::keychain::ApiKeyState,
) -> CredentialReadState {
    match state {
        crate::keychain::ApiKeyState::Configured(_) => CredentialReadState::Configured,
        crate::keychain::ApiKeyState::Missing => CredentialReadState::Missing,
        crate::keychain::ApiKeyState::Unavailable(_) => CredentialReadState::Unavailable,
    }
}

fn resolve_legacy_credential(
    dir: &Path,
    slot: &str,
    legacy_value: &str,
    settings_source: Option<&str>,
    settings: &mut Settings,
    backend: &dyn CredentialBackend,
) -> CredentialResolution {
    let has_legacy_value = !legacy_value.trim().is_empty();
    let sidecar_value = read_secret_sidecar(dir, slot);
    let (candidate, source) = if has_legacy_value {
        (
            Some(legacy_value.to_owned()),
            settings_source.map(str::to_owned),
        )
    } else if let Some(value) = sidecar_value {
        (Some(value), Some(format!("sidecar:{slot}")))
    } else {
        (None, None)
    };

    match backend.read(slot) {
        crate::keychain::ApiKeyState::Configured(stored) => {
            settings
                .credential_baselines
                .insert(slot.to_owned(), stored.clone());
            if let Some(value) = candidate.as_deref() {
                if value == stored {
                    clear_secret_sidecar_if_matches(dir, slot, value);
                }
            }
            CredentialResolution {
                value: stored.clone(),
                state: crate::keychain::ApiKeyState::Configured(stored),
                settings_source_verified: settings_source.is_some() && has_legacy_value,
            }
        }
        crate::keychain::ApiKeyState::Unavailable(error) => {
            if let (Some(value), Some(source)) = (candidate, source) {
                settings
                    .unverified_credential_sources
                    .insert(source, value.clone());
                CredentialResolution {
                    value,
                    state: crate::keychain::ApiKeyState::Unavailable(error),
                    settings_source_verified: false,
                }
            } else {
                CredentialResolution {
                    value: String::new(),
                    state: crate::keychain::ApiKeyState::Unavailable(error),
                    settings_source_verified: false,
                }
            }
        }
        crate::keychain::ApiKeyState::Missing => {
            let Some(value) = candidate else {
                return CredentialResolution {
                    value: String::new(),
                    state: crate::keychain::ApiKeyState::Missing,
                    settings_source_verified: false,
                };
            };
            match persist_secret_to_secure_store(dir, slot, &value, backend, true, false) {
                Ok(()) => {
                    settings
                        .credential_baselines
                        .insert(slot.to_owned(), value.clone());
                    CredentialResolution {
                        value: value.clone(),
                        state: crate::keychain::ApiKeyState::Configured(value),
                        settings_source_verified: settings_source.is_some() && has_legacy_value,
                    }
                }
                Err(error) => {
                    let error = error.to_string();
                    if let Some(source) = source {
                        settings
                            .unverified_credential_sources
                            .insert(source, value.clone());
                    }
                    log::warn!("legacy credential migration for {slot} was not verified: {error}");
                    CredentialResolution {
                        value,
                        state: crate::keychain::ApiKeyState::Unavailable(error),
                        settings_source_verified: false,
                    }
                }
            }
        }
    }
}

/// Load settings from `settings.json`, then reconcile the API key with the
/// OS credential store. Returns the settings plus a flag indicating whether
/// the caller should persist the normalized result.
pub fn load_settings(dir: &Path) -> (Settings, bool) {
    load_settings_with_backend(dir, &OsCredentialBackend)
}

fn load_settings_with_backend(dir: &Path, backend: &dyn CredentialBackend) -> (Settings, bool) {
    let path = dir.join("settings.json");
    let raw = std::fs::read(&path).ok();
    let malformed = raw
        .as_deref()
        .is_some_and(|bytes| serde_json::from_slice::<Settings>(bytes).is_err());
    if malformed {
        log::error!("settings.json is malformed; keeping it untouched and starting with defaults");
    }
    let mut settings: Settings = raw
        .as_deref()
        .and_then(|bytes| serde_json::from_slice(bytes).ok())
        .unwrap_or_default();
    let schema_needs_persist = settings.schema_version < SETTINGS_SCHEMA_VERSION;
    let dictionary_sources_need_persist = dictionary_sources_need_normalization(&settings);
    let needs_backup = schema_needs_persist
        || dictionary_sources_need_persist
        || !settings.api_key.is_empty()
        || !settings.asr_api_key.is_empty()
        || !settings.cleanup_api_key.is_empty()
        || !settings.provider_api_keys.is_empty();
    if needs_backup {
        if let Some(raw) = raw.as_deref() {
            if let Err(error) = backup_legacy_settings(&path, raw) {
                log::warn!("failed to back up legacy settings before migration: {error}");
            }
        }
    }
    let hotkey_before = settings.hotkey.clone();
    let selected_action_hotkey_before = settings.selected_action_hotkey.clone();
    settings.normalize();
    let hotkeys_rewritten = settings.hotkey != hotkey_before
        || settings.selected_action_hotkey != selected_action_hotkey_before;

    let mut needs_persist =
        schema_needs_persist || dictionary_sources_need_persist || hotkeys_rewritten;
    let legacy_api_key = settings.api_key.clone();
    let api_resolution = resolve_legacy_credential(
        dir,
        "groq_api_key",
        &legacy_api_key,
        (!legacy_api_key.trim().is_empty()).then_some("settings:api_key"),
        &mut settings,
        backend,
    );
    settings.credential_read_states.insert(
        "groq_api_key".into(),
        credential_read_state_from_resolution(&api_resolution.state),
    );
    settings.api_key = api_resolution.value;
    if api_resolution.settings_source_verified {
        needs_persist = true;
    }
    let api_key_missing = matches!(api_resolution.state, crate::keychain::ApiKeyState::Missing);

    let legacy_asr_key = settings.asr_api_key.clone();
    let asr_resolution = resolve_legacy_credential(
        dir,
        "asr_api_key",
        &legacy_asr_key,
        (!legacy_asr_key.trim().is_empty()).then_some("settings:asr_api_key"),
        &mut settings,
        backend,
    );
    settings.credential_read_states.insert(
        "asr_api_key".into(),
        credential_read_state_from_resolution(&asr_resolution.state),
    );
    settings.asr_api_key = asr_resolution.value;
    if asr_resolution.settings_source_verified {
        needs_persist = true;
    }
    let asr_key_missing = matches!(asr_resolution.state, crate::keychain::ApiKeyState::Missing);

    let legacy_cleanup_key = settings.cleanup_api_key.clone();
    let cleanup_resolution = resolve_legacy_credential(
        dir,
        "cleanup_api_key",
        &legacy_cleanup_key,
        (!legacy_cleanup_key.trim().is_empty()).then_some("settings:cleanup_api_key"),
        &mut settings,
        backend,
    );
    settings.credential_read_states.insert(
        "cleanup_api_key".into(),
        credential_read_state_from_resolution(&cleanup_resolution.state),
    );
    settings.cleanup_api_key = cleanup_resolution.value;
    if cleanup_resolution.settings_source_verified {
        needs_persist = true;
    }
    let cleanup_key_missing = matches!(
        cleanup_resolution.state,
        crate::keychain::ApiKeyState::Missing
    );

    let mut provider_states = std::collections::HashMap::new();
    for provider in crate::providers::EngineProvider::ALL {
        let legacy_key = settings
            .provider_api_keys
            .get(provider.as_str())
            .cloned()
            .unwrap_or_default();
        let source = (!legacy_key.trim().is_empty())
            .then(|| format!("settings:provider:{}", provider.as_str()));
        let resolution = resolve_legacy_credential(
            dir,
            provider.keychain_account(),
            &legacy_key,
            source.as_deref(),
            &mut settings,
            backend,
        );
        settings.credential_read_states.insert(
            provider.keychain_account().to_owned(),
            credential_read_state_from_resolution(&resolution.state),
        );
        if resolution.settings_source_verified {
            needs_persist = true;
        }
        if !resolution.value.trim().is_empty() {
            settings
                .provider_api_keys
                .insert(provider.as_str().to_owned(), resolution.value);
        } else {
            settings.provider_api_keys.remove(provider.as_str());
        }
        provider_states.insert(provider, resolution.state);
    }
    bind_legacy_keys_into_pool(&mut settings);
    let selected_asr = settings.asr_provider;
    let selected_asr_key_missing = if selected_asr.is_groq() {
        api_key_missing
    } else {
        matches!(
            provider_states.get(&selected_asr),
            Some(crate::keychain::ApiKeyState::Missing)
        )
    };
    let selected_asr_url = settings.resolved_provider_base(selected_asr);
    let selected_asr_empty_ok = selected_asr == crate::engine::EngineProvider::OnDevice
        && settings.on_device_asr_ready(Some(&dir.join("models")))
        || selected_asr.allows_empty_key() && crate::providers::is_loopback_url(&selected_asr_url);
    if settings.onboarded
        && (selected_asr == crate::engine::EngineProvider::OnDevice
            && !settings.on_device_asr_ready(Some(&dir.join("models")))
            || selected_asr_key_missing
                && settings.asr_credential().trim().is_empty()
                && !selected_asr_empty_ok)
    {
        needs_persist = true;
        settings.onboarded = false;
    }
    // Only repair when the store confirmed the key is absent. A timeout must
    // not wipe a custom URL that still has a credential in the keychain.
    if asr_key_missing && settings.asr_api_key.trim().is_empty() && settings.repair_incomplete_asr()
    {
        needs_persist = true;
    }
    if cleanup_key_missing
        && settings.cleanup_api_key.trim().is_empty()
        && settings.repair_incomplete_cleanup()
    {
        needs_persist = true;
    }
    (settings, needs_persist)
}

fn bind_legacy_keys_into_pool(settings: &mut Settings) {
    if !settings.api_key.trim().is_empty() {
        settings
            .provider_api_keys
            .entry("groq".into())
            .or_insert_with(|| settings.api_key.clone());
    }
    if !settings.asr_provider.is_groq() && !settings.asr_api_key.trim().is_empty() {
        settings
            .provider_api_keys
            .entry(settings.asr_provider.as_str().to_owned())
            .or_insert_with(|| settings.asr_api_key.clone());
    }
    if !settings.cleanup_provider.is_groq() && !settings.cleanup_api_key.trim().is_empty() {
        settings
            .provider_api_keys
            .entry(settings.cleanup_provider.as_str().to_owned())
            .or_insert_with(|| settings.cleanup_api_key.clone());
    }
}

/// Keep a rollback point for settings migrations without copying a legacy API
/// key back onto disk. The backup is intentionally created only once so later
/// ordinary settings writes do not overwrite the original migration point.
fn backup_legacy_settings(path: &Path, raw: &[u8]) -> anyhow::Result<()> {
    let backup = path.with_file_name("settings.json.pre-migration.bak");
    if backup.exists() {
        return Ok(());
    }
    let mut value: serde_json::Value = serde_json::from_slice(raw)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("legacy settings must be a JSON object"))?;
    object.insert("api_key".into(), serde_json::Value::String(String::new()));
    object.insert(
        "asr_api_key".into(),
        serde_json::Value::String(String::new()),
    );
    object.insert(
        "cleanup_api_key".into(),
        serde_json::Value::String(String::new()),
    );
    object.insert(
        "provider_api_keys".into(),
        serde_json::Value::Object(serde_json::Map::new()),
    );
    let bytes = serde_json::to_vec_pretty(&value)?;
    write_atomic_bytes(&backup, &bytes)?;
    Ok(())
}

fn backup_malformed_settings_before_write(path: &Path) -> anyhow::Result<()> {
    let raw = match fs::read(path) {
        Ok(raw) if serde_json::from_slice::<Settings>(&raw).is_err() => raw,
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let backup = path.with_file_name("settings.json.malformed.bak");
    if backup.exists() {
        let metadata = fs::symlink_metadata(&backup)?;
        if !metadata.file_type().is_file() || fs::read(&backup)? != raw {
            anyhow::bail!("malformed settings backup already exists with different contents");
        }
        #[cfg(unix)]
        if metadata.permissions().mode() & 0o777 != 0o600 {
            anyhow::bail!("malformed settings backup has unexpected permissions");
        }
        return Ok(());
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&backup)?;
    file.write_all(&raw)?;
    file.sync_all()?;
    sync_directory(
        backup
            .parent()
            .ok_or_else(|| anyhow::anyhow!("invalid settings path"))?,
    )?;
    Ok(())
}
/// Persist settings to `settings.json`. API keys are written only to the OS
/// credential store and verified there before the settings file is updated.
pub fn save_settings(dir: &Path, settings: &Settings) -> anyhow::Result<()> {
    save_settings_with_backend(dir, settings, &OsCredentialBackend)
}

fn save_settings_with_backend(
    dir: &Path,
    settings: &Settings,
    backend: &dyn CredentialBackend,
) -> anyhow::Result<()> {
    save_settings_with_validation(dir, settings, backend, true)
}

/// Persist a structurally valid non-service settings change without accessing
/// credential stores. Verified credentials remain in their existing secure
/// store; deferred legacy settings values and sidecars remain their source of
/// truth until an explicit credential or service-route save can migrate them.
pub fn save_configuration(dir: &Path, settings: &Settings) -> anyhow::Result<()> {
    settings.validate_configuration()?;
    persist_settings_without_credential_changes(dir, settings)
}

fn persist_settings_without_credential_changes(
    dir: &Path,
    settings: &Settings,
) -> anyhow::Result<()> {
    let lock = SETTINGS_WRITE_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_private_dir(dir)?;
    backup_malformed_settings_before_write(&dir.join("settings.json"))?;

    let mut on_disk = settings.clone();
    on_disk.normalize_dictionary_sources();
    on_disk.api_key.clear();
    on_disk.asr_api_key.clear();
    on_disk.cleanup_api_key.clear();
    on_disk.provider_api_keys.clear();
    for (source, value) in &settings.unverified_credential_sources {
        let Some(field) = source.strip_prefix("settings:") else {
            // Sidecar sources remain in place; a preference save neither
            // reads nor removes their files.
            continue;
        };
        match field {
            "api_key" => on_disk.api_key.clone_from(value),
            "asr_api_key" => on_disk.asr_api_key.clone_from(value),
            "cleanup_api_key" => on_disk.cleanup_api_key.clone_from(value),
            provider_field if provider_field.starts_with("provider:") => {
                let provider = &provider_field["provider:".len()..];
                if crate::providers::EngineProvider::parse(provider).is_some() {
                    on_disk
                        .provider_api_keys
                        .insert(provider.to_owned(), value.clone());
                }
            }
            _ => {}
        }
    }

    let bytes = serde_json::to_vec_pretty(&on_disk)?;
    write_atomic_bytes(&dir.join("settings.json"), &bytes)?;
    Ok(())
}

fn save_settings_with_validation(
    dir: &Path,
    settings: &Settings,
    backend: &dyn CredentialBackend,
    check_readiness: bool,
) -> anyhow::Result<()> {
    settings.validate_internal(Some(&dir.join("models")), check_readiness)?;
    persist_settings_with_backend(dir, settings, backend)
}

/// Only the typed teaching fact can change through this entry. Do not validate
/// model availability/credentials as a prerequisite to acknowledging a guide.
pub(crate) fn acknowledge_guide(
    dir: &Path,
    current: &Settings,
    id: OnDemandGuideId,
    version: u32,
) -> anyhow::Result<Settings> {
    let mut next = current.clone();
    if next
        .guide_acknowledgements
        .acknowledge(id, version)
        .map_err(anyhow::Error::msg)?
    {
        persist_settings_without_credential_changes(dir, &next)?;
    }
    Ok(next)
}

fn persist_settings_with_backend(
    dir: &Path,
    settings: &Settings,
    backend: &dyn CredentialBackend,
) -> anyhow::Result<()> {
    let lock = SETTINGS_WRITE_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_private_dir(dir)?;
    backup_malformed_settings_before_write(&dir.join("settings.json"))?;
    // Persist every non-empty credential before writing settings.json. A
    // legacy value may remain in memory while its migration is pending; its
    // marker prevents a transient read failure from replacing a newer secure
    // value. On failure this function returns before modifying settings.json.
    if !settings.api_key.trim().is_empty() {
        persist_secret_to_secure_store(
            dir,
            "groq_api_key",
            &settings.api_key,
            backend,
            has_unverified_source(settings, "groq_api_key", &settings.api_key),
            is_explicit_credential_replacement(settings, "groq_api_key", &settings.api_key),
        )?;
    }
    if !settings.asr_api_key.trim().is_empty() {
        persist_secret_to_secure_store(
            dir,
            "asr_api_key",
            &settings.asr_api_key,
            backend,
            has_unverified_source(settings, "asr_api_key", &settings.asr_api_key),
            is_explicit_credential_replacement(settings, "asr_api_key", &settings.asr_api_key),
        )?;
    }
    if !settings.cleanup_api_key.trim().is_empty() {
        persist_secret_to_secure_store(
            dir,
            "cleanup_api_key",
            &settings.cleanup_api_key,
            backend,
            has_unverified_source(settings, "cleanup_api_key", &settings.cleanup_api_key),
            is_explicit_credential_replacement(
                settings,
                "cleanup_api_key",
                &settings.cleanup_api_key,
            ),
        )?;
    }
    for (id, key) in &settings.provider_api_keys {
        let Some(provider) = crate::providers::EngineProvider::parse(id) else {
            continue;
        };
        if key.trim().is_empty() {
            continue;
        }
        let slot = provider.keychain_account();
        if provider.is_custom() && settings.session_only_custom_credential {
            continue;
        }
        persist_secret_to_secure_store(
            dir,
            slot,
            key,
            backend,
            has_unverified_source(settings, slot, key),
            is_explicit_credential_replacement(settings, slot, key),
        )?;
    }
    let mut on_disk = settings.clone();
    on_disk.normalize_dictionary_sources();
    on_disk.api_key = String::new();
    on_disk.asr_api_key = String::new();
    on_disk.cleanup_api_key = String::new();
    on_disk.provider_api_keys.clear();
    let bytes = serde_json::to_vec_pretty(&on_disk)?;
    let path = dir.join("settings.json");
    write_atomic_bytes(&path, &bytes)?;
    Ok(())
}
static SETTINGS_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn schema(c: &Connection) -> anyhow::Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS dictations (
            id INTEGER PRIMARY KEY,
            created_at TEXT NOT NULL,
            duration_secs REAL,
            raw_text TEXT,
            final_text TEXT,
            cleanup_status TEXT,
            engine TEXT,
            degraded INTEGER,
            degraded_reason TEXT,
            status TEXT,
            raw_audio_path TEXT,
            delivery_method TEXT,
            fallback_reason TEXT,
            context_profile_id TEXT,
            context_policy_json TEXT
        );
        CREATE TABLE IF NOT EXISTS dictation_revisions (
            revision_id INTEGER PRIMARY KEY,
            dictation_id INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            final_text TEXT NOT NULL,
            cleanup_status TEXT,
            intent_json TEXT,
            model TEXT,
            context_policy_json TEXT,
            revision_reason TEXT NOT NULL,
            FOREIGN KEY(dictation_id) REFERENCES dictations(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_dictation_revisions_dictation
            ON dictation_revisions(dictation_id, revision_id DESC);
        CREATE TABLE IF NOT EXISTS learn_observations (
            observation_key TEXT PRIMARY KEY,
            source_kind TEXT NOT NULL,
            source_history_id INTEGER,
            source_revision_id INTEGER,
            created_at TEXT NOT NULL,
            rule_key TEXT,
            dictation_origin TEXT,
            channel TEXT,
            proof_eligible INTEGER NOT NULL DEFAULT 0,
            FOREIGN KEY(source_history_id) REFERENCES dictations(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_learn_observations_history
            ON learn_observations(source_history_id);
        CREATE TABLE IF NOT EXISTS usage (
            day TEXT PRIMARY KEY,
            asr_requests INTEGER NOT NULL DEFAULT 0,
            llm_requests INTEGER NOT NULL DEFAULT 0,
            audio_seconds REAL NOT NULL DEFAULT 0
        );",
    )?;
    ensure_column(c, "raw_audio_path", "TEXT")?;
    ensure_column(c, "degraded_reason", "TEXT")?;
    ensure_column(c, "delivery_method", "TEXT")?;
    ensure_column(c, "fallback_reason", "TEXT")?;
    ensure_column(c, "delivery_error_code", "TEXT")?;
    ensure_column(c, "delivery_user_reason", "TEXT")?;
    ensure_column(c, "context_profile_id", "TEXT")?;
    ensure_column(c, "context_policy_json", "TEXT")?;
    ensure_column(c, "context_family", "TEXT")?;
    ensure_column(c, "context_browser_host", "TEXT")?;
    ensure_column(c, "context_native_bundle", "TEXT")?;
    ensure_column(c, "cleanup_status", "TEXT")?;
    ensure_column(c, "verbatim_text", "TEXT")?;
    ensure_column(c, "verbatim_reviewed", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(c, "asr_text", "TEXT")?;
    ensure_column(c, "provider_cleaned_candidate", "TEXT")?;
    ensure_column(c, "cleanup_rejected_candidate", "TEXT")?;
    ensure_column(c, "cleanup_guard_reason", "TEXT")?;
    ensure_column(c, "cleanup_attempt_kind", "TEXT")?;
    ensure_column(c, "cleanup_transformations_json", "TEXT")?;
    ensure_column(c, "cleanup_final_guard_reason", "TEXT")?;
    ensure_column(c, "cleanup_failure_reason", "TEXT")?;
    ensure_column(c, "transaction_id", "TEXT")?;
    ensure_table_column(c, "learn_observations", "rule_key", "TEXT")?;
    ensure_table_column(c, "learn_observations", "dictation_origin", "TEXT")?;
    ensure_table_column(c, "learn_observations", "channel", "TEXT")?;
    ensure_table_column(
        c,
        "learn_observations",
        "proof_eligible",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    c.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_learn_observations_origin
         ON learn_observations(dictation_origin);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_learn_observations_independent_proof
         ON learn_observations(rule_key, dictation_origin)
         WHERE proof_eligible = 1 AND rule_key IS NOT NULL AND dictation_origin IS NOT NULL;",
    )?;
    c.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_dictations_transaction_id
         ON dictations(transaction_id) WHERE transaction_id IS NOT NULL;",
    )?;
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS learn_pairs (
            pair_key TEXT PRIMARY KEY,
            before_surface TEXT NOT NULL,
            after_surface TEXT NOT NULL,
            hits INTEGER NOT NULL,
            promoted INTEGER NOT NULL DEFAULT 0,
            last_at TEXT NOT NULL,
            dictionary_pending_action TEXT NOT NULL DEFAULT 'none',
            dictionary_effect_version INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS learned_term_usage (
            word TEXT PRIMARY KEY,
            replacement_runs INTEGER NOT NULL,
            last_replaced_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS style_drafts (
            draft_key TEXT PRIMARY KEY,
            mapping_id TEXT NOT NULL,
            style_key TEXT NOT NULL,
            excerpt TEXT NOT NULL,
            before_excerpt TEXT NOT NULL,
            after_excerpt TEXT NOT NULL,
            created_at TEXT NOT NULL
        );",
    )?;
    ensure_table_column(c, "learn_pairs", "family", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "mapping_id", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "browser_host", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "native_bundle", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "last_used_at", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "pinned", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_table_column(c, "learn_pairs", "tombstoned_at", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "ignored", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_table_column(
        c,
        "learn_pairs",
        "manual_scoped",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    // Automatically learned scene pairs are separately marked from pairs the
    // user confirmed manually. The default keeps all legacy rows inert unless
    // their target also remains in the global dictionary.
    ensure_table_column(
        c,
        "learn_pairs",
        "auto_scoped",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_table_column(
        c,
        "learn_pairs",
        "dictionary_pending_action",
        "TEXT NOT NULL DEFAULT 'none'",
    )?;
    ensure_table_column(
        c,
        "learn_pairs",
        "dictionary_effect_version",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    c.pragma_update(None, "user_version", HISTORY_SCHEMA_VERSION)?;
    Ok(())
}

fn open_history(dir: &Path) -> anyhow::Result<Connection> {
    ensure_private_dir(dir)?;
    let path = dir.join("history.sqlite");
    let existed = path.exists();
    let connection = Connection::open(&path)?;
    restrict_file_mode(&path)?;
    connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA secure_delete = ON;")?;
    // History writes can overlap with a completion, recovery scan, or the
    // settings window loading its list. Let SQLite briefly wait for the
    // active writer instead of turning a transient lock into lost history.
    connection.busy_timeout(Duration::from_secs(2))?;

    let schema_lock = HISTORY_SCHEMA_LOCK.get_or_init(|| Mutex::new(()));
    let _schema_guard = schema_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_version: i32 =
        connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if previous_version > HISTORY_SCHEMA_VERSION {
        anyhow::bail!("history schema {previous_version} is newer than supported schema {HISTORY_SCHEMA_VERSION}");
    }
    if previous_version != HISTORY_SCHEMA_VERSION {
        if existed && previous_version < HISTORY_SCHEMA_VERSION {
            let backup = dir.join(format!("history.sqlite.v{previous_version}.bak"));
            match fs::symlink_metadata(&backup) {
                Ok(metadata) if metadata.file_type().is_file() => {}
                Ok(_) => anyhow::bail!("unsafe history migration backup path"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    create_history_migration_backup(&connection, dir, &backup)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        // Opening the already-current schema is read-only. Reissuing the
        // CREATE/ensure statements on every History read takes a SQLite
        // schema writer lock and can block settings/preparation reads behind
        // an unrelated retained History transaction.
        connection.execute_batch("BEGIN IMMEDIATE")?;
        match schema(&connection) {
            Ok(()) => connection.execute_batch("COMMIT")?,
            Err(error) => {
                let _ = connection.execute_batch("ROLLBACK");
                return Err(error);
            }
        }
    }
    // Family labels identify broad app classes, not an individual target. Keep
    // legacy family-only promoted pairs inert; query first so normal History
    // opens stay read-only when no rows need the one-way policy migration.
    demote_family_only_promoted_learn_pairs(&connection)?;
    restrict_history_sidecars(dir)?;
    prune_history_migration_backups(
        dir,
        Some(HISTORY_MIGRATION_BACKUP_MAX_DAYS),
        SystemTime::now(),
    )?;
    Ok(connection)
}

fn create_history_migration_backup(
    connection: &Connection,
    dir: &Path,
    backup: &Path,
) -> anyhow::Result<()> {
    use std::sync::atomic::AtomicU64;
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    let temp_dir = dir.join(format!(
        ".history-backup-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    create_history_backup_temp_dir(&temp_dir)?;
    let result = (|| -> anyhow::Result<()> {
        let temporary_backup = temp_dir.join("history.sqlite");
        connection.execute(
            "VACUUM INTO ?1",
            [temporary_backup.to_string_lossy().as_ref()],
        )?;
        restrict_file_mode(&temporary_backup)?;
        let validation = Connection::open_with_flags(
            &temporary_backup,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let integrity: String =
            validation.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            anyhow::bail!("migration backup integrity check failed: {integrity}");
        }
        drop(validation);
        // A same-volume hard link atomically publishes the complete, checked
        // database and fails if any file or symlink already occupies the name.
        fs::hard_link(&temporary_backup, backup)?;
        sync_directory(dir)?;
        Ok(())
    })();
    let cleanup_result = fs::remove_dir_all(&temp_dir);
    result?;
    cleanup_result?;
    Ok(())
}

fn create_history_backup_temp_dir(temp_dir: &Path) -> anyhow::Result<()> {
    let mut owns_temp_dir = false;
    let setup_result = (|| -> anyhow::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, MetadataExt};
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(temp_dir)?;
            owns_temp_dir = true;
            fs::set_permissions(temp_dir, fs::Permissions::from_mode(0o700))?;
            if fs::symlink_metadata(temp_dir)?.uid() != unsafe { libc::geteuid() } {
                anyhow::bail!("migration backup temporary directory is not owned by this user");
            }
        }
        #[cfg(not(unix))]
        {
            fs::create_dir(&temp_dir)?;
            owns_temp_dir = true;
        }
        Ok(())
    })();
    if let Err(error) = setup_result {
        if owns_temp_dir {
            let _ = fs::remove_dir_all(temp_dir);
        }
        return Err(error);
    }
    Ok(())
}

fn demote_family_only_promoted_learn_pairs(connection: &Connection) -> anyhow::Result<()> {
    let has_legacy_rows: bool = connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM learn_pairs
            WHERE promoted = 1
              AND family IS NOT NULL
              AND family <> ''
              AND COALESCE(mapping_id, '') = ''
              AND COALESCE(browser_host, '') = ''
              AND COALESCE(native_bundle, '') = ''
        )",
        [],
        |row| row.get(0),
    )?;
    if has_legacy_rows {
        connection.execute(
            "UPDATE learn_pairs SET promoted = 0
             WHERE promoted = 1
               AND family IS NOT NULL
               AND family <> ''
               AND COALESCE(mapping_id, '') = ''
               AND COALESCE(browser_host, '') = ''
               AND COALESCE(native_bundle, '') = ''",
            [],
        )?;
    }
    Ok(())
}

/// Only names generated by open_history are owned rollback copies. Do not
/// recurse or follow links; user exports, settings and model files are outside
/// this cleanup boundary. None means the user explicitly cleared all data.
fn prune_history_migration_backups(
    dir: &Path,
    keep_days: Option<u64>,
    now: SystemTime,
) -> anyhow::Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(version) = name
            .to_str()
            .and_then(|name| name.strip_prefix("history.sqlite.v"))
            .and_then(|name| name.strip_suffix(".bak"))
        else {
            continue;
        };
        if !version
            .parse::<u32>()
            .is_ok_and(|value| value.to_string() == version)
        {
            continue;
        }
        let metadata = match fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        let expired = match keep_days {
            None => true,
            Some(days) => now
                .duration_since(metadata.modified()?)
                .is_ok_and(|age| age >= Duration::from_secs(days.saturating_mul(86_400))),
        };
        if expired {
            match fs::remove_file(entry.path()) {
                Ok(()) => removed += 1,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(removed)
}

static HISTORY_SCHEMA_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn ensure_column(c: &Connection, name: &str, definition: &str) -> anyhow::Result<()> {
    ensure_table_column(c, "dictations", name, definition)
}

fn ensure_table_column(
    c: &Connection,
    table: &str,
    name: &str,
    definition: &str,
) -> anyhow::Result<()> {
    let mut statement = c.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|column| column == name);
    if !exists {
        c.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {name} {definition}"),
            [],
        )?;
    }
    Ok(())
}
pub struct InsertHistory<'a> {
    pub dir: &'a Path,
    pub raw: &'a str,
    pub asr_text: Option<&'a str>,
    pub engine: Option<&'a str>,
    pub final_text: &'a str,
    pub cleanup_status: Option<&'a str>,
    pub duration: f64,
    pub degraded: bool,
    pub degraded_reason: Option<&'a str>,
    pub status: String,
    pub delivery_method: Option<&'a str>,
    pub fallback_reason: Option<&'a str>,
    pub spool: Option<&'a Path>,
    pub count_asr: bool,
    pub count_llm: bool,
    pub context: Option<&'a crate::context::ContextSnapshot>,
    pub cleanup_attempt_kind: Option<&'a str>,
    pub cleanup_transformations_json: Option<&'a str>,
    pub cleanup_final_guard_reason: Option<&'a str>,
    pub cleanup_failure_reason: Option<&'a str>,
}

/// Durable, content-bearing History intent written before a paste/copy side
/// effect. It contains only fields that the normal History insert already
/// persists; ContextPolicy must be pre-filtered with `history_metadata`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingHistoryIntent {
    pub transaction_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_spool_session_id: Option<String>,
    pub created_at_unix_ms: i64,
    pub raw_text: String,
    pub asr_text: Option<String>,
    pub provider_cleaned_candidate: Option<String>,
    pub final_text: String,
    pub duration_secs: f64,
    pub degraded: bool,
    pub degraded_reason: Option<String>,
    pub status: String,
    pub delivery_method: String,
    pub fallback_reason: Option<String>,
    pub context_profile_id: Option<String>,
    pub context_policy_json: Option<String>,
    pub context_family: Option<String>,
    pub context_browser_host: Option<String>,
    pub context_native_bundle: Option<String>,
    pub spool_path: Option<PathBuf>,
    pub engine: String,
    pub cleanup_status: String,
    pub diagnostic_code: Option<String>,
    pub diagnostic_user_reason: Option<String>,
    pub cleanup_rejected_candidate: Option<String>,
    pub cleanup_guard_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_attempt_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_transformations: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_final_guard_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_failure_reason: Option<String>,
}

impl PendingHistoryIntent {
    fn validate(&self) -> anyhow::Result<()> {
        if self.transaction_id.is_empty()
            || self.transaction_id.len() > 96
            || !self
                .transaction_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            anyhow::bail!("invalid pending History transaction identity");
        }
        if self
            .capture_spool_session_id
            .as_deref()
            .is_some_and(|session_id| !valid_spool_session_id(session_id))
        {
            anyhow::bail!("invalid pending History capture spool identity");
        }
        if !matches!(
            self.status.as_str(),
            "delivery_pending"
                | "delivery_cancelled"
                | "ok"
                | "degraded"
                | "history"
                | "copied"
                | "unverified"
                | "failed"
        ) {
            anyhow::bail!("invalid pending History delivery status");
        }
        if self.final_text.trim().is_empty()
            || !self.duration_secs.is_finite()
            || self.duration_secs < 0.0
            || self.status.len() > 128
            || self.delivery_method.len() > 64
            || self
                .cleanup_failure_reason
                .as_deref()
                .is_some_and(|reason| {
                    reason.len() > 96
                        || reason.is_empty()
                        || !reason.bytes().all(|byte| {
                            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                        })
                })
        {
            anyhow::bail!("invalid pending History payload");
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn pending_history_intent_for_test(
    transaction_id: &str,
    status: &str,
    delivery_method: &str,
    text: &str,
) -> PendingHistoryIntent {
    PendingHistoryIntent {
        transaction_id: transaction_id.to_owned(),
        capture_spool_session_id: None,
        created_at_unix_ms: now_ms() as i64,
        raw_text: text.to_owned(),
        asr_text: Some(text.to_owned()),
        provider_cleaned_candidate: None,
        final_text: text.to_owned(),
        duration_secs: 1.0,
        degraded: false,
        degraded_reason: None,
        status: status.to_owned(),
        delivery_method: delivery_method.to_owned(),
        fallback_reason: None,
        context_profile_id: None,
        context_policy_json: None,
        context_family: None,
        context_browser_host: None,
        context_native_bundle: None,
        spool_path: None,
        engine: "test-engine".into(),
        cleanup_status: "cleanup_not_requested".into(),
        diagnostic_code: None,
        diagnostic_user_reason: None,
        cleanup_rejected_candidate: None,
        cleanup_guard_reason: None,
        cleanup_attempt_kind: None,
        cleanup_transformations: None,
        cleanup_final_guard_reason: None,
        cleanup_failure_reason: None,
    }
}

fn pending_history_root(data_root: &Path) -> PathBuf {
    normalized_data_root(data_root).join("history-pending")
}

fn pending_history_path(data_root: &Path, transaction_id: &str) -> anyhow::Result<PathBuf> {
    if transaction_id.is_empty()
        || transaction_id.len() > 96
        || !transaction_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        anyhow::bail!("invalid pending History transaction identity");
    }
    Ok(pending_history_root(data_root)
        .join(transaction_id)
        .join("intent.json"))
}

fn pending_history_usage(root: &Path) -> anyhow::Result<(usize, u64)> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => anyhow::bail!("pending History directory is not a private directory"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(error) => return Err(error.into()),
    }
    let mut count = 0usize;
    let mut bytes = 0u64;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_dir() || file_type.is_symlink() {
            anyhow::bail!("pending History root contains an unowned entry");
        }
        let id = entry
            .file_name()
            .to_str()
            .map(str::to_owned)
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 96
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
            .ok_or_else(|| anyhow::anyhow!("pending History root contains an invalid identity"))?;
        let directory = entry.path();
        let mut files = 0usize;
        for child in fs::read_dir(&directory)? {
            let child = child?;
            let child_type = child.file_type()?;
            let name = child.file_name();
            if !child_type.is_file()
                || child_type.is_symlink()
                || !matches!(
                    name.to_str(),
                    Some("intent.json" | ".intent.pending" | ".delivery.cancelled")
                )
            {
                anyhow::bail!("pending History transaction contains an unowned entry");
            }
            if name == ".delivery.cancelled" && child.metadata()?.len() > 96 {
                anyhow::bail!("pending History cancellation marker is oversized");
            }
            files = files.saturating_add(1);
            bytes = bytes.saturating_add(child.metadata()?.len());
        }
        if files > 0 {
            count = count.saturating_add(1);
        }
        if files > 3 {
            anyhow::bail!("pending History transaction exceeds its file-count bound");
        }
        let expected = root.join(id);
        if normalized_data_root(&directory) != normalized_data_root(&expected) {
            anyhow::bail!("pending History identity directory does not match its name");
        }
    }
    Ok((count, bytes))
}

fn write_pending_history_atomically(
    data_root: &Path,
    path: &Path,
    bytes: &[u8],
) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid pending History path"))?;
    let expected_root = normalized_data_root(&data_root.join("history-pending"));
    if normalized_data_root(parent).parent() != Some(expected_root.as_path())
        || path.file_name().is_none_or(|name| name != "intent.json")
    {
        anyhow::bail!("pending History writer does not own this intent path");
    }
    ensure_private_dir(parent)?;
    let staged = parent.join(".intent.pending");
    let write_result = (|| -> anyhow::Result<()> {
        let mut file = File::create(&staged)?;
        restrict_file_mode(&staged)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        #[cfg(test)]
        wait_for_pending_history_write_test_gate(data_root);
        fs::rename(&staged, path)?;
        restrict_file_mode(path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if write_result.is_err() {
        // A complete staged file is recovery-owned evidence. Leave it in
        // place for startup reconciliation instead of deleting possibly
        // unique transcript content after an ambiguous rename failure.
        if fs::metadata(&staged).is_ok_and(|metadata| metadata.len() == 0) {
            let _ = fs::remove_file(&staged);
        }
        if fs::read_dir(parent)
            .ok()
            .is_some_and(|mut entries| entries.next().is_none())
        {
            let _ = fs::remove_dir(parent);
        }
    }
    write_result
}

fn recover_pending_history_staging(root: &Path) -> anyhow::Result<usize> {
    let (count, _) = pending_history_usage(root)?;
    if count == 0 {
        return Ok(0);
    }
    let mut recovered = 0usize;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let id = entry.file_name().to_str().map(str::to_owned).filter(|id| {
            !id.is_empty()
                && id.len() <= 96
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        });
        let Some(id) = id else {
            continue;
        };
        if pending_history_cancelled(root, &id)? {
            let transaction_dir = entry.path();
            fs::remove_dir_all(&transaction_dir)?;
            if let Some(parent) = transaction_dir.parent() {
                File::open(parent)?.sync_all()?;
            }
            continue;
        }
        let staged = entry.path().join(".intent.pending");
        let metadata = match fs::symlink_metadata(&staged) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if !metadata.file_type().is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > PENDING_HISTORY_JOB_MAX_BYTES as u64
        {
            anyhow::bail!("pending History staging file is not a bounded regular file");
        }
        let intent: PendingHistoryIntent = serde_json::from_slice(&fs::read(&staged)?)?;
        intent.validate()?;
        if intent.transaction_id != id {
            anyhow::bail!("pending History staging identity does not match its directory");
        }
        // `root` is already the history-pending directory, whereas
        // `pending_history_path` accepts the application data root. Keep the
        // recovery destination under the enumerated transaction directory.
        let final_path = entry.path().join("intent.json");
        fs::rename(&staged, &final_path)?;
        restrict_file_mode(&final_path)?;
        File::open(entry.path())?.sync_all()?;
        recovered = recovered.saturating_add(1);
    }
    Ok(recovered)
}

fn pending_history_cancelled(root: &Path, transaction_id: &str) -> anyhow::Result<bool> {
    let marker = root.join(transaction_id).join(".delivery.cancelled");
    match fs::symlink_metadata(&marker) {
        Ok(metadata)
            if metadata.file_type().is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() <= 96 =>
        {
            let recorded_id = fs::read_to_string(marker)?;
            if recorded_id != transaction_id {
                anyhow::bail!("pending History cancellation identity does not match its path");
            }
            Ok(true)
        }
        Ok(_) => anyhow::bail!("pending History cancellation marker is not a bounded file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn active_pending_history_origin(data_root: &Path, transaction_id: &str) -> anyhow::Result<bool> {
    let key = (normalized_data_root(data_root), transaction_id.to_owned());
    let active = PENDING_HISTORY_ACTIVE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(&key);
    if !active {
        return Ok(false);
    }
    let root = pending_history_root(data_root);
    if pending_history_cancelled(&root, transaction_id)? {
        return Ok(false);
    }
    let path = pending_history_path(data_root, transaction_id)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > PENDING_HISTORY_JOB_MAX_BYTES as u64
    {
        anyhow::bail!("pending History origin is not a bounded regular file");
    }
    let intent: PendingHistoryIntent = serde_json::from_slice(&fs::read(path)?)?;
    intent.validate()?;
    Ok(intent.transaction_id == transaction_id && intent.status != "delivery_cancelled")
}

fn discard_pending_dictation_proofs(data_root: &Path, transaction_id: &str) -> anyhow::Result<()> {
    if !data_root.join("history.sqlite").exists() {
        return Ok(());
    }
    let mut connection = open_history(data_root)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute(
        "DELETE FROM learn_observations
         WHERE dictation_origin = ?1
           AND proof_eligible = 1
           AND source_history_id IS NULL",
        [transaction_id],
    )?;
    transaction.commit()?;
    Ok(())
}

/// Persist one bounded recovery intent before any irreversible delivery
/// action. The shared pending-tree scope protects quota and enumeration while
/// remaining disjoint from History database commits and audio capture.
pub(crate) fn write_pending_history_intent(
    data_root: &Path,
    intent: &PendingHistoryIntent,
    owner: &PendingHistoryActiveLease,
) -> anyhow::Result<()> {
    owner.ensure_writable(data_root, &intent.transaction_id)?;
    intent.validate()?;
    let serialized = serde_json::to_vec(intent)?;
    if serialized.len() > PENDING_HISTORY_JOB_MAX_BYTES {
        anyhow::bail!("pending History payload exceeds the bounded size limit");
    }
    let _mutation = begin_spool_root_mutation(data_root)?;
    let _commit = try_spool_io_commit_pending_history(data_root, &intent.transaction_id)?;
    owner.ensure_writable(data_root, &intent.transaction_id)?;
    let root = pending_history_root(data_root);
    let path = pending_history_path(data_root, &intent.transaction_id)?;
    let budget_lock = PENDING_HISTORY_BUDGET_LOCK.get_or_init(|| Mutex::new(()));
    let _budget_guard = budget_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_private_dir(&root)?;
    let (count, bytes) = pending_history_usage(&root)?;
    if pending_history_cancelled(&root, &intent.transaction_id)? {
        anyhow::bail!("pending History identity was already cancelled");
    }
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => anyhow::bail!("pending History intent path is not a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let transaction_dir_exists = path.parent().is_some_and(Path::exists);
    let projected_count = count.saturating_add(usize::from(!transaction_dir_exists));
    // The old final and new staging files coexist until atomic rename, so
    // charge the complete write while it is in flight.
    let projected_bytes = bytes.saturating_add(serialized.len() as u64);
    if projected_count > PENDING_HISTORY_JOB_CAP
        || projected_bytes > PENDING_HISTORY_BYTE_CAP as u64
    {
        anyhow::bail!("pending History recovery capacity is full");
    }
    write_pending_history_atomically(data_root, &path, &serialized)?;
    File::open(&root)?.sync_all()?;
    Ok(())
}

pub(crate) fn remove_pending_history_intent(
    data_root: &Path,
    transaction_id: &str,
) -> anyhow::Result<()> {
    let (_mutation, _commit) =
        acquire_pending_history_mutation_and_commit(data_root, transaction_id)?;
    let budget_lock = PENDING_HISTORY_BUDGET_LOCK.get_or_init(|| Mutex::new(()));
    let _budget_guard = budget_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    remove_pending_history_intent_under_permit(data_root, transaction_id)
}

fn remove_pending_history_intent_under_permit(
    data_root: &Path,
    transaction_id: &str,
) -> anyhow::Result<()> {
    let path = pending_history_path(data_root, transaction_id)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(&path)?;
            if let Some(parent) = path.parent() {
                File::open(parent)?.sync_all()?;
                match fs::remove_dir(parent) {
                    Ok(()) => {
                        if let Some(root) = parent.parent() {
                            File::open(root)?.sync_all()?;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(())
        }
        Ok(_) => anyhow::bail!("pending History intent path is not a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Mark a known no-side-effect cancellation before attempting cleanup. If
/// removing the file then fails, startup recognizes the tombstone and will
/// discard it without creating a misleading History row.
pub(crate) fn cancel_pending_history_intent(
    data_root: &Path,
    transaction_id: &str,
    owner: &PendingHistoryActiveLease,
) -> anyhow::Result<()> {
    owner.poison_for_known_no_effect_cancel(data_root, transaction_id)?;
    let (_mutation, _commit) =
        acquire_pending_history_mutation_and_commit(data_root, transaction_id)?;
    let budget_lock = PENDING_HISTORY_BUDGET_LOCK.get_or_init(|| Mutex::new(()));
    let _budget_guard = budget_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = pending_history_root(data_root);
    let transaction_dir = root.join(transaction_id);
    ensure_private_dir(&transaction_dir)?;
    let marker = transaction_dir.join(".delivery.cancelled");
    match fs::symlink_metadata(&marker) {
        Ok(_) => {
            if !pending_history_cancelled(&root, transaction_id)? {
                anyhow::bail!("pending History cancellation marker is invalid");
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut file = File::create(&marker)?;
            restrict_file_mode(&marker)?;
            file.write_all(transaction_id.as_bytes())?;
            file.sync_all()?;
            File::open(&transaction_dir)?.sync_all()?;
        }
        Err(error) => return Err(error.into()),
    }
    discard_pending_dictation_proofs(data_root, transaction_id)?;
    let path = pending_history_path(data_root, transaction_id)?;
    let staged = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid pending History path"))?
        .join(".intent.pending");
    for candidate in [&path, &staged] {
        match fs::symlink_metadata(candidate) {
            Ok(metadata)
                if metadata.file_type().is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.len() <= PENDING_HISTORY_JOB_MAX_BYTES as u64 =>
            {
                if candidate == &staged {
                    let staged_intent: PendingHistoryIntent =
                        serde_json::from_slice(&fs::read(candidate)?)?;
                    staged_intent.validate()?;
                    if staged_intent.transaction_id != transaction_id {
                        anyhow::bail!("pending History staging identity does not match its path");
                    }
                } else {
                    let intent: PendingHistoryIntent =
                        serde_json::from_slice(&fs::read(candidate)?)?;
                    intent.validate()?;
                    if intent.transaction_id != transaction_id {
                        anyhow::bail!("pending History intent identity does not match its path");
                    }
                }
                fs::remove_file(candidate)?;
            }
            Ok(_) => anyhow::bail!("pending History intent cannot be safely cancelled"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let marker = transaction_dir.join(".delivery.cancelled");
    match fs::symlink_metadata(&marker) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(&marker)?;
        }
        Ok(_) => anyhow::bail!("pending History cancellation marker is invalid"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    File::open(&transaction_dir)?.sync_all()?;
    match fs::remove_dir(&transaction_dir) {
        Ok(()) => File::open(&root)?.sync_all()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

/// Read the exact text owned by one live History recovery ticket. The ticket
/// caller supplies only the stable transaction identity; the text is loaded
/// from that transaction's validated pending intent or matching History row.
pub(crate) fn history_recovery_text_for_transaction(
    data_root: &Path,
    transaction_id: &str,
) -> anyhow::Result<String> {
    let _mutation = begin_spool_root_mutation(data_root)?;
    let pending_root = pending_history_root(data_root);
    {
        let _pending_commit = try_spool_io_commit_pending_history(data_root, transaction_id)?;
        if pending_history_cancelled(&pending_root, transaction_id)? {
            anyhow::bail!("History recovery was cancelled");
        }
        let path = pending_history_path(data_root, transaction_id)?;
        match fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.file_type().is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.len() <= PENDING_HISTORY_JOB_MAX_BYTES as u64 =>
            {
                let intent: PendingHistoryIntent = serde_json::from_slice(&fs::read(path)?)?;
                intent.validate()?;
                if intent.transaction_id != transaction_id {
                    anyhow::bail!("pending History identity does not match its ticket");
                }
                if intent.final_text.trim().is_empty() {
                    anyhow::bail!("History recovery has no copyable text");
                }
                return Ok(intent.final_text);
            }
            Ok(_) => anyhow::bail!("pending History recovery is not a bounded regular file"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    let _history_commit = try_spool_io_commit_scope(data_root, SpoolIoScope::HistoryDatabase)?;
    let connection = open_history(data_root)?;
    let text = connection
        .query_row(
            "SELECT COALESCE(NULLIF((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),''),NULLIF(final_text,''),raw_text,'') FROM dictations WHERE transaction_id=?1",
            [transaction_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("History transaction has no copyable text"))?;
    Ok(text)
}

/// Promote a pending History intent exactly once. The transaction ID is a
/// unique database key, so a crash after SQLite commit but before deleting the
/// sidecar cannot create a duplicate on the next startup.
pub(crate) fn promote_pending_history_intent(
    data_root: &Path,
    transaction_id: &str,
    owner: &PendingHistoryActiveLease,
) -> anyhow::Result<()> {
    owner.ensure_writable(data_root, transaction_id)?;
    let _mutation = begin_spool_root_mutation(data_root)?;
    let _intent_commit = try_spool_io_commit_pending_history(data_root, transaction_id)?;
    owner.ensure_writable(data_root, transaction_id)?;
    let pending_root = pending_history_root(data_root);
    if pending_history_cancelled(&pending_root, transaction_id)? {
        discard_pending_dictation_proofs(data_root, transaction_id)?;
        let transaction_dir = pending_root.join(transaction_id);
        fs::remove_dir_all(&transaction_dir)?;
        File::open(&pending_root)?.sync_all()?;
        return Ok(());
    }
    let path = pending_history_path(data_root, transaction_id)?;
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        anyhow::bail!("pending History intent is not a regular file");
    }
    if metadata.len() > PENDING_HISTORY_JOB_MAX_BYTES as u64 {
        anyhow::bail!("pending History intent exceeds the bounded read size");
    }
    let intent: PendingHistoryIntent = serde_json::from_slice(&fs::read(&path)?)?;
    intent.validate()?;
    if intent.transaction_id != transaction_id {
        anyhow::bail!("pending History intent identity does not match its path");
    }
    if intent.status == "delivery_cancelled" {
        drop(_intent_commit);
        remove_pending_history_intent(data_root, transaction_id)?;
        return Ok(());
    }
    drop(_intent_commit);
    let _history_commit = try_spool_io_commit_scope(data_root, SpoolIoScope::HistoryDatabase)?;
    let mut connection = open_history(data_root)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let existing_id = transaction
        .query_row(
            "SELECT id FROM dictations WHERE transaction_id=?1",
            [&intent.transaction_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    if existing_id.is_none() {
        let interrupted = intent.status == "delivery_pending";
        let status = if interrupted {
            "delivery_unconfirmed"
        } else {
            intent.status.as_str()
        };
        let delivery_method = if interrupted {
            "unknown"
        } else {
            intent.delivery_method.as_str()
        };
        let fallback_reason = if interrupted {
            Some("delivery_state_unknown_after_restart")
        } else {
            intent.fallback_reason.as_deref()
        };
        let cleanup_transformations_json = intent
            .cleanup_transformations
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        insert_history_with_status_and_candidate_on_connection(
            &transaction,
            InsertHistory {
                dir: data_root,
                raw: &intent.raw_text,
                asr_text: intent.asr_text.as_deref(),
                engine: Some(&intent.engine),
                final_text: &intent.final_text,
                cleanup_status: Some(&intent.cleanup_status),
                duration: intent.duration_secs,
                degraded: intent.degraded,
                degraded_reason: intent.degraded_reason.as_deref(),
                status: status.to_owned(),
                delivery_method: Some(delivery_method),
                fallback_reason,
                spool: intent.spool_path.as_deref(),
                count_asr: true,
                count_llm: true,
                context: None,
                cleanup_attempt_kind: intent.cleanup_attempt_kind.as_deref(),
                cleanup_transformations_json: cleanup_transformations_json.as_deref(),
                cleanup_final_guard_reason: intent.cleanup_final_guard_reason.as_deref(),
                cleanup_failure_reason: intent.cleanup_failure_reason.as_deref(),
            },
            intent.provider_cleaned_candidate.as_deref(),
            None,
            intent.cleanup_rejected_candidate.as_deref(),
            intent.cleanup_guard_reason.as_deref(),
        )?;
        let inserted_id = transaction.last_insert_rowid();
        transaction.execute(
            "UPDATE dictations SET transaction_id=?1, created_at=datetime(?2 / 1000, 'unixepoch'), context_profile_id=?3, context_policy_json=?4, context_family=?5, context_browser_host=?6, context_native_bundle=?7, delivery_error_code=?8, delivery_user_reason=?9 WHERE id=?10",
            params![
                intent.transaction_id,
                intent.created_at_unix_ms,
                intent.context_profile_id,
                intent.context_policy_json,
                intent.context_family,
                intent.context_browser_host,
                intent.context_native_bundle,
                if interrupted { Some("delivery_unconfirmed_after_restart") } else { intent.diagnostic_code.as_deref() },
                if interrupted { Some("Delivery was interrupted before its final result was recorded.") } else { intent.diagnostic_user_reason.as_deref() },
                inserted_id
            ],
        )?;
    }
    let history_id = transaction.query_row(
        "SELECT id FROM dictations WHERE transaction_id = ?1",
        [&intent.transaction_id],
        |row| row.get::<_, i64>(0),
    )?;
    transaction.execute(
        "UPDATE learn_observations
         SET source_history_id = ?1
         WHERE dictation_origin = ?2
           AND proof_eligible = 1
           AND source_history_id IS NULL",
        params![history_id, intent.transaction_id],
    )?;
    #[cfg(test)]
    wait_for_history_promotion_test_gate(data_root);
    transaction.commit()?;
    drop(_history_commit);
    // The row commit is the durable ACK. Keep the intent until its capture
    // spool has been retained or removed; startup repeats this idempotently if
    // the process exits between the database commit and spool cleanup.
    let capture_spool_settled = match intent.capture_spool_session_id.as_deref() {
        Some(session_id) => settle_capture_spool_after_history_ack(
            data_root,
            session_id,
            intent.spool_path.as_deref(),
        )?,
        None => true,
    };
    if capture_spool_settled {
        let _ = remove_pending_history_intent(data_root, transaction_id);
    }
    Ok(())
}

pub(crate) fn promote_all_pending_history_intents(data_root: &Path) -> anyhow::Result<usize> {
    let root = pending_history_root(data_root);
    let intents = {
        let _mutation = begin_spool_root_mutation(data_root)?;
        let _commit = try_spool_io_commit_scope(data_root, SpoolIoScope::PendingHistoryIntent)?;
        recover_pending_history_staging(&root)?;
        let (count, _) = pending_history_usage(&root)?;
        if count == 0 {
            return Ok(0);
        }
        let mut intents = fs::read_dir(&root)?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<Result<Vec<_>, _>>()?;
        intents.sort();
        intents
    };
    let mut promoted = 0usize;
    let mut first_error: Option<anyhow::Error> = None;
    for transaction_id in intents {
        if !transaction_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || transaction_id.is_empty()
            || transaction_id.len() > 96
        {
            first_error.get_or_insert_with(|| {
                anyhow::anyhow!("pending History root contains an invalid identity")
            });
            continue;
        }
        let Some(active_owner) = reserve_pending_history_startup_owner(data_root, &transaction_id)
        else {
            // A live dictation still owns this immutable transaction. Startup
            // maintenance must not promote its pre-delivery status while the
            // native delivery result is still being decided.
            continue;
        };
        let inspected = (|| -> anyhow::Result<bool> {
            let _mutation = begin_spool_root_mutation(data_root)?;
            let _commit = try_spool_io_commit_pending_history(data_root, &transaction_id)?;
            if pending_history_cancelled(&root, &transaction_id)? {
                discard_pending_dictation_proofs(data_root, &transaction_id)?;
                let transaction_dir = root.join(&transaction_id);
                fs::remove_dir_all(&transaction_dir)?;
                File::open(&root)?.sync_all()?;
                return Ok(true);
            }
            let intent_path = pending_history_path(data_root, &transaction_id)?;
            let metadata = fs::symlink_metadata(&intent_path)?;
            if !metadata.file_type().is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > PENDING_HISTORY_JOB_MAX_BYTES as u64
            {
                anyhow::bail!("pending History intent is not a bounded regular file");
            }
            let intent: PendingHistoryIntent = serde_json::from_slice(&fs::read(intent_path)?)?;
            intent.validate()?;
            if intent.transaction_id != transaction_id {
                anyhow::bail!("pending History identity does not match its directory");
            }
            Ok(false)
        })();
        match inspected {
            Ok(true) => continue,
            Ok(false) => {}
            Err(error) => {
                first_error.get_or_insert(error);
                continue;
            }
        }
        match promote_pending_history_intent(data_root, &transaction_id, &active_owner) {
            Ok(()) => promoted = promoted.saturating_add(1),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(promoted),
    }
}

#[cfg(test)]
pub fn insert_history(
    dir: &Path,
    raw: &str,
    final_text: &str,
    duration: f64,
    degraded: bool,
) -> anyhow::Result<()> {
    insert_history_with_status(InsertHistory {
        dir,
        raw,
        asr_text: None,
        engine: None,
        final_text,
        cleanup_status: None,
        duration,
        degraded,
        degraded_reason: None,
        status: if degraded { "degraded" } else { "ok" }.to_owned(),
        delivery_method: Some("history"),
        fallback_reason: None,
        spool: None,
        count_asr: true,
        count_llm: true,
        context: None,
        cleanup_attempt_kind: None,
        cleanup_transformations_json: None,
        cleanup_final_guard_reason: None,
        cleanup_failure_reason: None,
    })
}

#[cfg(test)]
pub fn insert_history_with_context(
    dir: &Path,
    raw: &str,
    final_text: &str,
    duration: f64,
    degraded: bool,
    context: &crate::context::ContextSnapshot,
) -> anyhow::Result<()> {
    insert_history_with_delivery(
        dir,
        raw,
        final_text,
        duration,
        degraded,
        None,
        if degraded { "degraded" } else { "ok" },
        "history",
        None,
        context,
    )
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub fn insert_history_with_delivery(
    dir: &Path,
    raw: &str,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
) -> anyhow::Result<()> {
    insert_history_with_delivery_and_spool(
        dir,
        raw,
        final_text,
        duration,
        degraded,
        degraded_reason,
        status,
        delivery_method,
        fallback_reason,
        context,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub fn insert_history_with_delivery_and_spool(
    dir: &Path,
    raw: &str,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
    spool: Option<&Path>,
) -> anyhow::Result<()> {
    insert_history_with_status(InsertHistory {
        dir,
        raw,
        asr_text: None,
        engine: None,
        final_text,
        cleanup_status: None,
        duration,
        degraded,
        degraded_reason,
        status: status.to_owned(),
        delivery_method: Some(delivery_method),
        fallback_reason,
        spool,
        count_asr: true,
        count_llm: true,
        context: Some(context),
        cleanup_attempt_kind: None,
        cleanup_transformations_json: None,
        cleanup_final_guard_reason: None,
        cleanup_failure_reason: None,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn insert_history_with_asr_and_delivery_and_spool_and_cleanup(
    dir: &Path,
    raw: &str,
    asr_text: Option<&str>,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
    spool: Option<&Path>,
    engine: &str,
    cleanup_status: &str,
) -> anyhow::Result<()> {
    insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup(
        dir,
        raw,
        asr_text,
        None,
        final_text,
        duration,
        degraded,
        degraded_reason,
        status,
        delivery_method,
        fallback_reason,
        context,
        spool,
        engine,
        cleanup_status,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup(
    dir: &Path,
    raw: &str,
    asr_text: Option<&str>,
    provider_cleaned_candidate: Option<&str>,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
    spool: Option<&Path>,
    engine: &str,
    cleanup_status: &str,
) -> anyhow::Result<()> {
    insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup_diagnostic(
        dir,
        raw,
        asr_text,
        provider_cleaned_candidate,
        final_text,
        duration,
        degraded,
        degraded_reason,
        status,
        delivery_method,
        fallback_reason,
        context,
        spool,
        engine,
        cleanup_status,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup_diagnostic(
    dir: &Path,
    raw: &str,
    asr_text: Option<&str>,
    provider_cleaned_candidate: Option<&str>,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
    spool: Option<&Path>,
    engine: &str,
    cleanup_status: &str,
    diagnostic: Option<&crate::delivery_diagnostics::DeliveryDiagnostic>,
) -> anyhow::Result<()> {
    insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup_rejection(
        dir,
        raw,
        asr_text,
        provider_cleaned_candidate,
        final_text,
        duration,
        degraded,
        degraded_reason,
        status,
        delivery_method,
        fallback_reason,
        context,
        spool,
        engine,
        cleanup_status,
        diagnostic,
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup_rejection(
    dir: &Path,
    raw: &str,
    asr_text: Option<&str>,
    provider_cleaned_candidate: Option<&str>,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
    spool: Option<&Path>,
    engine: &str,
    cleanup_status: &str,
    diagnostic: Option<&crate::delivery_diagnostics::DeliveryDiagnostic>,
    cleanup_rejected_candidate: Option<&str>,
    cleanup_guard_reason: Option<&str>,
) -> anyhow::Result<()> {
    insert_history_with_status_and_candidate(
        InsertHistory {
            dir,
            raw,
            asr_text,
            engine: Some(engine),
            final_text,
            cleanup_status: Some(cleanup_status),
            duration,
            degraded,
            degraded_reason,
            status: status.to_owned(),
            delivery_method: Some(delivery_method),
            fallback_reason,
            spool,
            count_asr: true,
            count_llm: true,
            context: Some(context),
            cleanup_attempt_kind: None,
            cleanup_transformations_json: None,
            cleanup_final_guard_reason: None,
            cleanup_failure_reason: None,
        },
        provider_cleaned_candidate,
        diagnostic,
        cleanup_rejected_candidate,
        cleanup_guard_reason,
    )
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub fn insert_history_with_delivery_and_spool_and_cleanup(
    dir: &Path,
    raw: &str,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
    spool: Option<&Path>,
    cleanup_status: &str,
) -> anyhow::Result<()> {
    insert_history_with_asr_and_delivery_and_spool_and_cleanup(
        dir,
        raw,
        None,
        final_text,
        duration,
        degraded,
        degraded_reason,
        status,
        delivery_method,
        fallback_reason,
        context,
        spool,
        "unknown",
        cleanup_status,
    )
}
#[cfg(test)]
#[allow(dead_code)]
pub fn insert_failed_history(
    dir: &Path,
    raw: &str,
    duration: f64,
    spool: Option<&Path>,
) -> anyhow::Result<()> {
    insert_history_with_status(InsertHistory {
        dir,
        raw,
        asr_text: None,
        engine: None,
        final_text: "",
        cleanup_status: None,
        duration,
        degraded: false,
        degraded_reason: Some("asr_failed"),
        status: "failed".to_owned(),
        delivery_method: Some("none"),
        fallback_reason: Some("asr_failed"),
        spool,
        count_asr: false,
        count_llm: false,
        context: None,
        cleanup_attempt_kind: None,
        cleanup_transformations_json: None,
        cleanup_final_guard_reason: None,
        cleanup_failure_reason: None,
    })
}

pub fn insert_failed_history_with_context(
    dir: &Path,
    raw: &str,
    duration: f64,
    spool: Option<&Path>,
    context: &crate::context::ContextSnapshot,
    engine: &str,
) -> anyhow::Result<()> {
    insert_history_with_status(InsertHistory {
        dir,
        raw,
        asr_text: None,
        engine: Some(engine),
        final_text: "",
        cleanup_status: None,
        duration,
        degraded: false,
        degraded_reason: Some("asr_failed"),
        status: "failed".to_owned(),
        delivery_method: Some("none"),
        fallback_reason: Some("asr_failed"),
        spool,
        count_asr: false,
        count_llm: false,
        context: Some(context),
        cleanup_attempt_kind: None,
        cleanup_transformations_json: None,
        cleanup_final_guard_reason: None,
        cleanup_failure_reason: None,
    })
}

/// Insert one failed-ASR History row for a recovery WAV at most once. The
/// acknowledged path is the stable identity retained across a lost caller
/// reply; a repeated owner attempt cannot create a duplicate row.
pub fn insert_failed_history_with_context_once(
    dir: &Path,
    duration: f64,
    spool: &Path,
    context: &crate::context::ContextSnapshot,
    engine: &str,
) -> anyhow::Result<()> {
    let _history_commit = try_spool_io_commit_scope(dir, SpoolIoScope::HistoryDatabase)?;
    let audio_path = spool.to_string_lossy().into_owned();
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let already_recorded: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM dictations WHERE raw_audio_path=?",
        [&audio_path],
        |row| row.get(0),
    )?;
    if already_recorded > 0 {
        transaction.commit()?;
        return Ok(());
    }
    insert_history_with_status_and_candidate_on_connection(
        &transaction,
        InsertHistory {
            dir,
            raw: "",
            asr_text: None,
            engine: Some(engine),
            final_text: "",
            cleanup_status: None,
            duration,
            degraded: false,
            degraded_reason: Some("asr_failed"),
            status: "failed".to_owned(),
            delivery_method: Some("none"),
            fallback_reason: Some("asr_failed"),
            spool: Some(spool),
            count_asr: false,
            count_llm: false,
            context: Some(context),
            cleanup_attempt_kind: None,
            cleanup_transformations_json: None,
            cleanup_final_guard_reason: None,
            cleanup_failure_reason: None,
        },
        None,
        None,
        None,
        None,
    )?;
    transaction.commit()?;
    Ok(())
}

fn insert_history_with_status(input: InsertHistory) -> anyhow::Result<()> {
    insert_history_with_status_and_candidate(input, None, None, None, None)
}

fn insert_history_with_status_and_candidate(
    input: InsertHistory,
    provider_cleaned_candidate: Option<&str>,
    diagnostic: Option<&crate::delivery_diagnostics::DeliveryDiagnostic>,
    cleanup_rejected_candidate: Option<&str>,
    cleanup_guard_reason: Option<&str>,
) -> anyhow::Result<()> {
    let _history_commit = try_spool_io_commit_scope(input.dir, SpoolIoScope::HistoryDatabase)?;
    let c = open_history(input.dir)?;
    insert_history_with_status_and_candidate_on_connection(
        &c,
        input,
        provider_cleaned_candidate,
        diagnostic,
        cleanup_rejected_candidate,
        cleanup_guard_reason,
    )
}

fn insert_history_with_status_and_candidate_on_connection(
    c: &Connection,
    input: InsertHistory<'_>,
    provider_cleaned_candidate: Option<&str>,
    diagnostic: Option<&crate::delivery_diagnostics::DeliveryDiagnostic>,
    cleanup_rejected_candidate: Option<&str>,
    cleanup_guard_reason: Option<&str>,
) -> anyhow::Result<()> {
    let InsertHistory {
        dir: _,
        raw,
        asr_text,
        engine,
        final_text,
        cleanup_status,
        duration,
        degraded,
        degraded_reason,
        status,
        delivery_method,
        fallback_reason,
        spool,
        count_asr,
        count_llm,
        context,
        cleanup_attempt_kind,
        cleanup_transformations_json,
        cleanup_final_guard_reason,
        cleanup_failure_reason,
    } = input;
    let context_profile_id = context.map(|snapshot| snapshot.profile.id.as_str());
    let context_policy_json = context
        .map(|snapshot| serde_json::to_string(&snapshot.policy.history_metadata()))
        .transpose()?;
    let context_family =
        context.map(|snapshot| crate::context::family_id(snapshot.profile.family).to_owned());
    let context_browser_host =
        context.and_then(|snapshot| snapshot.target_guard.browser_host.clone());
    let context_native_bundle = context.and_then(|snapshot| {
        if snapshot.target_guard.browser_host.is_some() {
            None
        } else {
            snapshot.target_guard.bundle_id.clone()
        }
    });
    c.execute("INSERT INTO dictations (created_at,duration_secs,raw_text,final_text,cleanup_status,engine,degraded,degraded_reason,status,raw_audio_path,delivery_method,fallback_reason,context_profile_id,context_policy_json,context_family,context_browser_host,context_native_bundle,asr_text,provider_cleaned_candidate,cleanup_rejected_candidate,cleanup_guard_reason,delivery_error_code,delivery_user_reason,cleanup_attempt_kind,cleanup_transformations_json,cleanup_final_guard_reason,cleanup_failure_reason) VALUES (datetime('now'),?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)", params![duration, raw, final_text, cleanup_status, engine.unwrap_or("unknown"), degraded as i32, degraded_reason, status, spool.map(|p| p.to_string_lossy().to_string()), delivery_method, fallback_reason, context_profile_id, context_policy_json, context_family, context_browser_host, context_native_bundle, asr_text, provider_cleaned_candidate, cleanup_rejected_candidate, cleanup_guard_reason, diagnostic.map(|item| item.code), diagnostic.map(|item| item.user_reason), cleanup_attempt_kind, cleanup_transformations_json, cleanup_final_guard_reason, cleanup_failure_reason])?;
    if count_asr || count_llm {
        c.execute("INSERT INTO usage(day,asr_requests,llm_requests,audio_seconds) VALUES (date('now'),?,?,?) ON CONFLICT(day) DO UPDATE SET asr_requests=asr_requests+excluded.asr_requests,llm_requests=llm_requests+excluded.llm_requests,audio_seconds=audio_seconds+excluded.audio_seconds", params![count_asr as i64, count_llm as i64, duration])?;
    }
    Ok(())
}
#[cfg(test)]
pub fn get_usage(dir: &Path, quota: QuotaView) -> anyhow::Result<Usage> {
    let c = open_history(dir)?;
    let row = match c.query_row(
        "SELECT asr_requests,llm_requests,audio_seconds FROM usage WHERE day=date('now')",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ) {
        Ok(row) => row,
        Err(rusqlite::Error::QueryReturnedNoRows) => (0, 0, 0.0),
        Err(error) => return Err(error.into()),
    };
    Ok(Usage {
        asr_requests: row.0,
        llm_requests: row.1,
        audio_seconds: row.2,
        quota,
    })
}
pub fn get_history_page(
    dir: &Path,
    limit: i64,
    before_id: Option<i64>,
    query: Option<&str>,
) -> anyhow::Result<HistoryPage> {
    let c = open_history(dir)?;
    let limit = limit.clamp(1, 100);
    let fetch_limit = limit + 1;
    let search_pattern = query
        .filter(|value| !value.is_empty())
        .map(|value| format!("%{}%", escape_history_search(value)));
    let mut s = c.prepare("SELECT id,created_at,COALESCE(raw_text,''),COALESCE((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),final_text,''),COALESCE(duration_secs,0),COALESCE(degraded,0),degraded_reason,COALESCE(status,'ok'),delivery_method,fallback_reason,context_profile_id,raw_audio_path,COALESCE(cleanup_status,'unknown'),(SELECT COUNT(*) FROM dictation_revisions WHERE dictation_id=dictations.id),verbatim_text,COALESCE(verbatim_reviewed,0),asr_text,engine,provider_cleaned_candidate,delivery_error_code,delivery_user_reason,cleanup_rejected_candidate,cleanup_guard_reason,cleanup_attempt_kind,cleanup_transformations_json,cleanup_final_guard_reason,cleanup_failure_reason FROM dictations WHERE (?1 IS NULL OR id < ?1) AND (?2 IS NULL OR (COALESCE(raw_text,'') LIKE ?2 ESCAPE '\\' OR COALESCE(asr_text,'') LIKE ?2 ESCAPE '\\' OR COALESCE(provider_cleaned_candidate,'') LIKE ?2 ESCAPE '\\' OR COALESCE(cleanup_rejected_candidate,'') LIKE ?2 ESCAPE '\\' OR COALESCE((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),final_text,'') LIKE ?2 ESCAPE '\\')) ORDER BY id DESC LIMIT ?3")?;
    let mut items = s
        .query_map(
            params![before_id, search_pattern.as_deref(), fetch_limit],
            history_row(dir),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let has_more = items.len() > limit as usize;
    items.truncate(limit as usize);
    Ok(HistoryPage { items, has_more })
}

fn escape_history_search(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn history_row<'a>(
    dir: &'a Path,
) -> impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<HistoryItem> + 'a {
    move |r| {
        let raw_audio_path: Option<String> = r.get(11)?;
        let status: String = r.get(7)?;
        let has_audio = raw_audio_path
            .as_deref()
            .is_some_and(|path| managed_audio_exists(dir, Path::new(path)));
        let retryable = has_audio && matches!(status.as_str(), "failed" | "degraded");
        let verbatim_text: Option<String> = r.get::<_, Option<String>>(14)?.and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(value)
            }
        });
        Ok(HistoryItem {
            id: r.get(0)?,
            created_at: r.get(1)?,
            raw_text: r.get(2)?,
            final_text: r.get(3)?,
            cleanup_status: r.get(12)?,
            duration: r.get(4)?,
            degraded: r.get::<_, i32>(5)? != 0,
            degraded_reason: r.get(6)?,
            status,
            delivery_method: r.get(8)?,
            fallback_reason: r.get(9)?,
            context_profile_id: r.get(10)?,
            retryable,
            revision_count: r.get::<_, i64>(13)? as usize,
            has_audio,
            verbatim_text,
            verbatim_reviewed: r.get::<_, i32>(15)? != 0,
            asr_text: r.get(16)?,
            engine: r.get(17)?,
            provider_cleaned_candidate: r.get(18)?,
            delivery_error_code: r.get(19)?,
            delivery_user_reason: r.get(20)?,
            cleanup_rejected_candidate: r.get(21)?,
            cleanup_guard_reason: r.get(22)?,
            cleanup_attempt_kind: r.get(23)?,
            cleanup_transformations: r
                .get::<_, Option<String>>(24)?
                .map(|json| {
                    serde_json::from_str(&json).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            24,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })
                })
                .transpose()?,
            cleanup_final_guard_reason: r.get(25)?,
            cleanup_failure_reason: r.get(26)?,
        })
    }
}

#[cfg(test)]
pub fn get_history(dir: &Path, limit: i64) -> anyhow::Result<Vec<HistoryItem>> {
    Ok(get_history_page(dir, limit, None, None)?.items)
}

pub fn purge_history(dir: &Path, keep_history_days: u64) -> anyhow::Result<usize> {
    purge_history_with_details(dir, keep_history_days).map(|(deleted, _)| deleted)
}

pub(crate) fn purge_history_with_details(
    dir: &Path,
    keep_history_days: u64,
) -> anyhow::Result<(usize, bool)> {
    let _mutation = begin_spool_root_mutation(dir)?;
    // Retention may delete linked recovery or gold audio as well as History
    // rows. Keep the compatibility root-wide guard for the full transitive
    // operation instead of claiming a destination-only History scope.
    let _commit = try_spool_io_commit(dir)?;
    let backup_days = if keep_history_days == 0 {
        HISTORY_MIGRATION_BACKUP_MAX_DAYS
    } else {
        keep_history_days.min(HISTORY_MIGRATION_BACKUP_MAX_DAYS)
    };
    prune_history_migration_backups(dir, Some(backup_days), SystemTime::now())?;
    // Zero is the explicit "keep forever" setting for live history text.
    if keep_history_days == 0 {
        return Ok((0, false));
    }
    let c = open_history(dir)?;
    let cutoff = format!("-{keep_history_days} days");
    let mut statement =
        c.prepare("SELECT raw_audio_path FROM dictations WHERE created_at < datetime('now', ?)")?;
    let paths = statement
        .query_map([cutoff.as_str()], |row| row.get::<_, Option<String>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let deleted = c.execute(
        "DELETE FROM dictations WHERE created_at < datetime('now', ?)",
        [cutoff.as_str()],
    )?;
    let deleted_drafts = c.execute(
        "DELETE FROM style_drafts WHERE created_at < datetime('now', ?)",
        [cutoff.as_str()],
    )?;
    c.execute(
        "DELETE FROM usage WHERE day < date('now', ?)",
        [cutoff.as_str()],
    )?;
    drop(statement);
    drop(c);
    for path in paths.into_iter().flatten() {
        remove_spool_artifact_inner(dir, Path::new(&path), true);
    }
    Ok((deleted, deleted_drafts > 0))
}

const LEARN_PAIR_COLUMNS: &str = "pair_key, before_surface, after_surface, hits, promoted, last_at, family, mapping_id, browser_host, native_bundle, last_used_at, pinned, tombstoned_at, ignored, manual_scoped, auto_scoped";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnPairScope {
    pub family: Option<String>,
    pub mapping_id: Option<String>,
    pub browser_host: Option<String>,
    pub native_bundle: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnPairScopeMatch {
    Match,
    Mismatch,
    Unknown,
}

impl LearnPairScope {
    pub fn from_snapshot(snapshot: &crate::context::ContextSnapshot) -> Self {
        Self {
            family: Some(crate::context::family_id(snapshot.profile.family).to_owned()),
            mapping_id: snapshot.profile.id.strip_prefix("user.").map(str::to_owned),
            browser_host: snapshot.target_guard.browser_host.clone(),
            native_bundle: if snapshot.target_guard.browser_host.is_some() {
                None
            } else {
                snapshot.target_guard.bundle_id.clone()
            },
        }
    }

    /// A scoped pair applies only when every declared dimension is known and
    /// equal. An absent observed value is kept distinct from a mismatch.
    pub fn assess_record(&self, row: &LearnPairRecord) -> LearnPairScopeMatch {
        let dimensions = [
            (row.family.as_deref(), self.family.as_deref()),
            (row.mapping_id.as_deref(), self.mapping_id.as_deref()),
            (row.browser_host.as_deref(), self.browser_host.as_deref()),
            (row.native_bundle.as_deref(), self.native_bundle.as_deref()),
        ];
        let mut unknown = false;
        for (required, observed) in dimensions {
            match (required, observed) {
                (None, _) => {}
                (Some(_), None) => unknown = true,
                (Some(required), Some(observed)) if required != observed => {
                    return LearnPairScopeMatch::Mismatch;
                }
                (Some(_), Some(_)) => {}
            }
        }
        if unknown {
            LearnPairScopeMatch::Unknown
        } else {
            LearnPairScopeMatch::Match
        }
    }

    /// Select the one set of promoted pairs authorized for this observed
    /// context. Downstream replacement, hinting, usage, and final protection
    /// must all consume this set rather than reinterpreting scope fields.
    pub fn applicable_promoted_records(&self, rows: &[LearnPairRecord]) -> Vec<LearnPairRecord> {
        rows.iter()
            .filter(|row| {
                row.is_live_promoted() && self.assess_record(row) == LearnPairScopeMatch::Match
            })
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnPairRecord {
    pub pair_key: String,
    pub before_surface: String,
    pub after_surface: String,
    pub hits: u32,
    pub promoted: bool,
    pub last_at: String,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub mapping_id: Option<String>,
    #[serde(default)]
    pub browser_host: Option<String>,
    #[serde(default)]
    pub native_bundle: Option<String>,
    #[serde(default)]
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub tombstoned_at: Option<String>,
    #[serde(default)]
    pub ignored: bool,
    #[serde(default)]
    pub promote_hits: u32,
    #[serde(default)]
    pub manual_scoped: bool,
    #[serde(default)]
    pub auto_scoped: bool,
}

impl LearnPairRecord {
    pub fn is_live_promoted(&self) -> bool {
        self.promoted && !self.ignored && self.tombstoned_at.is_none()
    }

    pub fn is_pending(&self) -> bool {
        !self.promoted && !self.ignored && self.tombstoned_at.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleDraftRecord {
    pub draft_key: String,
    pub mapping_id: String,
    pub style_key: String,
    pub excerpt: String,
    pub before_excerpt: String,
    pub after_excerpt: String,
    pub created_at: String,
}

fn learn_pair_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LearnPairRecord> {
    Ok(LearnPairRecord {
        pair_key: row.get(0)?,
        before_surface: row.get(1)?,
        after_surface: row.get(2)?,
        hits: row.get::<_, i64>(3)? as u32,
        promoted: row.get::<_, i64>(4)? != 0,
        last_at: row.get(5)?,
        family: row.get(6)?,
        mapping_id: row.get(7)?,
        browser_host: row.get(8)?,
        native_bundle: row.get(9)?,
        last_used_at: row.get(10)?,
        pinned: row.get::<_, Option<i64>>(11)?.unwrap_or(0) != 0,
        tombstoned_at: row.get(12)?,
        ignored: row.get::<_, Option<i64>>(13)?.unwrap_or(0) != 0,
        promote_hits: 0,
        manual_scoped: row.get::<_, Option<i64>>(14)?.unwrap_or(0) != 0,
        auto_scoped: row.get::<_, Option<i64>>(15)?.unwrap_or(0) != 0,
    })
}

#[cfg(test)]
pub fn upsert_learn_pair(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
) -> anyhow::Result<Option<LearnPairRecord>> {
    Ok(
        match upsert_learn_pair_with_scope(dir, pair_key, before_surface, after_surface, None)? {
            LearnPairUpsert::Stored(row) => Some(*row),
            LearnPairUpsert::Skipped(_) => None,
        },
    )
}

/// The DB owner distinguishes intentional suppression from missing/failed data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LearnPairSkipReason {
    TombstoneOrIgnored,
    PendingCapacity,
    ScopeConflict,
    DuplicateObservation,
    UnknownDictationOrigin,
}
impl LearnPairSkipReason {
    pub fn code(self) -> &'static str {
        match self {
            Self::TombstoneOrIgnored => "tombstone_or_ignored",
            Self::PendingCapacity => "cap",
            Self::ScopeConflict => "scope_conflict",
            Self::DuplicateObservation => "duplicate_observation",
            Self::UnknownDictationOrigin => "dictation_origin_unknown",
        }
    }
}
pub enum LearnPairUpsert {
    Stored(Box<LearnPairRecord>),
    Skipped(LearnPairSkipReason),
}

/// Opaque identity for one user-originated learning event. The key hashes the
/// source and pair identity; only History row/revision IDs are retained as
/// metadata so deleting the row also deletes its receipts.
#[derive(Clone, Debug)]
pub(crate) struct LearnPairObservation {
    observation_key: String,
    source_kind: &'static str,
    source_history_id: Option<i64>,
    source_revision_id: Option<i64>,
    rule_key: Option<String>,
    dictation_origin: Option<String>,
    channel: Option<&'static str>,
    proof_eligible: bool,
}

impl LearnPairObservation {
    pub(crate) fn history_edit(history_id: i64, revision_id: i64, pair_key: &str) -> Self {
        Self {
            observation_key: learn_observation_key(
                "history_edit",
                &history_id.to_string(),
                pair_key,
            ),
            source_kind: "history_edit",
            source_history_id: Some(history_id),
            source_revision_id: Some(revision_id),
            rule_key: None,
            dictation_origin: None,
            channel: None,
            proof_eligible: false,
        }
    }

    pub(crate) fn history_edit_from_store(
        dir: &Path,
        history_id: i64,
        revision_id: i64,
        pair_key: &str,
    ) -> anyhow::Result<Option<Self>> {
        let connection = open_history(dir)?;
        let origin = connection
            .query_row(
                "SELECT d.transaction_id
                 FROM dictations d
                 JOIN dictation_revisions r ON r.dictation_id = d.id
                 WHERE d.id = ?1 AND r.revision_id = ?2",
                params![history_id, revision_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        let Some(origin) = origin else {
            return Ok(None);
        };
        let Some(origin) = origin.filter(|value| !value.is_empty()) else {
            return Ok(Some(Self::history_edit(history_id, revision_id, pair_key)));
        };
        Ok(Some(Self {
            observation_key: learn_observation_key(
                "history_edit",
                &format!("{origin}:{revision_id}"),
                pair_key,
            ),
            source_kind: "history_edit",
            source_history_id: Some(history_id),
            source_revision_id: Some(revision_id),
            rule_key: Some(pair_key.to_owned()),
            dictation_origin: Some(origin),
            channel: Some("history_edit"),
            proof_eligible: true,
        }))
    }

    pub(crate) fn paste_session(session_id: &str, pair_key: &str) -> Self {
        Self {
            observation_key: learn_observation_key("paste_session", session_id, pair_key),
            source_kind: "paste_session",
            source_history_id: None,
            source_revision_id: None,
            rule_key: None,
            dictation_origin: None,
            channel: None,
            proof_eligible: false,
        }
    }

    pub(crate) fn paste_session_with_dictation_origin(
        session_id: &str,
        pair_key: &str,
        dictation_origin: &str,
    ) -> Self {
        Self {
            observation_key: learn_observation_key("paste_session", session_id, pair_key),
            source_kind: "paste_session",
            source_history_id: None,
            source_revision_id: None,
            rule_key: Some(pair_key.to_owned()),
            dictation_origin: Some(dictation_origin.to_owned()),
            channel: Some("paste"),
            proof_eligible: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn research_fixture_origin(source_origin_id: &str, pair_key: &str) -> Self {
        Self::research_fixture_origin_with_observation_id(
            source_origin_id,
            source_origin_id,
            pair_key,
        )
    }

    #[cfg(any(test, feature = "research-harness"))]
    pub(crate) fn research_fixture_origin_with_observation_id(
        observation_id: &str,
        source_origin_id: &str,
        pair_key: &str,
    ) -> Self {
        let dictation_origin = format!("research_fixture:{source_origin_id}");
        Self {
            observation_key: learn_observation_key(
                "research_fixture_edit",
                observation_id,
                pair_key,
            ),
            source_kind: "research_fixture_edit",
            source_history_id: None,
            source_revision_id: None,
            rule_key: Some(pair_key.to_owned()),
            dictation_origin: Some(dictation_origin),
            channel: Some("research_fixture"),
            proof_eligible: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn proof_eligible(&self) -> bool {
        self.proof_eligible
    }

    #[cfg(any(test, feature = "research-harness"))]
    pub(crate) fn research_fixture_edit(operation_id: &str, pair_key: &str) -> Self {
        Self {
            observation_key: learn_observation_key("research_fixture_edit", operation_id, pair_key),
            source_kind: "research_fixture_edit",
            source_history_id: None,
            source_revision_id: None,
            rule_key: None,
            dictation_origin: None,
            channel: None,
            proof_eligible: false,
        }
    }

    #[cfg(any(test, feature = "research-harness"))]
    pub(crate) fn research_fixture_apply(operation_id: &str, pair_key: &str) -> Self {
        Self {
            observation_key: learn_observation_key(
                "research_fixture_apply",
                operation_id,
                pair_key,
            ),
            source_kind: "research_fixture_apply",
            source_history_id: None,
            source_revision_id: None,
            rule_key: None,
            dictation_origin: None,
            channel: None,
            proof_eligible: false,
        }
    }

    #[cfg(any(test, feature = "research-harness"))]
    pub(crate) fn research_fixture_noop(operation_id: &str, pair_key: &str) -> Self {
        Self {
            observation_key: learn_observation_key("research_fixture_noop", operation_id, pair_key),
            source_kind: "research_fixture_noop",
            source_history_id: None,
            source_revision_id: None,
            rule_key: None,
            dictation_origin: None,
            channel: None,
            proof_eligible: false,
        }
    }

    #[cfg(any(test, feature = "research-harness"))]
    pub(crate) fn research_fixture_undo(operation_id: &str, pair_key: &str) -> Self {
        Self {
            observation_key: learn_observation_key("research_fixture_undo", operation_id, pair_key),
            source_kind: "research_fixture_undo",
            source_history_id: None,
            source_revision_id: None,
            rule_key: None,
            dictation_origin: None,
            channel: None,
            proof_eligible: false,
        }
    }
}

fn learn_observation_key(source_kind: &str, origin: &str, pair_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    for value in [
        source_kind.as_bytes(),
        origin.as_bytes(),
        pair_key.as_bytes(),
    ] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value);
    }
    hex::encode(digest.finalize())
}

enum VerifiedDictationOrigin {
    Committed(i64),
    Pending(SpoolIoCommitPermit),
    Fixture,
}

fn verify_dictation_origin(
    dir: &Path,
    connection: &Connection,
    observation: &LearnPairObservation,
) -> anyhow::Result<Option<VerifiedDictationOrigin>> {
    let (Some(rule_key), Some(dictation_origin), Some(channel)) = (
        observation.rule_key.as_deref(),
        observation.dictation_origin.as_deref(),
        observation.channel,
    ) else {
        return Ok(None);
    };
    let expected_source_kind = match channel {
        "paste" => "paste_session",
        "history_edit" => "history_edit",
        "research_fixture" => "research_fixture_edit",
        _ => return Ok(None),
    };
    if rule_key.is_empty()
        || rule_key.len() > 256
        || dictation_origin.is_empty()
        || dictation_origin.len() > 256
        || observation.source_kind != expected_source_kind
    {
        return Ok(None);
    }
    if channel == "research_fixture" {
        return Ok(dictation_origin
            .strip_prefix("research_fixture:")
            .filter(|source_id| !source_id.is_empty() && source_id.len() <= 240)
            .map(|_| VerifiedDictationOrigin::Fixture));
    }
    if let Some(history_id) = connection
        .query_row(
            "SELECT id FROM dictations WHERE transaction_id = ?1",
            [dictation_origin],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
    {
        return Ok(Some(VerifiedDictationOrigin::Committed(history_id)));
    }
    if channel == "history_edit" {
        return Ok(None);
    }
    let pending_commit = try_spool_io_commit_pending_history(dir, dictation_origin)?;
    if !active_pending_history_origin(dir, dictation_origin)? {
        return Ok(None);
    }
    Ok(Some(VerifiedDictationOrigin::Pending(pending_commit)))
}

pub(crate) fn count_independent_learn_pair_hits(dir: &Path, pair_key: &str) -> anyhow::Result<u32> {
    let connection = open_history(dir)?;
    let count = connection.query_row(
        "SELECT COUNT(DISTINCT dictation_origin)
         FROM learn_observations
         WHERE rule_key = ?1
           AND proof_eligible = 1
           AND dictation_origin IS NOT NULL",
        [pair_key],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(count as u32)
}

#[cfg(test)]
pub fn upsert_learn_pair_with_scope(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
) -> anyhow::Result<LearnPairUpsert> {
    upsert_learn_pair_with_observation(dir, pair_key, before_surface, after_surface, scope, None)
}

pub(crate) fn upsert_learn_pair_with_observation(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
    observation: Option<&LearnPairObservation>,
) -> anyhow::Result<LearnPairUpsert> {
    upsert_learn_pair_with_observation_policy(
        dir,
        pair_key,
        before_surface,
        after_surface,
        scope,
        observation,
        false,
    )
}

pub(crate) fn upsert_style_learning_pair_with_observation(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
    observation: Option<&LearnPairObservation>,
) -> anyhow::Result<LearnPairUpsert> {
    if !(pair_key.starts_with("learn:style:") || pair_key.starts_with("learn:intensity:")) {
        anyhow::bail!("text-refresh upsert is limited to internal style learning keys");
    }
    upsert_learn_pair_with_observation_policy(
        dir,
        pair_key,
        before_surface,
        after_surface,
        scope,
        observation,
        true,
    )
}

#[cfg(any(test, feature = "research-harness"))]
pub(crate) fn has_learn_pair_observation(
    dir: &Path,
    observation: &LearnPairObservation,
) -> anyhow::Result<bool> {
    let connection = open_history(dir)?;
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM learn_observations WHERE observation_key = ?1)",
        [&observation.observation_key],
        |row| row.get(0),
    )?)
}

#[cfg(any(test, feature = "research-harness"))]
pub(crate) fn record_research_fixture_application(
    dir: &Path,
    pair_key: &str,
    after_surface: &str,
    observation: &LearnPairObservation,
    applied: bool,
) -> anyhow::Result<bool> {
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute(
        "INSERT OR IGNORE INTO learn_observations (observation_key, source_kind, source_history_id, source_revision_id, created_at) VALUES (?1, ?2, ?3, ?4, datetime('now'))",
        params![
            observation.observation_key,
            observation.source_kind,
            observation.source_history_id,
            observation.source_revision_id
        ],
    )?;
    if transaction.changes() == 0 {
        return Ok(false);
    }
    if applied {
        transaction.execute(
            "UPDATE learn_pairs SET last_used_at = datetime('now') WHERE pair_key = ?1 AND after_surface = ?2 AND promoted = 1 AND ignored = 0 AND tombstoned_at IS NULL",
            params![pair_key, after_surface],
        )?;
        if transaction.changes() != 1 {
            anyhow::bail!("research application pair is no longer live");
        }
        transaction.execute(
            "INSERT INTO learned_term_usage (word, replacement_runs, last_replaced_at) VALUES (?1, 1, datetime('now')) ON CONFLICT(word) DO UPDATE SET replacement_runs = replacement_runs + 1, last_replaced_at = datetime('now')",
            [after_surface],
        )?;
    }
    transaction.execute(
        "DELETE FROM learn_observations WHERE source_history_id IS NULL AND observation_key IN (SELECT observation_key FROM learn_observations WHERE source_history_id IS NULL ORDER BY rowid DESC LIMIT -1 OFFSET 4096)",
        [],
    )?;
    transaction.commit()?;
    Ok(true)
}

fn upsert_learn_pair_with_observation_policy(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
    observation: Option<&LearnPairObservation>,
    refresh_conflicting_text: bool,
) -> anyhow::Result<LearnPairUpsert> {
    let mut c = open_history(dir)?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if let Some(existing) = tx
        .query_row(
            &format!("SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs WHERE pair_key = ?1"),
            [pair_key],
            learn_pair_from_row,
        )
        .optional()?
    {
        if existing.ignored || existing.tombstoned_at.is_some() {
            return Ok(LearnPairUpsert::Skipped(
                LearnPairSkipReason::TombstoneOrIgnored,
            ));
        }
        let existing_scope = LearnPairScope {
            family: existing.family.clone(),
            mapping_id: existing.mapping_id.clone(),
            browser_host: existing.browser_host.clone(),
            native_bundle: existing.native_bundle.clone(),
        };
        if existing_scope != scope.cloned().unwrap_or_default()
            || (!refresh_conflicting_text
                && (existing.before_surface != before_surface
                    || existing.after_surface != after_surface))
        {
            return Ok(LearnPairUpsert::Skipped(LearnPairSkipReason::ScopeConflict));
        }
    } else {
        let pending: i64 = tx.query_row(
            "SELECT COUNT(*) FROM learn_pairs WHERE promoted = 0 AND ignored = 0 AND tombstoned_at IS NULL",
            [],
            |row| row.get(0),
        )?;
        if pending >= LEARN_PAIRS_PENDING_CAP {
            return Ok(LearnPairUpsert::Skipped(
                LearnPairSkipReason::PendingCapacity,
            ));
        }
    }
    let mut pending_origin_commit = None;
    if let Some(observation) = observation {
        let source_history_id = if observation.proof_eligible {
            if observation.rule_key.as_deref() != Some(pair_key) {
                return Ok(LearnPairUpsert::Skipped(
                    LearnPairSkipReason::UnknownDictationOrigin,
                ));
            }
            match verify_dictation_origin(dir, &tx, observation)? {
                Some(VerifiedDictationOrigin::Committed(history_id)) => Some(history_id),
                Some(VerifiedDictationOrigin::Pending(commit)) => {
                    pending_origin_commit = Some(commit);
                    None
                }
                Some(VerifiedDictationOrigin::Fixture) => None,
                None => {
                    return Ok(LearnPairUpsert::Skipped(
                        LearnPairSkipReason::UnknownDictationOrigin,
                    ));
                }
            }
        } else {
            observation.source_history_id
        };
        tx.execute(
            "INSERT OR IGNORE INTO learn_observations (
                observation_key, source_kind, source_history_id, source_revision_id,
                created_at, rule_key, dictation_origin, channel, proof_eligible
             ) VALUES (?1, ?2, ?3, ?4, datetime('now'), ?5, ?6, ?7, ?8)",
            params![
                observation.observation_key,
                observation.source_kind,
                source_history_id,
                observation.source_revision_id,
                observation.rule_key,
                observation.dictation_origin,
                observation.channel,
                observation.proof_eligible
            ],
        )?;
        if tx.changes() == 0 {
            if observation.proof_eligible {
                tx.execute(
                    "UPDATE learn_observations
                     SET source_history_id = COALESCE(source_history_id, ?3),
                         source_revision_id = COALESCE(source_revision_id, ?4),
                         channel = CASE WHEN ?5 = 'history_edit' THEN ?5 ELSE channel END
                     WHERE rule_key = ?1 AND dictation_origin = ?2 AND proof_eligible = 1",
                    params![
                        observation.rule_key,
                        observation.dictation_origin,
                        source_history_id,
                        observation.source_revision_id,
                        observation.channel
                    ],
                )?;
            }
            tx.commit()?;
            return Ok(LearnPairUpsert::Skipped(
                LearnPairSkipReason::DuplicateObservation,
            ));
        }
        // Paste receipts only need to cover recently completed process/session
        // retries. History receipts live with their parent History row.
        tx.execute(
            "DELETE FROM learn_observations WHERE source_history_id IS NULL AND observation_key IN (SELECT observation_key FROM learn_observations WHERE source_history_id IS NULL ORDER BY rowid DESC LIMIT -1 OFFSET 4096)",
            [],
        )?;
    }
    let scope = scope.cloned().unwrap_or_default();
    let before_update = if refresh_conflicting_text {
        "excluded.before_surface"
    } else {
        "learn_pairs.before_surface"
    };
    let after_update = if refresh_conflicting_text {
        "excluded.after_surface"
    } else {
        "learn_pairs.after_surface"
    };
    let record = tx.query_row(
        &format!(
        "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, family, mapping_id, browser_host, native_bundle, last_used_at, pinned, tombstoned_at, ignored)
             VALUES (?1, ?2, ?3, 1, 0, datetime('now'), ?4, ?5, ?6, ?7, NULL, 0, NULL, 0)
             ON CONFLICT(pair_key) DO UPDATE SET
               before_surface = {before_update},
               after_surface = {after_update},
               hits = hits + 1,
               last_at = datetime('now'),
               family = excluded.family,
               mapping_id = excluded.mapping_id,
               browser_host = excluded.browser_host,
               native_bundle = excluded.native_bundle
             RETURNING {LEARN_PAIR_COLUMNS}"
        ),
        params![
            pair_key,
            before_surface,
            after_surface,
            scope.family,
            scope.mapping_id,
            scope.browser_host,
            scope.native_bundle
        ],
        learn_pair_from_row,
    )?;
    tx.commit()?;
    drop(pending_origin_commit);
    Ok(LearnPairUpsert::Stored(Box::new(record)))
}

pub fn get_learn_pair(dir: &Path, pair_key: &str) -> anyhow::Result<Option<LearnPairRecord>> {
    let c = open_history(dir)?;
    Ok(c.query_row(
        &format!("SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs WHERE pair_key = ?1"),
        [pair_key],
        learn_pair_from_row,
    )
    .optional()?)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DictionaryEffectAction {
    Add,
    Remove,
}

impl DictionaryEffectAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Remove => "remove",
        }
    }

    fn from_str(value: &str) -> anyhow::Result<Self> {
        match value {
            "add" => Ok(Self::Add),
            "remove" => Ok(Self::Remove),
            _ => anyhow::bail!("invalid pending dictionary effect"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingDictionaryEffect {
    pub pair_key: String,
    pub word: String,
    pub action: DictionaryEffectAction,
    pub version: i64,
}

fn is_global_dictionary_word_rule(
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    manual_scoped: bool,
    auto_scoped: bool,
) -> bool {
    !before_surface.trim().is_empty()
        && !after_surface.trim().is_empty()
        && !manual_scoped
        && !auto_scoped
        && !pair_key.starts_with("learn:style:")
        && !pair_key.starts_with("learn:intensity:")
}

fn stage_dictionary_effect(
    transaction: &rusqlite::Transaction<'_>,
    pair_key: &str,
    action: DictionaryEffectAction,
) -> anyhow::Result<()> {
    transaction.execute(
        "UPDATE learn_pairs
         SET dictionary_pending_action = ?2,
             dictionary_effect_version = dictionary_effect_version + 1
         WHERE pair_key = ?1",
        params![pair_key, action.as_str()],
    )?;
    Ok(())
}

pub(crate) fn list_pending_dictionary_effects(
    dir: &Path,
) -> anyhow::Result<Vec<PendingDictionaryEffect>> {
    let connection = open_history(dir)?;
    let mut statement = connection.prepare(
        "SELECT pair_key, after_surface, dictionary_pending_action, dictionary_effect_version
         FROM learn_pairs
         WHERE dictionary_pending_action != 'none'
         ORDER BY dictionary_effect_version, pair_key",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    rows.map(|row| {
        let (pair_key, word, action, version) = row?;
        Ok(PendingDictionaryEffect {
            pair_key,
            word,
            action: DictionaryEffectAction::from_str(&action)?,
            version,
        })
    })
    .collect()
}

pub(crate) fn acknowledge_dictionary_effect(
    dir: &Path,
    pair_key: &str,
    version: i64,
) -> anyhow::Result<bool> {
    let connection = open_history(dir)?;
    connection.execute(
        "UPDATE learn_pairs
         SET dictionary_pending_action = 'none'
         WHERE pair_key = ?1
           AND dictionary_effect_version = ?2
           AND dictionary_pending_action != 'none'",
        params![pair_key, version],
    )?;
    Ok(connection.changes() == 1)
}

pub(crate) fn cancel_pending_dictionary_effects_for_word(
    dir: &Path,
    word: &str,
) -> anyhow::Result<bool> {
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute(
        "UPDATE learn_pairs
         SET dictionary_pending_action = 'none',
             dictionary_effect_version = dictionary_effect_version + 1
         WHERE after_surface = ?1 COLLATE NOCASE
           AND dictionary_pending_action = 'add'",
        [word],
    )?;
    let cancelled = transaction.changes() > 0;
    transaction.commit()?;
    Ok(cancelled)
}

#[cfg(test)]
pub fn mark_learn_pair_promoted(dir: &Path, pair_key: &str) -> anyhow::Result<bool> {
    mark_learn_pair_promoted_with_scope_kind(dir, pair_key, false)
}

#[cfg(test)]
pub(crate) fn mark_learn_pair_promoted_with_scope_kind(
    dir: &Path,
    pair_key: &str,
    auto_scoped: bool,
) -> anyhow::Result<bool> {
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let prior = transaction
        .query_row(
            "SELECT before_surface, after_surface, promoted, ignored, tombstoned_at,
                    manual_scoped, auto_scoped, dictionary_pending_action
             FROM learn_pairs WHERE pair_key = ?1",
            [pair_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<i64>>(5)?.unwrap_or(0) != 0,
                    row.get::<_, Option<i64>>(6)?.unwrap_or(0) != 0,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?;
    let prior_global = prior.as_ref().is_some_and(
        |(before, after, promoted, ignored, tombstoned_at, manual_scoped, prior_auto, _)| {
            *promoted
                && !*ignored
                && tombstoned_at.is_none()
                && is_global_dictionary_word_rule(
                    pair_key,
                    before,
                    after,
                    *manual_scoped,
                    *prior_auto,
                )
        },
    );
    let had_pending_global_add = prior.as_ref().is_some_and(
        |(before, after, _, _, _, manual_scoped, prior_auto, action)| {
            action == "add"
                && is_global_dictionary_word_rule(
                    pair_key,
                    before,
                    after,
                    *manual_scoped,
                    *prior_auto,
                )
        },
    );
    transaction.execute(
        "UPDATE learn_pairs SET promoted = 1, ignored = 0, tombstoned_at = NULL, auto_scoped = CASE WHEN ?2 THEN 1 ELSE auto_scoped END, last_at = datetime('now') WHERE pair_key = ?1",
        params![pair_key, auto_scoped],
    )?;
    let changed = transaction.changes() > 0;
    if changed {
        let (before, after, manual_scoped, stored_auto_scoped) = transaction.query_row(
            "SELECT before_surface, after_surface, manual_scoped, auto_scoped
             FROM learn_pairs WHERE pair_key = ?1",
            [pair_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?.unwrap_or(0) != 0,
                    row.get::<_, Option<i64>>(3)?.unwrap_or(0) != 0,
                ))
            },
        )?;
        let is_global = is_global_dictionary_word_rule(
            pair_key,
            &before,
            &after,
            manual_scoped,
            stored_auto_scoped,
        );
        if is_global {
            stage_dictionary_effect(&transaction, pair_key, DictionaryEffectAction::Add)?;
        } else if prior_global || had_pending_global_add {
            stage_dictionary_effect(&transaction, pair_key, DictionaryEffectAction::Remove)?;
        }
    }
    transaction.commit()?;
    Ok(changed)
}

/// Promote an automatically learned pair only while its exact candidate is
/// still live, in the same scope, and at its required hit threshold. This
/// conditional update never clears a tombstone or ignored marker.
pub(crate) fn promote_learn_pair_if_eligible(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
    threshold: u32,
    auto_scoped: bool,
) -> anyhow::Result<bool> {
    let scope = scope.cloned().unwrap_or_default();
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let changed = transaction.execute(
        "UPDATE learn_pairs
         SET promoted = 1,
             auto_scoped = CASE WHEN ?9 THEN 1 ELSE auto_scoped END,
             last_at = datetime('now')
         WHERE pair_key = ?1
           AND before_surface = ?2
           AND after_surface = ?3
           AND family IS ?4
           AND mapping_id IS ?5
           AND browser_host IS ?6
           AND native_bundle IS ?7
           AND (?2 = '' AND hits >= ?8 OR ?2 != '' AND (
               SELECT COUNT(DISTINCT dictation_origin)
               FROM learn_observations
               WHERE rule_key = ?1
                 AND proof_eligible = 1
                 AND dictation_origin IS NOT NULL
           ) >= ?8)
           AND promoted = 0
           AND ignored = 0
           AND tombstoned_at IS NULL",
        params![
            pair_key,
            before_surface,
            after_surface,
            scope.family,
            scope.mapping_id,
            scope.browser_host,
            scope.native_bundle,
            threshold as i64,
            auto_scoped
        ],
    )?;
    if changed > 0 {
        let (before, after, manual_scoped, stored_auto_scoped) = transaction.query_row(
            "SELECT before_surface, after_surface, manual_scoped, auto_scoped
             FROM learn_pairs WHERE pair_key = ?1",
            [pair_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?.unwrap_or(0) != 0,
                    row.get::<_, Option<i64>>(3)?.unwrap_or(0) != 0,
                ))
            },
        )?;
        if is_global_dictionary_word_rule(
            pair_key,
            &before,
            &after,
            manual_scoped,
            stored_auto_scoped,
        ) {
            stage_dictionary_effect(&transaction, pair_key, DictionaryEffectAction::Add)?;
        }
    }
    transaction.commit()?;
    Ok(changed > 0)
}

pub fn ensure_learn_pair_promoted(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
) -> anyhow::Result<()> {
    let scope = scope.ok_or_else(|| anyhow::anyhow!("learn pair scope is required"))?;
    let mut c = open_history(dir)?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let existing = tx
        .query_row(
            &format!("SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs WHERE pair_key = ?1"),
            [pair_key],
            learn_pair_from_row,
        )
        .optional()?;
    if let Some(existing) = existing {
        let existing_scope = LearnPairScope {
            family: existing.family,
            mapping_id: existing.mapping_id,
            browser_host: existing.browser_host,
            native_bundle: existing.native_bundle,
        };
        if existing_scope != *scope {
            anyhow::bail!("learn pair scope conflicts with the recorded scope");
        }
        if existing.before_surface != before_surface || existing.after_surface != after_surface {
            anyhow::bail!("learn pair surface conflicts with the recorded pair key");
        }
    }
    tx.execute(
        "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, family, mapping_id, browser_host, native_bundle, pinned, ignored, manual_scoped)
         VALUES (?1, ?2, ?3, 3, 1, datetime('now'), ?4, ?5, ?6, ?7, 0, 0, 0)
         ON CONFLICT(pair_key) DO UPDATE SET
           promoted = 1,
           ignored = 0,
           tombstoned_at = NULL,
           last_at = datetime('now'),
           family = excluded.family,
           mapping_id = excluded.mapping_id,
           browser_host = excluded.browser_host,
           native_bundle = excluded.native_bundle",
        params![
            pair_key,
            before_surface,
            after_surface,
            scope.family,
            scope.mapping_id,
            scope.browser_host,
            scope.native_bundle
        ],
    )?;
    let (before, after, manual_scoped, auto_scoped) = tx.query_row(
        "SELECT before_surface, after_surface, manual_scoped, auto_scoped
         FROM learn_pairs WHERE pair_key = ?1",
        [pair_key],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?.unwrap_or(0) != 0,
                row.get::<_, Option<i64>>(3)?.unwrap_or(0) != 0,
            ))
        },
    )?;
    if is_global_dictionary_word_rule(pair_key, &before, &after, manual_scoped, auto_scoped) {
        // Explicit confirmation also repairs a missing Settings projection for
        // an already-promoted global rule. The Settings owner applies this
        // source idempotently before conditionally acknowledging this version.
        stage_dictionary_effect(&tx, pair_key, DictionaryEffectAction::Add)?;
    }
    tx.commit()?;
    Ok(())
}

#[derive(Debug)]
pub enum ManualScopedPairStoreError {
    InvalidScope,
    ScopeConflict,
    #[expect(
        dead_code,
        reason = "retain the private storage cause for internal diagnostics while code() exposes only a stable category"
    )]
    Storage(anyhow::Error),
}

impl ManualScopedPairStoreError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidScope => "manual_pair_scope_unknown",
            Self::ScopeConflict => "manual_pair_scope_conflict",
            Self::Storage(_) => "manual_pair_save_failed",
        }
    }
}

pub fn ensure_manual_scoped_learn_pair(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: &LearnPairScope,
) -> Result<LearnPairRecord, ManualScopedPairStoreError> {
    if scope.native_bundle.as_deref().is_none_or(str::is_empty) || scope.browser_host.is_some() {
        return Err(ManualScopedPairStoreError::InvalidScope);
    }
    let mut c = open_history(dir).map_err(ManualScopedPairStoreError::Storage)?;
    let tx = c
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?;
    let existing = tx
        .query_row(
            &format!("SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs WHERE pair_key = ?1"),
            [pair_key],
            learn_pair_from_row,
        )
        .optional()
        .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?;
    let was_global_promoted = existing.as_ref().is_some_and(|existing| {
        existing.is_live_promoted()
            && is_global_dictionary_word_rule(
                pair_key,
                &existing.before_surface,
                &existing.after_surface,
                existing.manual_scoped,
                existing.auto_scoped,
            )
    });
    let had_pending_global_add = tx
        .query_row(
            "SELECT dictionary_pending_action = 'add' FROM learn_pairs WHERE pair_key = ?1",
            [pair_key],
            |row| row.get::<_, bool>(0),
        )
        .optional()
        .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?
        .unwrap_or(false);
    if let Some(existing) = existing {
        let existing_scope = LearnPairScope {
            family: existing.family,
            mapping_id: existing.mapping_id,
            browser_host: existing.browser_host,
            native_bundle: existing.native_bundle,
        };
        if existing.before_surface != before_surface || existing.after_surface != after_surface {
            return Err(ManualScopedPairStoreError::ScopeConflict);
        }
        // A user may explicitly rebind an existing exact pair only after its
        // previous rule has been disarmed. Active rules remain scope-bound.
        if existing_scope != *scope && !existing.ignored && existing.tombstoned_at.is_none() {
            return Err(ManualScopedPairStoreError::ScopeConflict);
        }
    }
    let row = tx.query_row(
        &format!(
            "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, family, mapping_id, browser_host, native_bundle, pinned, ignored, manual_scoped)
         VALUES (?1, ?2, ?3, 0, 1, datetime('now'), ?4, ?5, NULL, ?6, 0, 0, 1)
         ON CONFLICT(pair_key) DO UPDATE SET
           promoted = 1,
           ignored = 0,
           tombstoned_at = NULL,
           last_at = datetime('now'),
           family = excluded.family,
           mapping_id = excluded.mapping_id,
           browser_host = NULL,
           native_bundle = excluded.native_bundle,
           manual_scoped = 1,
           auto_scoped = 0
         RETURNING {LEARN_PAIR_COLUMNS}"
        ),
        params![
            pair_key,
            before_surface,
            after_surface,
            scope.family,
            scope.mapping_id,
            scope.native_bundle
        ],
        learn_pair_from_row,
    ).map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?;
    if was_global_promoted || had_pending_global_add {
        stage_dictionary_effect(&tx, pair_key, DictionaryEffectAction::Remove)
            .map_err(ManualScopedPairStoreError::Storage)?;
    }
    tx.commit()
        .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?;
    Ok(row)
}

pub fn set_learn_pair_hits(dir: &Path, pair_key: &str, hits: u32) -> anyhow::Result<bool> {
    let c = open_history(dir)?;
    c.execute(
        "UPDATE learn_pairs SET hits = ?2, promoted = 0, last_at = datetime('now') WHERE pair_key = ?1 AND ignored = 0 AND tombstoned_at IS NULL",
        params![pair_key, hits as i64],
    )?;
    Ok(c.changes() > 0)
}

fn tombstone_learn_pair_with_observation(
    dir: &Path,
    pair_key: &str,
    observation: Option<&LearnPairObservation>,
) -> anyhow::Result<(bool, bool)> {
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let prior = transaction
        .query_row(
            "SELECT before_surface, after_surface, promoted, ignored, tombstoned_at,
                    manual_scoped, auto_scoped, dictionary_pending_action
             FROM learn_pairs WHERE pair_key = ?1",
            [pair_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? != 0,
                    row.get::<_, i64>(3)? != 0,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<i64>>(5)?.unwrap_or(0) != 0,
                    row.get::<_, Option<i64>>(6)?.unwrap_or(0) != 0,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?;
    let should_remove_dictionary_effect = prior.as_ref().is_some_and(
        |(before, after, promoted, ignored, tombstoned_at, manual_scoped, auto_scoped, action)| {
            is_global_dictionary_word_rule(pair_key, before, after, *manual_scoped, *auto_scoped)
                && ((*promoted && !*ignored && tombstoned_at.is_none()) || action == "add")
        },
    );
    if let Some(observation) = observation {
        transaction.execute(
            "INSERT OR IGNORE INTO learn_observations (observation_key, source_kind, source_history_id, source_revision_id, created_at) VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            params![
                observation.observation_key,
                observation.source_kind,
                observation.source_history_id,
                observation.source_revision_id
            ],
        )?;
        if transaction.changes() == 0 {
            return Ok((false, true));
        }
        transaction.execute(
            "DELETE FROM learn_observations WHERE source_history_id IS NULL AND observation_key IN (SELECT observation_key FROM learn_observations WHERE source_history_id IS NULL ORDER BY rowid DESC LIMIT -1 OFFSET 4096)",
            [],
        )?;
    }
    if observation.is_some() {
        transaction.execute(
            "UPDATE learn_pairs SET ignored = 1, tombstoned_at = datetime('now'), last_at = datetime('now') WHERE pair_key = ?1 AND tombstoned_at IS NULL",
            [pair_key],
        )?;
    } else {
        transaction.execute(
            "UPDATE learn_pairs SET ignored = 1, tombstoned_at = datetime('now'), last_at = datetime('now') WHERE pair_key = ?1",
            [pair_key],
        )?;
    }
    let changed = transaction.changes() > 0;
    if observation.is_some() && !changed {
        anyhow::bail!("research undo pair is no longer live");
    }
    if changed && should_remove_dictionary_effect {
        stage_dictionary_effect(&transaction, pair_key, DictionaryEffectAction::Remove)?;
    }
    transaction.commit()?;
    Ok((changed, false))
}

pub fn tombstone_learn_pair(dir: &Path, pair_key: &str) -> anyhow::Result<bool> {
    Ok(tombstone_learn_pair_with_observation(dir, pair_key, None)?.0)
}

#[cfg(any(test, feature = "research-harness"))]
pub(crate) fn tombstone_research_learn_pair(
    dir: &Path,
    pair_key: &str,
    observation: &LearnPairObservation,
) -> anyhow::Result<(bool, bool)> {
    tombstone_learn_pair_with_observation(dir, pair_key, Some(observation))
}

pub fn ensure_pinned_dictionary_term(
    dir: &Path,
    after_surface: &str,
    pinned: bool,
) -> anyhow::Result<()> {
    let pair_key = crate::dictionary_learn::pair_key("", after_surface);
    let c = open_history(dir)?;
    c.execute(
        "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, pinned, ignored)
         VALUES (?1, '', ?2, 0, 1, datetime('now'), ?3, 0)
         ON CONFLICT(pair_key) DO UPDATE SET
           pinned = excluded.pinned,
           last_at = datetime('now')",
        params![pair_key, after_surface, i64::from(pinned)],
    )?;
    Ok(())
}

pub fn bump_learn_pairs_used(dir: &Path, pair_keys: &[String]) -> anyhow::Result<()> {
    if pair_keys.is_empty() {
        return Ok(());
    }
    let c = open_history(dir)?;
    let mut statement =
        c.prepare("UPDATE learn_pairs SET last_used_at = datetime('now') WHERE pair_key = ?1")?;
    for key in pair_keys {
        statement.execute([key])?;
    }
    Ok(())
}

pub fn list_learn_pairs(dir: &Path) -> anyhow::Result<Vec<LearnPairRecord>> {
    let c = open_history(dir)?;
    let mut statement = c.prepare(&format!(
        "SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs ORDER BY last_at DESC, pair_key ASC"
    ))?;
    let rows = statement.query_map([], learn_pair_from_row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Application allocation limits for the complete, non-paged Settings audit.
pub(crate) const AUDIT_PAIR_MAX_ROWS: usize = 1024;
pub(crate) const AUDIT_PAIR_MAX_UTF8_BYTES: usize = 2 * 1024 * 1024;
#[derive(Debug)]
pub(crate) enum AuditPairReadError {
    Capacity {
        resource: &'static str,
        limit: usize,
    },
    Store,
}
pub(crate) fn list_learn_pairs_for_audit(
    dir: &Path,
) -> Result<Vec<LearnPairRecord>, AuditPairReadError> {
    use rusqlite::types::ValueRef;
    let c = open_history(dir).map_err(|_| AuditPairReadError::Store)?;
    let mut query = c
        .prepare(&format!(
            "SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs ORDER BY last_at DESC, pair_key ASC"
        ))
        .map_err(|_| AuditPairReadError::Store)?;
    let mut cursor = query.query([]).map_err(|_| AuditPairReadError::Store)?;
    let mut rows = Vec::new();
    let mut bytes = 0usize;
    while let Some(row) = cursor.next().map_err(|_| AuditPairReadError::Store)? {
        if rows.len() == AUDIT_PAIR_MAX_ROWS {
            return Err(AuditPairReadError::Capacity {
                resource: "rows",
                limit: AUDIT_PAIR_MAX_ROWS,
            });
        }
        // SQLite may materialize its current row. No owned String/row copy is made before admission.
        for index in [0_usize, 1, 2, 5, 6, 7, 8, 9, 10, 12] {
            let size = match row.get_ref(index).map_err(|_| AuditPairReadError::Store)? {
                ValueRef::Text(text) => text.len(),
                ValueRef::Null => 0,
                _ => return Err(AuditPairReadError::Store),
            };
            bytes = bytes
                .checked_add(size)
                .filter(|n| *n <= AUDIT_PAIR_MAX_UTF8_BYTES)
                .ok_or(AuditPairReadError::Capacity {
                    resource: "utf8_bytes",
                    limit: AUDIT_PAIR_MAX_UTF8_BYTES,
                })?;
        }
        rows.push(learn_pair_from_row(row).map_err(|_| AuditPairReadError::Store)?);
    }
    Ok(rows)
}

#[derive(Clone, Serialize)]
pub struct LearnedTermUsage {
    pub word: String,
    pub replacement_runs: u64,
    pub last_replaced_at: String,
}

/// One observation per distinct corrected term in one local processing pass.
pub fn record_learned_term_usage(dir: &Path, words: &[String]) -> anyhow::Result<()> {
    if words.is_empty() {
        return Ok(());
    }
    let mut c = open_history(dir)?;
    let transaction = c.transaction()?;
    for word in words.iter().collect::<std::collections::BTreeSet<_>>() {
        transaction.execute("INSERT INTO learned_term_usage (word, replacement_runs, last_replaced_at) VALUES (?1, 1, datetime('now')) ON CONFLICT(word) DO UPDATE SET replacement_runs = replacement_runs + 1, last_replaced_at = datetime('now')", [word])?;
    }
    transaction.commit()?;
    Ok(())
}

pub fn list_learned_term_usage(dir: &Path) -> anyhow::Result<Vec<LearnedTermUsage>> {
    let c = open_history(dir)?;
    let mut statement = c.prepare("SELECT word, replacement_runs, last_replaced_at FROM learned_term_usage WHERE EXISTS (SELECT 1 FROM learn_pairs WHERE after_surface COLLATE NOCASE = word COLLATE NOCASE AND promoted = 1 AND ignored = 0 AND tombstoned_at IS NULL) ORDER BY last_replaced_at DESC, word ASC")?;
    let rows = statement.query_map([], |row| {
        Ok(LearnedTermUsage {
            word: row.get(0)?,
            replacement_runs: row.get(1)?,
            last_replaced_at: row.get(2)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn upsert_style_draft(
    dir: &Path,
    mapping_id: &str,
    style_key: &str,
    excerpt: &str,
    before_excerpt: &str,
    after_excerpt: &str,
) -> anyhow::Result<StyleDraftRecord> {
    let draft_key = format!("{mapping_id}\u{1e}{style_key}");
    let c = open_history(dir)?;
    c.execute(
        "INSERT INTO style_drafts (draft_key, mapping_id, style_key, excerpt, before_excerpt, after_excerpt, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))
         ON CONFLICT(draft_key) DO UPDATE SET
           excerpt = excluded.excerpt,
           before_excerpt = excluded.before_excerpt,
           after_excerpt = excluded.after_excerpt,
           created_at = datetime('now')",
        params![draft_key, mapping_id, style_key, excerpt, before_excerpt, after_excerpt],
    )?;
    Ok(StyleDraftRecord {
        draft_key,
        mapping_id: mapping_id.to_owned(),
        style_key: style_key.to_owned(),
        excerpt: excerpt.to_owned(),
        before_excerpt: before_excerpt.to_owned(),
        after_excerpt: after_excerpt.to_owned(),
        created_at: String::new(),
    })
}

pub fn list_style_drafts(dir: &Path) -> anyhow::Result<Vec<StyleDraftRecord>> {
    let c = open_history(dir)?;
    let mut statement = c.prepare(
        "SELECT draft_key, mapping_id, style_key, excerpt, before_excerpt, after_excerpt, created_at
         FROM style_drafts ORDER BY created_at DESC, draft_key ASC",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(StyleDraftRecord {
            draft_key: row.get(0)?,
            mapping_id: row.get(1)?,
            style_key: row.get(2)?,
            excerpt: row.get(3)?,
            before_excerpt: row.get(4)?,
            after_excerpt: row.get(5)?,
            created_at: row.get(6)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn get_style_draft(dir: &Path, draft_key: &str) -> anyhow::Result<Option<StyleDraftRecord>> {
    let c = open_history(dir)?;
    Ok(c.query_row(
        "SELECT draft_key, mapping_id, style_key, excerpt, before_excerpt, after_excerpt, created_at
         FROM style_drafts WHERE draft_key = ?1",
        [draft_key],
        |row| {
            Ok(StyleDraftRecord {
                draft_key: row.get(0)?,
                mapping_id: row.get(1)?,
                style_key: row.get(2)?,
                excerpt: row.get(3)?,
                before_excerpt: row.get(4)?,
                after_excerpt: row.get(5)?,
                created_at: row.get(6)?,
            })
        },
    )
    .optional()?)
}

pub fn delete_style_draft(dir: &Path, draft_key: &str) -> anyhow::Result<bool> {
    let c = open_history(dir)?;
    c.execute("DELETE FROM style_drafts WHERE draft_key = ?1", [draft_key])?;
    Ok(c.changes() > 0)
}

#[cfg(test)]
pub fn clear_all_data(dir: &Path) -> anyhow::Result<()> {
    let guard = begin_clear_all_data(dir)?;
    clear_all_data_under_guard(dir, &guard)
}

/// Clear data after admission has already been acquired. This runs on the
/// retained spool I/O owner in production; no registry guard crosses the
/// filesystem or database work.
pub(crate) fn clear_all_data_under_guard(
    dir: &Path,
    guard: &SpoolClearGuard,
) -> anyhow::Result<()> {
    if guard.data_root != normalized_data_root(dir) {
        anyhow::bail!("data clear lease does not own this directory");
    }
    let _commit = try_spool_io_commit(dir)?;
    let c = open_history(dir)?;
    c.execute_batch("DELETE FROM dictations; DELETE FROM learn_observations; DELETE FROM usage; DELETE FROM learn_pairs; DELETE FROM style_drafts; DELETE FROM learned_term_usage;")?;
    let vacuum_result = c.execute_batch("VACUUM");
    drop(c);
    // Run after open_history: clearing a legacy database can itself create a
    // rollback copy, which must not retain the very text the user cleared.
    prune_history_migration_backups(dir, None, SystemTime::now())?;
    let spool = dir.join("spool");
    if spool.exists() {
        fs::remove_dir_all(&spool)?;
    }
    ensure_private_dir(&spool)?;
    let gold = gold_root(dir);
    if gold.exists() {
        fs::remove_dir_all(&gold)?;
    }
    let pending = pending_history_root(dir);
    if pending.exists() {
        fs::remove_dir_all(&pending)?;
    }
    if let Err(error) = vacuum_result {
        return Err(error.into());
    }
    Ok(())
}

pub fn export_history_json(dir: &Path) -> anyhow::Result<String> {
    let mut items = Vec::new();
    let mut before_id = None;
    loop {
        let page = get_history_page(dir, 100, before_id, None)?;
        if let Some(last) = page.items.last() {
            before_id = Some(last.id);
        }
        let done = !page.has_more;
        items.extend(page.items);
        if done {
            break;
        }
    }
    Ok(serde_json::to_string_pretty(&items)?)
}

pub fn write_export_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        anyhow::bail!("invalid export path");
    }
    write_atomic_bytes_with_parent_mode(path, contents.as_bytes(), false)
}
pub fn failed_spool(dir: &Path, id: i64) -> anyhow::Result<Option<String>> {
    let c = open_history(dir)?;
    let path: Option<String> = c
        .query_row(
            "SELECT raw_audio_path FROM dictations WHERE id=? AND status IN ('failed','degraded')",
            [id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let root = dir.join("spool");
    Ok(path.filter(|value| {
        let path = Path::new(value);
        is_safe_spool_path(&root, path) && path.is_file()
    }))
}
#[derive(Debug, Clone, Default)]
pub struct HistoryScene {
    pub policy: Option<crate::context::ContextPolicy>,
    pub profile_id: Option<String>,
    pub family: Option<String>,
    pub browser_host: Option<String>,
    pub native_bundle: Option<String>,
}

pub(crate) struct ManualHistorySave {
    pub revision_id: Option<i64>,
    /// The adopted text immediately before this manual revision. For an
    /// unchanged retry of the latest manual revision, this is reconstructed
    /// from its predecessor so learning can reuse the same origin receipt.
    pub learning_before_text: String,
    pub scene: HistoryScene,
    pub changed: bool,
}

pub(crate) struct RecleanHistorySave<'a> {
    pub dictation_id: i64,
    pub final_text: &'a str,
    pub cleanup_status: &'a str,
    pub intent: &'a crate::llm::CleanupIntent,
    pub model: &'a str,
    pub context_policy: &'a crate::context::ContextPolicy,
    pub cleanup_rejected_candidate: Option<&'a str>,
    pub cleanup_guard_reason: Option<&'a str>,
    pub cleanup_attempt_kind: Option<&'a str>,
    pub cleanup_transformations: Option<&'a [String]>,
    pub cleanup_final_guard_reason: Option<&'a str>,
    pub cleanup_failure_reason: Option<&'a str>,
}

impl HistoryScene {
    pub fn learn_scope(&self) -> LearnPairScope {
        LearnPairScope {
            family: self.family.clone(),
            mapping_id: self
                .profile_id
                .as_deref()
                .and_then(|id| id.strip_prefix("user."))
                .map(str::to_owned),
            browser_host: self.browser_host.clone(),
            native_bundle: self.native_bundle.clone(),
        }
    }
}

#[cfg(test)]
pub fn history_context(
    dir: &Path,
    id: i64,
) -> anyhow::Result<Option<crate::context::ContextPolicy>> {
    Ok(history_scene(dir, id)?.policy)
}

pub fn history_scene(dir: &Path, id: i64) -> anyhow::Result<HistoryScene> {
    let c = open_history(dir)?;
    let scene = c
        .query_row(
            "SELECT context_policy_json, context_profile_id, context_family, context_browser_host, context_native_bundle FROM dictations WHERE id=?",
            [id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((policy_json, profile_id, family, browser_host, native_bundle)) = scene else {
        return Ok(HistoryScene::default());
    };
    Ok(HistoryScene {
        policy: policy_json
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
        profile_id,
        family,
        browser_host,
        native_bundle,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn save_history_revision(
    dir: &Path,
    dictation_id: i64,
    final_text: &str,
    cleanup_status: Option<&str>,
    intent: Option<&crate::llm::CleanupIntent>,
    model: Option<&str>,
    context_policy: Option<&crate::context::ContextPolicy>,
    revision_reason: &str,
) -> anyhow::Result<i64> {
    if final_text.trim().is_empty() {
        anyhow::bail!("history revision cannot be empty");
    }
    if revision_reason.trim().is_empty() || revision_reason.len() > 128 {
        anyhow::bail!("history revision reason is invalid");
    }
    let c = open_history(dir)?;
    let intent_json = intent.map(serde_json::to_string).transpose()?;
    let context_policy_json = context_policy
        .map(crate::context::ContextPolicy::history_metadata)
        .map(|policy| serde_json::to_string(&policy))
        .transpose()?;
    c.execute(
        "INSERT INTO dictation_revisions (dictation_id,created_at,final_text,cleanup_status,intent_json,model,context_policy_json,revision_reason) SELECT ?,datetime('now'),?,?,?,?,?,? WHERE EXISTS (SELECT 1 FROM dictations WHERE id=?)",
        params![
            dictation_id,
            final_text,
            cleanup_status,
            intent_json,
            model,
            context_policy_json,
            revision_reason,
            dictation_id
        ],
    )?;
    if c.changes() == 0 {
        anyhow::bail!("history record was not found");
    }
    c.execute(
        "UPDATE dictations SET cleanup_rejected_candidate=NULL, cleanup_guard_reason=NULL, cleanup_attempt_kind=NULL, cleanup_transformations_json=NULL, cleanup_final_guard_reason=NULL, cleanup_failure_reason=NULL WHERE id=?",
        [dictation_id],
    )?;
    Ok(c.last_insert_rowid())
}

/// Saves an explicit History edit while reading the prior adopted text and
/// captured scene under the same SQLite write transaction. A repeated save of
/// the current text is a no-op, so it cannot create a new learning event.
pub(crate) fn save_manual_history_revision(
    dir: &Path,
    dictation_id: i64,
    final_text: &str,
) -> anyhow::Result<ManualHistorySave> {
    if final_text.trim().is_empty() {
        anyhow::bail!("history revision cannot be empty");
    }
    let mut c = open_history(dir)?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let row = tx
        .query_row(
            "SELECT COALESCE(NULLIF((SELECT final_text FROM dictation_revisions WHERE dictation_id=d.id ORDER BY revision_id DESC LIMIT 1),''),NULLIF(d.final_text,''),COALESCE(d.raw_text,'')), (SELECT revision_id FROM dictation_revisions WHERE dictation_id=d.id ORDER BY revision_id DESC LIMIT 1), (SELECT revision_reason FROM dictation_revisions WHERE dictation_id=d.id ORDER BY revision_id DESC LIMIT 1), COALESCE(NULLIF((SELECT final_text FROM dictation_revisions previous WHERE previous.dictation_id=d.id AND previous.revision_id < (SELECT revision_id FROM dictation_revisions latest WHERE latest.dictation_id=d.id ORDER BY latest.revision_id DESC LIMIT 1) ORDER BY previous.revision_id DESC LIMIT 1),''),NULLIF(d.final_text,''),COALESCE(d.raw_text,'')), d.context_policy_json, d.context_profile_id, d.context_family, d.context_browser_host, d.context_native_bundle FROM dictations d WHERE d.id=?1",
            [dictation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("history record was not found"))?;
    let (
        previous_text,
        previous_revision_id,
        previous_reason,
        retry_before_text,
        policy_json,
        profile_id,
        family,
        browser_host,
        native_bundle,
    ) = row;
    let scene = HistoryScene {
        policy: policy_json
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
        profile_id,
        family,
        browser_host,
        native_bundle,
    };
    if previous_text == final_text {
        tx.commit()?;
        let revision_id =
            previous_revision_id.filter(|_| previous_reason.as_deref() == Some("manual_edit"));
        return Ok(ManualHistorySave {
            revision_id,
            learning_before_text: if revision_id.is_some() {
                retry_before_text
            } else {
                previous_text
            },
            scene,
            changed: false,
        });
    }
    tx.execute(
        "INSERT INTO dictation_revisions (dictation_id,created_at,final_text,cleanup_status,intent_json,model,context_policy_json,revision_reason) VALUES (?1,datetime('now'),?2,NULL,NULL,NULL,NULL,'manual_edit')",
        params![dictation_id, final_text],
    )?;
    let revision_id = tx.last_insert_rowid();
    tx.execute(
        "UPDATE dictations SET cleanup_rejected_candidate=NULL, cleanup_guard_reason=NULL, cleanup_attempt_kind=NULL, cleanup_transformations_json=NULL, cleanup_final_guard_reason=NULL, cleanup_failure_reason=NULL WHERE id=?1",
        [dictation_id],
    )?;
    tx.commit()?;
    Ok(ManualHistorySave {
        revision_id: Some(revision_id),
        learning_before_text: previous_text,
        scene,
        changed: true,
    })
}

fn history_revision_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryRevision> {
    let intent_json: Option<String> = row.get(5)?;
    let context_policy_json: Option<String> = row.get(7)?;
    Ok(HistoryRevision {
        revision_id: row.get(0)?,
        dictation_id: row.get(1)?,
        created_at: row.get(2)?,
        final_text: row.get(3)?,
        cleanup_status: row.get(4)?,
        intent: intent_json.and_then(|value| serde_json::from_str(&value).ok()),
        model: row.get(6)?,
        context_policy: context_policy_json.and_then(|value| serde_json::from_str(&value).ok()),
        revision_reason: row.get(8)?,
    })
}

const HISTORY_REVISION_COLUMNS: &str =
    "revision_id,dictation_id,created_at,final_text,cleanup_status,intent_json,model,context_policy_json,revision_reason";

pub fn get_history_revisions(
    dir: &Path,
    dictation_id: i64,
) -> anyhow::Result<Vec<HistoryRevision>> {
    let c = open_history(dir)?;
    let mut statement = c.prepare(&format!(
        "SELECT {HISTORY_REVISION_COLUMNS} FROM dictation_revisions WHERE dictation_id=? ORDER BY revision_id ASC"
    ))?;
    let rows = statement.query_map([dictation_id], history_revision_from_row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn get_history_revision(
    dir: &Path,
    dictation_id: i64,
    revision_id: i64,
) -> anyhow::Result<Option<HistoryRevision>> {
    let c = open_history(dir)?;
    let mut statement = c.prepare(&format!(
        "SELECT {HISTORY_REVISION_COLUMNS} FROM dictation_revisions WHERE dictation_id=? AND revision_id=?"
    ))?;
    statement
        .query_row(
            params![dictation_id, revision_id],
            history_revision_from_row,
        )
        .optional()
        .map_err(Into::into)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryDelivery {
    Copied,
    ClipboardChanged,
    ClipboardOwnershipUnverified,
    ClipboardWriteFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecleanDelivery {
    Copied,
    ClipboardChanged,
    ClipboardOwnershipUnverified,
    ClipboardWriteFailed,
}

impl RecleanDelivery {
    fn persistence(self) -> (&'static str, &'static str) {
        match self {
            Self::Copied => ("clipboard", "reclean_clipboard_only"),
            Self::ClipboardChanged => ("history", "reclean_clipboard_changed"),
            Self::ClipboardOwnershipUnverified => {
                ("history", "reclean_clipboard_ownership_unverified")
            }
            Self::ClipboardWriteFailed => ("history", "reclean_clipboard_write_failed"),
        }
    }
}

impl RetryDelivery {
    fn persistence(self) -> (&'static str, &'static str) {
        match self {
            Self::Copied => ("clipboard", "retry_clipboard_only"),
            Self::ClipboardChanged => ("history", "retry_clipboard_changed"),
            Self::ClipboardOwnershipUnverified => {
                ("history", "retry_clipboard_ownership_unverified")
            }
            Self::ClipboardWriteFailed => ("history", "retry_clipboard_write_failed"),
        }
    }
}

// Keep the persisted transcript, provider-original transcript, and delivery
// status explicit at this history boundary.
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub fn mark_retried_with_texts(
    dir: &Path,
    id: i64,
    raw_text: Option<&str>,
    asr_text: Option<&str>,
    engine: Option<&str>,
    final_text: &str,
    degraded: bool,
    degraded_reason: Option<&str>,
    cleanup_status: Option<&str>,
    delivery: RetryDelivery,
) -> anyhow::Result<()> {
    mark_retried_with_texts_and_cleanup_rejection(
        dir,
        id,
        raw_text,
        asr_text,
        engine,
        final_text,
        degraded,
        degraded_reason,
        cleanup_status,
        delivery,
        None,
        None,
        None,
        None,
        None,
        None,
    )
}

pub fn save_reclean_revision_with_facts(
    dir: &Path,
    request: RecleanHistorySave<'_>,
) -> anyhow::Result<i64> {
    let RecleanHistorySave {
        dictation_id,
        final_text,
        cleanup_status,
        intent,
        model,
        context_policy,
        cleanup_rejected_candidate,
        cleanup_guard_reason,
        cleanup_attempt_kind,
        cleanup_transformations,
        cleanup_final_guard_reason,
        cleanup_failure_reason,
    } = request;
    if final_text.trim().is_empty() {
        anyhow::bail!("history revision cannot be empty");
    }
    let mut c = open_history(dir)?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let intent_json = serde_json::to_string(intent)?;
    let context_policy_json = serde_json::to_string(&context_policy.history_metadata())?;
    tx.execute(
        "INSERT INTO dictation_revisions (dictation_id,created_at,final_text,cleanup_status,intent_json,model,context_policy_json,revision_reason) SELECT ?,datetime('now'),?,?,?,?,?,? WHERE EXISTS (SELECT 1 FROM dictations WHERE id=?)",
        params![dictation_id, final_text, cleanup_status, intent_json, model, context_policy_json, "ai_reclean", dictation_id],
    )?;
    if tx.changes() == 0 {
        anyhow::bail!("history record was not found");
    }
    let revision_id = tx.last_insert_rowid();
    let transformations_json = cleanup_transformations
        .map(serde_json::to_string)
        .transpose()?;
    tx.execute(
        "UPDATE dictations SET cleanup_rejected_candidate=?,cleanup_guard_reason=?,cleanup_attempt_kind=?,cleanup_transformations_json=?,cleanup_final_guard_reason=?,cleanup_failure_reason=? WHERE id=?",
        params![cleanup_rejected_candidate, cleanup_guard_reason, cleanup_attempt_kind, transformations_json, cleanup_final_guard_reason, cleanup_failure_reason, dictation_id],
    )?;
    tx.commit()?;
    Ok(revision_id)
}

#[allow(clippy::too_many_arguments)]
pub fn mark_retried_with_texts_and_cleanup_rejection(
    dir: &Path,
    id: i64,
    raw_text: Option<&str>,
    asr_text: Option<&str>,
    engine: Option<&str>,
    final_text: &str,
    degraded: bool,
    degraded_reason: Option<&str>,
    cleanup_status: Option<&str>,
    delivery: RetryDelivery,
    cleanup_rejected_candidate: Option<&str>,
    cleanup_guard_reason: Option<&str>,
    cleanup_attempt_kind: Option<&str>,
    cleanup_transformations: Option<&[String]>,
    cleanup_final_guard_reason: Option<&str>,
    cleanup_failure_reason: Option<&str>,
) -> anyhow::Result<()> {
    let mut c = open_history(dir)?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let (delivery_method, fallback_reason) = delivery.persistence();
    let status = if degraded {
        "degraded"
    } else if delivery == RetryDelivery::Copied {
        "copied"
    } else {
        "history"
    };
    tx.execute(
        "INSERT INTO dictation_revisions (dictation_id,created_at,final_text,cleanup_status,revision_reason) SELECT ?,datetime('now'),?,?,? WHERE EXISTS (SELECT 1 FROM dictations WHERE id=?)",
        params![id, final_text, cleanup_status, "retry", id],
    )?;
    if tx.changes() == 0 {
        anyhow::bail!("history record was not found");
    }
    let transformations_json = cleanup_transformations
        .map(serde_json::to_string)
        .transpose()?;
    tx.execute(
        "UPDATE dictations SET status=?, degraded=?, degraded_reason=?, raw_text=COALESCE(?,raw_text), asr_text=COALESCE(?,asr_text), engine=COALESCE(?,engine), cleanup_status=COALESCE(?,cleanup_status), provider_cleaned_candidate=NULL, cleanup_rejected_candidate=?, cleanup_guard_reason=?, cleanup_attempt_kind=?, cleanup_transformations_json=?, cleanup_final_guard_reason=?, cleanup_failure_reason=?, raw_audio_path=NULL, delivery_method=?, fallback_reason=?, delivery_error_code=NULL, delivery_user_reason=NULL WHERE id=?",
        params![
            status,
            degraded as i32,
            degraded_reason,
            raw_text,
            asr_text,
            engine,
            cleanup_status,
            cleanup_rejected_candidate,
            cleanup_guard_reason,
            cleanup_attempt_kind,
            transformations_json,
            cleanup_final_guard_reason,
            cleanup_failure_reason,
            delivery_method,
            fallback_reason,
            id
        ],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn record_recovered_history(dir: &Path, recovery: &RecoveredSpool) -> anyhow::Result<()> {
    let _mutation = begin_spool_root_mutation(dir)?;
    let spool_root = dir.join("spool");
    let gold_root = gold_root(dir);
    let (owner_path, expected_kind) = if is_safe_spool_path(&spool_root, &recovery.audio_path) {
        let owner_path = spool_session_key_for_artifact(&recovery.audio_path)
            .ok_or_else(|| anyhow::anyhow!("invalid recovered spool path"))?;
        let kind = spool_lease_kind_for_artifact(&recovery.audio_path, &owner_path);
        (owner_path, kind)
    } else if is_safe_spool_path(&gold_root, &recovery.audio_path) {
        let owner_path = spool_session_key_for_artifact(&recovery.audio_path)
            .ok_or_else(|| anyhow::anyhow!("invalid recovered artifact path"))?;
        (owner_path, SpoolLeaseKind::Artifact)
    } else {
        anyhow::bail!("recovered audio is outside the owned storage roots");
    };
    let lease = wait_for_history_delete_spool_lease(dir, &recovery.audio_path)?;
    if lease.inner.kind != expected_kind {
        anyhow::bail!("recovered audio owner kind changed during admission");
    }
    let _owner_commit = try_spool_io_commit_audio_session(&lease)?;
    match expected_kind {
        SpoolLeaseKind::Session => {
            let root_metadata = fs::symlink_metadata(&spool_root)?;
            if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
                anyhow::bail!("recovered spool root has an unsafe file type");
            }
            let metadata = fs::symlink_metadata(&owner_path)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                anyhow::bail!("recovered session has an unsafe owner directory");
            }
            let session_id = owner_path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| anyhow::anyhow!("recovered session id is invalid"))?;
            if !valid_spool_session_id(session_id)
                || recovery.audio_path != owner_path.join("recovery.wav")
            {
                anyhow::bail!("recovered session path is invalid");
            }
            let manifest = load_manifest(&owner_path)?;
            if manifest.schema_version != SPOOL_MANIFEST_VERSION
                || manifest.session_id != session_id
                || !matches!(
                    manifest.status.as_str(),
                    "active" | "recoverable" | "degraded"
                )
            {
                anyhow::bail!("recovered session is no longer eligible for History");
            }
        }
        SpoolLeaseKind::Artifact => {
            let managed_root = if recovery.audio_path.starts_with(&spool_root) {
                &spool_root
            } else {
                &gold_root
            };
            let root_metadata = fs::symlink_metadata(managed_root)?;
            if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
                anyhow::bail!("recovered artifact root has an unsafe file type");
            }
            let metadata = fs::symlink_metadata(&recovery.audio_path)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                anyhow::bail!("recovered artifact has an unsafe file type");
            }
        }
    }
    let artifact_metadata = fs::symlink_metadata(&recovery.audio_path)?;
    if !artifact_metadata.is_file() || artifact_metadata.file_type().is_symlink() {
        anyhow::bail!("recovered audio has an unsafe file type");
    }
    let _history_commit = try_history_database_commit_with_wait(dir)?;
    let mut c = open_history(dir)?;
    let transaction = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let already_recorded: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM dictations WHERE raw_audio_path=?",
        [recovery.audio_path.to_string_lossy().as_ref()],
        |row| row.get(0),
    )?;
    if already_recorded > 0 {
        transaction.commit()?;
        return Ok(());
    }
    let context = crate::context::ContextSnapshot::general();
    insert_history_with_status_and_candidate_on_connection(
        &transaction,
        InsertHistory {
            dir,
            raw: "",
            asr_text: None,
            engine: None,
            final_text: "",
            cleanup_status: None,
            duration: recovery.duration_secs,
            degraded: true,
            degraded_reason: Some("interrupted_recording"),
            status: "failed".into(),
            delivery_method: Some("none"),
            fallback_reason: Some("crash_recovery"),
            spool: Some(&recovery.audio_path),
            count_asr: false,
            count_llm: false,
            context: Some(&context),
            cleanup_attempt_kind: None,
            cleanup_transformations_json: None,
            cleanup_final_guard_reason: None,
            cleanup_failure_reason: None,
        },
        None,
        None,
        None,
        None,
    )?;
    transaction.commit()?;
    Ok(())
}

pub fn remove_spool_artifact(dir: &Path, path: &Path) {
    remove_spool_artifact_inner(dir, path, false);
}

fn remove_spool_artifact_inner(dir: &Path, path: &Path, root_commit_held: bool) {
    let spool_root = dir.join("spool");
    if is_safe_spool_path(&spool_root, path) {
        let Some(key) = spool_session_key_for_artifact(path) else {
            return;
        };
        let lease_kind = spool_lease_kind_for_artifact(path, &key);
        let lease = match reserve_spool_session_path(&key, false, lease_kind) {
            Ok(lease) => lease,
            Err(_) => {
                defer_spool_artifact_removal_until_release(path);
                return;
            }
        };
        let _commit = if root_commit_held {
            None
        } else {
            match try_spool_io_commit_audio_session(&lease) {
                Ok(commit) => Some(commit),
                Err(_) => return,
            }
        };
        if !spool_lease_can_write(&lease) {
            return;
        }
        if path.is_dir() {
            let _ = fs::remove_dir_all(path);
            return;
        }
        let _ = fs::remove_file(path);
        if let Some(parent) = path.parent() {
            if parent != spool_root && parent.join("manifest.json").is_file() {
                let _ = fs::remove_dir_all(parent);
            }
        }
        return;
    }
    if is_safe_spool_path(&gold_root(dir), path) && path.is_file() {
        let Some(key) = spool_session_key_for_artifact(path) else {
            return;
        };
        let lease = match reserve_spool_session_path(&key, false, SpoolLeaseKind::Artifact) {
            Ok(lease) => lease,
            Err(_) => return,
        };
        let _commit = if root_commit_held {
            None
        } else {
            match try_spool_io_commit_audio_session(&lease) {
                Ok(commit) => Some(commit),
                Err(_) => return,
            }
        };
        if spool_lease_can_write(&lease) {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
pub fn history_audio_bytes(dir: &Path, id: i64) -> anyhow::Result<Vec<u8>> {
    let c = open_history(dir)?;
    let path: Option<String> = c
        .query_row(
            "SELECT raw_audio_path FROM dictations WHERE id=?",
            [id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let path = path.ok_or_else(|| anyhow::anyhow!("history audio is no longer available"))?;
    let path = Path::new(&path);
    if !managed_audio_exists(dir, path) {
        anyhow::bail!("history audio is no longer available");
    }
    read_spool_file(path)
}

#[cfg(test)]
pub fn save_verbatim(dir: &Path, id: i64, text: &str, reviewed: bool) -> anyhow::Result<()> {
    let trimmed = text.trim();
    if reviewed && trimmed.is_empty() {
        anyhow::bail!("reviewed verbatim text cannot be empty");
    }
    let stored = if reviewed || !trimmed.is_empty() {
        Some(trimmed)
    } else {
        None
    };
    let c = open_history(dir)?;
    c.execute(
        "UPDATE dictations SET verbatim_text=?, verbatim_reviewed=? WHERE id=?",
        params![stored, reviewed as i32, id],
    )?;
    if c.changes() == 0 {
        anyhow::bail!("history record was not found");
    }
    Ok(())
}

pub fn qwen_gold_language_tag(language: &str) -> &'static str {
    match language {
        "zh" => "Chinese",
        "en" => "English",
        _ => "None",
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GoldExport {
    pub directory: String,
    pub jsonl_path: String,
    pub count: usize,
}

pub fn export_gold_corpus(
    dir: &Path,
    downloads: &Path,
    language: &str,
) -> anyhow::Result<GoldExport> {
    if downloads.as_os_str().is_empty() || !downloads.is_absolute() {
        anyhow::bail!("invalid export path");
    }
    let c = open_history(dir)?;
    let mut statement = c.prepare(
        "SELECT id, COALESCE(raw_text,''), raw_audio_path FROM dictations WHERE raw_audio_path IS NOT NULL ORDER BY id ASC",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    drop(c);

    let language_tag = qwen_gold_language_tag(language);
    let export_dir = downloads.join(format!("voiceflow-gold-{}", now_ms()));
    ensure_private_dir(&export_dir)?;
    let jsonl_path = export_dir.join("corpus.jsonl");
    let mut jsonl = String::new();
    let mut count = 0usize;
    let gold = gold_root(dir);
    for (id, raw_text, audio_path) in rows {
        let text = raw_text.trim();
        if text.is_empty() {
            continue;
        }
        let Some(audio_path) = audio_path else {
            continue;
        };
        let source = Path::new(&audio_path);
        if !is_safe_spool_path(&gold, source) || !source.is_file() {
            continue;
        }
        let wav = match read_spool_file(source) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!("skipping gold export for history {id}: {error}");
                continue;
            }
        };
        count += 1;
        let file_name = format!("utt{count:04}.wav");
        let dest = export_dir.join(&file_name);
        write_atomic_bytes(&dest, &wav)?;
        let line = serde_json::json!({
            "audio": dest.to_string_lossy(),
            "text": format!("language {language_tag}<asr_text>{text}"),
        });
        jsonl.push_str(&line.to_string());
        jsonl.push('\n');
    }
    if count == 0 {
        let _ = fs::remove_dir_all(&export_dir);
        anyhow::bail!("没有可导出的训练音频。");
    }
    write_atomic_bytes(&jsonl_path, jsonl.as_bytes())?;
    Ok(GoldExport {
        directory: export_dir.to_string_lossy().into_owned(),
        jsonl_path: jsonl_path.to_string_lossy().into_owned(),
        count,
    })
}

pub fn purge_gold_audio(dir: &Path, keep_audio_days: u64) -> anyhow::Result<usize> {
    let _mutation = begin_spool_root_mutation(dir)?;
    // The operation reads the gold tree and updates History rows whose audio
    // references expired, so both typed regions are part of its ownership.
    let _history_commit = try_spool_io_commit_scope(dir, SpoolIoScope::HistoryDatabase)?;
    let _commit = try_spool_io_commit_scope(dir, SpoolIoScope::GoldAudio)?;
    let root = gold_root(dir);
    if !root.exists() {
        return Ok(0);
    }
    let max_age = keep_audio_days.saturating_mul(86_400);
    let mut deleted = 0usize;
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let age = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(max_age.saturating_add(1));
        if (keep_audio_days == 0 || age > max_age) && fs::remove_file(&path).is_ok() {
            deleted += 1;
        }
    }
    let c = open_history(dir)?;
    let mut statement =
        c.prepare("SELECT id, raw_audio_path FROM dictations WHERE raw_audio_path IS NOT NULL")?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (id, path) in rows.into_iter() {
        let Some(path) = path else {
            continue;
        };
        let path = Path::new(&path);
        if is_safe_spool_path(&root, path) && !path.is_file() {
            c.execute("UPDATE dictations SET raw_audio_path=NULL WHERE id=?", [id])?;
        }
    }
    Ok(deleted)
}
pub fn history_text(dir: &Path, id: i64) -> anyhow::Result<String> {
    let c = open_history(dir)?;
    Ok(c.query_row(
        "SELECT COALESCE(NULLIF((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),''),NULLIF(final_text,''),raw_text,'') FROM dictations WHERE id=?",
        [id],
        |r| r.get(0),
    )?)
}

/// Return the provider's original ASR transcript for History re-cleanup.
/// Older rows intentionally remain unavailable when this nullable field is absent.
pub fn history_asr_text(dir: &Path, id: i64) -> anyhow::Result<Option<String>> {
    let c = open_history(dir)?;
    Ok(
        c.query_row("SELECT asr_text FROM dictations WHERE id=?", [id], |r| {
            r.get(0)
        })?,
    )
}

#[cfg(test)]
pub fn update_history_revision_state(
    dir: &Path,
    id: i64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    cleanup_status: Option<&str>,
) -> anyhow::Result<()> {
    let c = open_history(dir)?;
    c.execute(
        "UPDATE dictations SET status=?,degraded=?,degraded_reason=?,cleanup_status=COALESCE(?,cleanup_status),cleanup_rejected_candidate=?,cleanup_guard_reason=?,delivery_method='clipboard',fallback_reason=NULL,delivery_error_code=NULL,delivery_user_reason=NULL WHERE id=?",
        params![status, degraded as i32, degraded_reason, cleanup_status, Option::<&str>::None, Option::<&str>::None, id],
    )?;
    if c.changes() == 0 {
        anyhow::bail!("history record was not found");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn update_history_revision_state_with_reclean_delivery(
    dir: &Path,
    id: i64,
    degraded: bool,
    degraded_reason: Option<&str>,
    cleanup_status: Option<&str>,
    cleanup_rejected_candidate: Option<&str>,
    cleanup_guard_reason: Option<&str>,
    delivery: Option<RecleanDelivery>,
    cleanup_attempt_kind: Option<&str>,
    cleanup_transformations: Option<&[String]>,
    cleanup_final_guard_reason: Option<&str>,
    cleanup_failure_reason: Option<&str>,
) -> anyhow::Result<()> {
    let (status, delivery_method, fallback_reason) = if degraded {
        ("degraded", "history", None)
    } else if let Some(delivery) = delivery {
        let (method, reason) = delivery.persistence();
        (
            if delivery == RecleanDelivery::Copied {
                "copied"
            } else {
                "history"
            },
            method,
            Some(reason),
        )
    } else {
        ("history", "history", Some("reclean_copy_not_attempted"))
    };
    let c = open_history(dir)?;
    let transformations_json = cleanup_transformations
        .map(serde_json::to_string)
        .transpose()?;
    c.execute(
        "UPDATE dictations SET status=?,degraded=?,degraded_reason=?,cleanup_status=COALESCE(?,cleanup_status),cleanup_rejected_candidate=?,cleanup_guard_reason=?,cleanup_attempt_kind=?,cleanup_transformations_json=?,cleanup_final_guard_reason=?,cleanup_failure_reason=?,delivery_method=?,fallback_reason=?,delivery_error_code=NULL,delivery_user_reason=NULL WHERE id=?",
        params![status, degraded as i32, degraded_reason, cleanup_status, cleanup_rejected_candidate, cleanup_guard_reason, cleanup_attempt_kind, transformations_json, cleanup_final_guard_reason, cleanup_failure_reason, delivery_method, fallback_reason, id],
    )?;
    if c.changes() == 0 {
        anyhow::bail!("history record was not found");
    }
    Ok(())
}
pub fn delete_history(dir: &Path, id: i64) -> anyhow::Result<()> {
    // Keep the same admission order as recovery: claim the data-root mutation,
    // wait for the exact spool owner to release its path, then claim History.
    // Waiting for a spool lease while holding History could deadlock a worker
    // that still needs to finish its History commit before releasing the lease.
    let _mutation = begin_spool_root_mutation(dir)?;
    let c = open_history(dir)?;
    let (spool, transaction_id) = c
        .query_row(
            "SELECT raw_audio_path, transaction_id FROM dictations WHERE id=?",
            [id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .optional()?
        .unwrap_or((None, None));
    let spool_path = spool.as_deref().map(Path::new);
    let _spool_lease = if let Some(path) = spool_path {
        Some(wait_for_history_delete_spool_lease(dir, path)?)
    } else {
        None
    };
    if let Some(transaction_id) = transaction_id.as_deref() {
        // A committed History row may still have its recovery intent if the
        // process stopped after the SQLite ACK. Remove that intent before
        // deleting the row so a later recovery pass cannot recreate the proof.
        remove_pending_history_intent(dir, transaction_id)?;
    }
    let _history_commit = try_history_database_commit_with_wait(dir)?;
    let mut connection = c;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if let Some(path) = spool_path {
        if is_safe_spool_path(&dir.join("spool"), path) {
            if let Some(lease) = _spool_lease.as_ref() {
                mark_spool_artifact_complete(dir, path, lease.inner.kind)?;
            }
        }
    }
    transaction.execute("DELETE FROM dictations WHERE id=?", [id])?;
    transaction.commit()?;
    if let Some(path) = spool_path {
        remove_spool_artifact_for_history_delete(dir, path)?;
    }
    Ok(())
}

fn wait_for_history_delete_spool_lease(
    dir: &Path,
    path: &Path,
) -> anyhow::Result<AudioSpoolSessionLease> {
    if is_safe_spool_path(&dir.join("spool"), path) {
        let key = spool_session_key_for_artifact(path)
            .ok_or_else(|| anyhow::anyhow!("invalid History spool artifact path"))?;
        let kind = spool_lease_kind_for_artifact(path, &key);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match reserve_spool_session_path(&key, false, kind) {
                Ok(lease) => return Ok(lease),
                Err(error) if Instant::now() < deadline => {
                    #[cfg(test)]
                    wait_for_history_delete_spool_test_gate(&key);
                    std::thread::sleep(Duration::from_millis(2));
                    drop(error);
                }
                Err(error) => return Err(error.context("History spool deletion timed out")),
            }
        }
    }
    if is_safe_spool_path(&gold_root(dir), path) {
        let key = spool_session_key_for_artifact(path)
            .ok_or_else(|| anyhow::anyhow!("invalid History gold artifact path"))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match reserve_spool_session_path(&key, false, SpoolLeaseKind::Artifact) {
                Ok(lease) => return Ok(lease),
                Err(error) if Instant::now() < deadline => {
                    #[cfg(test)]
                    wait_for_history_delete_spool_test_gate(&key);
                    std::thread::sleep(Duration::from_millis(2));
                    drop(error);
                }
                Err(error) => return Err(error.context("History artifact deletion timed out")),
            }
        }
    }
    anyhow::bail!("History references a spool artifact outside the owned storage roots")
}

fn mark_spool_artifact_complete(
    dir: &Path,
    path: &Path,
    lease_kind: SpoolLeaseKind,
) -> anyhow::Result<()> {
    if is_safe_spool_path(&dir.join("spool"), path) {
        let Some(session_dir) = spool_session_key_for_artifact(path) else {
            anyhow::bail!("invalid History spool session path");
        };
        if lease_kind == SpoolLeaseKind::Session {
            match fs::symlink_metadata(&session_dir) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Ok(_) => anyhow::bail!("History spool session has an unsafe file type"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.into()),
            }
        }
        let artifact_metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => Some(metadata),
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && lease_kind == SpoolLeaseKind::Artifact =>
            {
                return Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if artifact_metadata.as_ref().is_some_and(|metadata| {
            metadata.file_type().is_symlink()
                || (lease_kind == SpoolLeaseKind::Artifact && !metadata.is_file())
                || (lease_kind == SpoolLeaseKind::Session
                    && !metadata.is_file()
                    && !(path == session_dir && metadata.is_dir()))
        }) {
            anyhow::bail!("History spool artifact has an unsafe file type");
        }
        if lease_kind == SpoolLeaseKind::Artifact {
            return Ok(());
        }
        let manifest_metadata = fs::symlink_metadata(manifest_path(&session_dir))?;
        if !manifest_metadata.is_file() || manifest_metadata.file_type().is_symlink() {
            anyhow::bail!("History spool manifest has an unsafe file type");
        }
        let mut manifest = load_manifest(&session_dir)?;
        manifest.status = "complete".to_owned();
        save_manifest(&session_dir, &manifest)?;
    }
    Ok(())
}

fn remove_spool_artifact_for_history_delete(dir: &Path, path: &Path) -> anyhow::Result<()> {
    if is_safe_spool_path(&dir.join("spool"), path) || is_safe_spool_path(&gold_root(dir), path) {
        let owner_path = spool_session_key_for_artifact(path)
            .ok_or_else(|| anyhow::anyhow!("invalid History artifact owner path"))?;
        let manifest_path = owner_path.join("manifest.json");
        let remove_path = if fs::symlink_metadata(&manifest_path)
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        {
            owner_path
        } else {
            path.to_path_buf()
        };
        match fs::symlink_metadata(&remove_path) {
            Ok(metadata) if metadata.file_type().is_dir() => fs::remove_dir_all(&remove_path)?,
            Ok(metadata) if metadata.file_type().is_file() => fs::remove_file(&remove_path)?,
            Ok(_) => anyhow::bail!("History spool artifact has an unsafe file type"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    } else {
        anyhow::bail!("History spool artifact path changed during deletion")
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeCredentialBackend {
        values: RefCell<std::collections::BTreeMap<String, String>>,
        read_scripts: RefCell<
            std::collections::BTreeMap<
                String,
                std::collections::VecDeque<crate::keychain::ApiKeyState>,
            >,
        >,
        write_errors: RefCell<std::collections::BTreeMap<String, String>>,
        writes: RefCell<Vec<(String, String)>>,
    }

    impl FakeCredentialBackend {
        fn seed(&self, slot: &str, key: &str) {
            self.values
                .borrow_mut()
                .insert(slot.to_owned(), key.to_owned());
        }

        fn fail_write(&self, slot: &str, error: &str) {
            self.write_errors
                .borrow_mut()
                .insert(slot.to_owned(), error.to_owned());
        }

        fn script_reads(
            &self,
            slot: &str,
            states: impl IntoIterator<Item = crate::keychain::ApiKeyState>,
        ) {
            self.read_scripts
                .borrow_mut()
                .insert(slot.to_owned(), states.into_iter().collect());
        }
    }

    impl CredentialBackend for FakeCredentialBackend {
        fn read(&self, slot: &str) -> crate::keychain::ApiKeyState {
            if let Some(state) = self
                .read_scripts
                .borrow_mut()
                .get_mut(slot)
                .and_then(std::collections::VecDeque::pop_front)
            {
                return state;
            }
            let values = self.values.borrow();
            let configured = |slot: &str| {
                values
                    .get(slot)
                    .filter(|key| !key.trim().is_empty())
                    .cloned()
                    .map(crate::keychain::ApiKeyState::Configured)
            };
            if let Some(state) = configured(slot) {
                return state;
            }
            if slot == "provider_custom" {
                if let Some(state) =
                    configured("asr_api_key").or_else(|| configured("cleanup_api_key"))
                {
                    return state;
                }
            }
            crate::keychain::ApiKeyState::Missing
        }

        fn write(&self, slot: &str, key: &str) -> Result<(), String> {
            self.writes
                .borrow_mut()
                .push((slot.to_owned(), key.to_owned()));
            if let Some(error) = self.write_errors.borrow().get(slot) {
                return Err(error.clone());
            }
            if key.is_empty() {
                self.values.borrow_mut().remove(slot);
            } else {
                self.values
                    .borrow_mut()
                    .insert(slot.to_owned(), key.to_owned());
            }
            Ok(())
        }
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "voiceflow-store-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[cfg(unix)]
    #[test]
    fn export_preserves_selected_directory_permissions_and_restricts_file() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir("export-mode");
        fs::create_dir_all(&root).unwrap();
        for mode in [0o755, 0o750] {
            fs::set_permissions(&root, fs::Permissions::from_mode(mode)).unwrap();
            let output = root.join(format!("export-{mode:o}.json"));
            write_export_file(&output, "private export").unwrap();
            assert_eq!(
                fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                mode
            );
            assert_eq!(
                fs::metadata(&output).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(fs::read_to_string(&output).unwrap(), "private export");
            fs::set_permissions(&output, fs::Permissions::from_mode(0o644)).unwrap();
            write_export_file(&output, "replacement").unwrap();
            assert_eq!(
                fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                mode
            );
            assert_eq!(
                fs::metadata(&output).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(fs::read_to_string(&output).unwrap(), "replacement");
            assert!(write_export_file(Path::new("relative-export.json"), "no").is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_settings_are_backed_up_before_first_persist_and_backup_is_stable() {
        let dir = temp_dir("malformed-settings");
        fs::create_dir_all(&dir).unwrap();
        let original = b"{ definitely not settings";
        fs::write(dir.join("settings.json"), original).unwrap();
        let (_loaded, needs_persist) =
            load_settings_with_backend(&dir, &FakeCredentialBackend::default());
        assert!(!needs_persist);
        let settings = Settings::default();
        persist_settings_without_credential_changes(&dir, &settings).unwrap();
        let backup = dir.join("settings.json.malformed.bak");
        assert_eq!(fs::read(&backup).unwrap(), original);
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        persist_settings_without_credential_changes(&dir, &settings).unwrap();
        assert_eq!(fs::read(&backup).unwrap(), original);
        fs::remove_dir_all(dir).unwrap();

        let reuse_dir = temp_dir("malformed-settings-backup-reuse");
        fs::create_dir_all(&reuse_dir).unwrap();
        let original = b"{ still malformed settings";
        fs::write(reuse_dir.join("settings.json"), original).unwrap();
        let backup = reuse_dir.join("settings.json.malformed.bak");
        fs::write(&backup, original).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o600)).unwrap();
        let backend = FakeCredentialBackend::default();
        let (_loaded, needs_persist) = load_settings_with_backend(&reuse_dir, &backend);
        assert!(!needs_persist);
        persist_settings_with_backend(&reuse_dir, &settings, &backend).unwrap();
        assert_eq!(fs::read(&backup).unwrap(), original);
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(backend.writes.borrow().is_empty());
        fs::remove_dir_all(reuse_dir).unwrap();

        let conflict_dir = temp_dir("malformed-settings-conflict");
        fs::create_dir_all(&conflict_dir).unwrap();
        let original = b"{ bad settings }";
        fs::write(conflict_dir.join("settings.json"), original).unwrap();
        fs::write(
            conflict_dir.join("settings.json.malformed.bak"),
            b"different existing backup",
        )
        .unwrap();
        assert!(persist_settings_without_credential_changes(&conflict_dir, &settings).is_err());
        assert_eq!(
            fs::read(conflict_dir.join("settings.json")).unwrap(),
            original
        );
        fs::remove_dir_all(conflict_dir).unwrap();
    }

    #[test]
    fn pending_history_scope_covers_quota_tree_but_is_disjoint_from_capture_and_history() {
        let root = temp_dir("pending-scope");
        let pending = try_spool_io_commit_scope(&root, SpoolIoScope::PendingHistoryIntent)
            .expect("the pending directory tree is admitted");
        assert!(
            try_spool_io_commit_scope(&root, SpoolIoScope::PendingHistoryIntent).is_err(),
            "distinct transaction identities still share the quota/enumeration scope"
        );
        let history = try_spool_io_commit_scope(&root, SpoolIoScope::HistoryDatabase)
            .expect("pending metadata does not lock the History database");
        let capture = try_spool_io_commit_scope(&root, SpoolIoScope::RecoverySpool)
            .expect("pending metadata does not lock active audio capture");
        assert!(
            try_spool_io_commit(&root).is_err(),
            "the compatibility root-wide permit still conflicts with every typed scope"
        );
        drop(capture);
        drop(history);
        drop(pending);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn live_spool_session_scopes_are_disjoint_and_still_conflict_with_root_maintenance() {
        let root = temp_dir("live-session-scopes");
        let (first_dir, first_lease) = begin_audio_spool_session(&root, "scope-first")
            .expect("first live session reserves its derived path");
        let (second_dir, second_lease) = begin_audio_spool_session(&root, "scope-second")
            .expect("second live session reserves a disjoint derived path");
        initialize_audio_spool_session(&first_lease).unwrap();
        initialize_audio_spool_session(&second_lease).unwrap();

        let first = try_spool_io_commit_audio_session(&first_lease)
            .expect("first live session owns its own path scope");
        let second = try_spool_io_commit_audio_session(&second_lease)
            .expect("another live session may use a disjoint path scope");
        assert!(try_spool_io_commit_audio_session(&first_lease).is_err());
        assert!(try_spool_io_commit_scope(&root, SpoolIoScope::RecoverySpool).is_err());
        assert!(try_spool_io_commit(&root).is_err());

        drop(second);
        drop(first);
        drop(second_lease);
        drop(first_lease);
        let _ = fs::remove_dir_all(root);
        assert!(!first_dir.join("manifest.json").exists());
        assert!(!second_dir.join("manifest.json").exists());
    }

    #[test]
    fn deferred_session_cleanup_keeps_exact_owner_without_restoring_write_access() {
        let root = temp_dir("deferred-session-cleanup-scope");
        let (session_dir, lease) = begin_audio_spool_session(&root, "cleanup-owned-session")
            .expect("reserve the live session");
        initialize_audio_spool_session(&lease).unwrap();

        assert!(defer_spool_artifact_removal_until_release(&session_dir));
        assert!(lease.deletion_requested());
        assert!(try_spool_io_commit_audio_session(&lease).is_err());
        let cleanup = try_spool_io_commit_audio_session_cleanup(&lease)
            .expect("the exact current lease keeps cleanup permission");
        drop(cleanup);
        delete_audio_spool_session(&lease).expect("cleanup deletes its exact session tree");
        assert!(!session_dir.exists());
        assert!(try_spool_io_commit_audio_session(&lease).is_err());
        assert!(reserve_spool_session_path(&session_dir, true, SpoolLeaseKind::Session).is_err());

        drop(lease);
        let (reused_dir, reused_lease) = begin_audio_spool_session(&root, "cleanup-owned-session")
            .expect("reuse waits for the cleanup owner to release its lease");
        assert_eq!(reused_dir, session_dir);
        initialize_audio_spool_session(&reused_lease).unwrap();
        drop(reused_lease);
        clear_all_data(&root).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn spool_quota_scan_tolerates_sibling_temp_rename_and_session_cleanup() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        let root = temp_dir("spool-quota-disappearing-entry");
        let (first_dir, first_lease) =
            begin_audio_spool_session(&root, "quota-first").expect("first session reserves a path");
        let (_second_dir, second_lease) = begin_audio_spool_session(&root, "quota-second")
            .expect("second session reserves a disjoint path");
        initialize_audio_spool_session(&first_lease).unwrap();
        initialize_audio_spool_session(&second_lease).unwrap();
        drop(first_lease);
        drop(second_lease);
        let quota_anchor = root.join("spool/quota-anchor.bin");
        File::create(&quota_anchor)
            .unwrap()
            .set_len(MAX_SPOOL_BYTES - 24 * 1024)
            .unwrap();

        let first_relative = Path::new("quota-first/rename-race.wav");
        let first_target = root.join("spool").join(first_relative);
        let first_temp = first_target.parent().unwrap().join(format!(
            ".{}.tmp-{}",
            first_target.file_name().unwrap().to_string_lossy(),
            std::process::id()
        ));
        let (rename_entered_tx, rename_entered_rx) = mpsc::channel();
        let (rename_release_tx, rename_release_rx) = mpsc::channel();
        pause_next_atomic_rename_for_test(&first_temp, rename_entered_tx, rename_release_rx);
        let first_root = root.clone();
        let first_writer = std::thread::spawn(move || {
            write_spool_file(&first_root, first_relative, &vec![0x25; 16 * 1024])
        });
        rename_entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("A owns its session scope and has a synced temporary file");

        let (scan_entered_tx, scan_entered_rx) = mpsc::channel();
        let (scan_release_tx, scan_release_rx) = mpsc::channel();
        pause_next_spool_size_walk_for_test(&first_temp, scan_entered_tx, scan_release_rx);
        let second_root = root.clone();
        let second_writer = std::thread::spawn(move || {
            write_spool_file(
                &second_root,
                Path::new("quota-second/parallel.wav"),
                &vec![0x50; 16 * 1024],
            )
        });
        scan_entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("B's real quota walk reaches A's temporary directory entry");

        rename_release_tx
            .send(())
            .expect("allow A's actual atomic rename");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !first_target.exists() {
            assert!(Instant::now() < deadline, "A's final file rename completes");
            std::thread::sleep(Duration::from_millis(1));
        }
        first_writer
            .join()
            .unwrap()
            .expect("A releases its reservation after the actual write returns");
        assert_eq!(
            recovery_spool_reserved_bytes_for_test(&root),
            0,
            "A's reservation can drop while B still owns the recursive scan"
        );
        scan_release_tx
            .send(())
            .expect("resume B's quota metadata lookup after A's temp entry vanished");
        let second_error = second_writer
            .join()
            .unwrap()
            .expect_err("B retains A's start-of-scan charge and does not over-admit quota");
        assert!(second_error
            .to_string()
            .contains("audio spool quota exceeded"));
        assert_eq!(fs::read(&first_target).unwrap().len(), 16 * 1024);
        let second_target = root.join("spool/quota-second/parallel.wav");
        assert!(!second_target.exists());
        fs::remove_file(&quota_anchor).unwrap();

        let (delete_scan_entered_tx, delete_scan_entered_rx) = mpsc::channel();
        let (delete_scan_release_tx, delete_scan_release_rx) = mpsc::channel();
        pause_next_spool_size_walk_for_test(
            &first_target,
            delete_scan_entered_tx,
            delete_scan_release_rx,
        );
        let second_root = root.clone();
        let second_writer = std::thread::spawn(move || {
            write_spool_file(
                &second_root,
                Path::new("quota-second/after-delete.wav"),
                &vec![0x75; 16 * 1024],
            )
        });
        delete_scan_entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("B's next actual quota walk pauses on A's existing file");
        let first_root = root.clone();
        let first_dir_for_delete = first_dir.clone();
        let deleter = std::thread::spawn(move || {
            remove_spool_artifact(&first_root, &first_dir_for_delete);
        });
        deleter.join().unwrap();
        assert!(!first_dir.exists(), "A's sibling session cleanup completes");
        delete_scan_release_tx
            .send(())
            .expect("resume B's quota scan after A's session disappeared");
        second_writer
            .join()
            .unwrap()
            .expect("B's capture artifact is not lost to A's scoped cleanup");
        assert_eq!(
            fs::read(root.join("spool/quota-second/after-delete.wav"))
                .unwrap()
                .len(),
            16 * 1024
        );

        clear_all_data(&root).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn quota_ledger_fences_same_root_scans_and_clears_markers_after_faults() {
        use std::sync::mpsc;
        use std::time::Duration;

        let root = temp_dir("spool-quota-ledger-fence");
        let spool_root = root.join("spool");
        fs::create_dir_all(&spool_root).unwrap();
        let scan_target = spool_root.join("scan-target.bin");
        fs::write(&scan_target, b"counted before reservation").unwrap();

        let (scan_entered_tx, scan_entered_rx) = mpsc::channel();
        let (scan_release_tx, scan_release_rx) = mpsc::channel();
        pause_next_spool_size_walk_for_test(&scan_target, scan_entered_tx, scan_release_rx);
        let first_root = spool_root.clone();
        let (first_result_tx, first_result_rx) = mpsc::channel();
        let first = std::thread::spawn(move || {
            first_result_tx
                .send(reserve_recovery_spool_bytes(&first_root, 1).map_err(|e| e.to_string()))
                .unwrap();
        });
        scan_entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the first reservation pauses in a real recursive scan");

        let (same_root_started_tx, same_root_started_rx) = mpsc::channel();
        let (same_root_result_tx, same_root_result_rx) = mpsc::channel();
        let same_root_spool = spool_root.clone();
        let same_root = std::thread::spawn(move || {
            same_root_started_tx.send(()).unwrap();
            same_root_result_tx
                .send(reserve_recovery_spool_bytes(&same_root_spool, 1).map_err(|e| e.to_string()))
                .unwrap();
        });
        same_root_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("a same-root waiter starts");
        let same_root_waited = matches!(
            same_root_result_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );

        let other_spool_root = temp_dir("spool-quota-other-root").join("spool");
        let (other_result_tx, other_result_rx) = mpsc::channel();
        let other_root = std::thread::spawn(move || {
            let result = reserve_recovery_spool_bytes(&other_spool_root, 1)
                .map(drop)
                .map_err(|error| error.to_string());
            other_result_tx.send(result).unwrap();
        });
        let other_progressed = other_result_rx.recv_timeout(Duration::from_secs(2));
        scan_release_tx.send(()).unwrap();
        let first_reservation = first_result_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .expect("first same-root reservation is admitted after its scan");
        let second_reservation = same_root_result_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .expect("the waiting same-root reservation follows the first admission");
        first.join().unwrap();
        same_root.join().unwrap();
        other_root.join().unwrap();
        other_progressed
            .expect("an unrelated root's quota scan is not serialized behind the paused scan")
            .expect("unrelated root reservation succeeds");
        assert!(
            same_root_waited,
            "no same-root reservation enters between scan start and admission"
        );
        assert_eq!(
            recovery_spool_reserved_bytes_for_test(&root),
            2,
            "the ledger includes both serialized same-root reservations"
        );
        drop(second_reservation);
        drop(first_reservation);

        inject_spool_quota_scan_fault_for_test(&spool_root, SpoolQuotaScanFault::Error);
        let error = reserve_recovery_spool_bytes(&spool_root, 1)
            .err()
            .expect("the injected scanner error is returned");
        assert!(error
            .to_string()
            .contains("injected spool quota scan failure"));
        drop(
            reserve_recovery_spool_bytes(&spool_root, 1)
                .expect("the scan marker clears after an I/O error"),
        );

        inject_spool_quota_scan_fault_for_test(&spool_root, SpoolQuotaScanFault::Panic);
        let panic_result = std::panic::catch_unwind(|| {
            let _ = reserve_recovery_spool_bytes(&spool_root, 1);
        });
        assert!(panic_result.is_err(), "the panic fault reaches the scanner");
        drop(
            reserve_recovery_spool_bytes(&spool_root, 1)
                .expect("the scan marker clears during unwinding"),
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn atomic_file_ack_requires_parent_directory_sync_after_rename() {
        let root = temp_dir("atomic-parent-sync");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("committed-but-unacked.bin");
        fail_next_atomic_parent_sync_for_test(&root);

        let error = write_atomic_bytes(&path, b"durable receipt requires directory sync")
            .expect_err("directory sync failure cannot produce a successful ACK");
        assert!(error
            .to_string()
            .contains("injected atomic parent directory sync failure"));
        assert_eq!(
            fs::read(&path).unwrap(),
            b"durable receipt requires directory sync",
            "rename may have happened, so the outcome is retained as unacknowledged"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pending_history_cancel_waits_for_paused_journal_and_blocks_late_promotion() {
        let root = temp_dir("pending-cancel-race");
        let intent = pending_history_intent_for_test(
            "dictation-test-cancel-race",
            "delivery_pending",
            "unknown",
            "cancelled before external delivery",
        );
        let transaction_id = intent.transaction_id.clone();
        let active_owner = register_pending_history_owner(&root, &transaction_id)
            .expect("reserve the live cancellation identity");
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        pause_next_pending_history_write_for_test(&root, entered_tx, release_rx);

        let writer_root = root.clone();
        let writer_owner = active_owner.clone();
        let writer = std::thread::spawn(move || {
            write_pending_history_intent(&writer_root, &intent, &writer_owner)
        });
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the intent writer pauses after staging fsync while it owns quota admission");

        let (cancel_tx, cancel_rx) = std::sync::mpsc::channel();
        let cancel_root = root.clone();
        let cancel_id = transaction_id.clone();
        let cancel_owner = active_owner.clone();
        let cancel = std::thread::spawn(move || {
            let result = cancel_pending_history_intent(&cancel_root, &cancel_id, &cancel_owner)
                .map_err(|error| error.to_string());
            let _ = cancel_tx.send(result);
        });
        assert!(matches!(
            cancel_rx.recv_timeout(Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));

        release_tx
            .send(())
            .expect("release staged journal installation");
        writer
            .join()
            .expect("journal writer thread exits")
            .expect("the paused atomic journal write completes");
        cancel.join().expect("cancellation owner exits");
        cancel_rx
            .try_recv()
            .expect("cancellation reports its durable tombstone and cleanup")
            .expect("known no-side-effect cancellation is durably recorded");

        assert_eq!(promote_all_pending_history_intents(&root).unwrap(), 0);
        assert!(get_history(&root, 10).unwrap().is_empty());
        assert!(!pending_history_path(&root, &transaction_id)
            .unwrap()
            .exists());
        assert!(!pending_history_root(&root).join(&transaction_id).exists());
        assert!(
            write_pending_history_intent(
                &root,
                &pending_history_intent_for_test(
                    &transaction_id,
                    "delivery_pending",
                    "unknown",
                    "must not return after cancellation",
                ),
                &active_owner,
            )
            .is_err(),
            "a late writer carrying the cancelled owner token cannot resurrect the intent"
        );
        clear_all_data(&root).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn history_recovery_text_uses_only_its_pending_intent_or_history_transaction() {
        let root = temp_dir("history-recovery-text");
        let transaction_id = "dictation-recovery-copy";
        let text = "Exact recovery text, with punctuation.";
        let intent =
            pending_history_intent_for_test(transaction_id, "delivery_pending", "unknown", text);
        let owner = register_pending_history_owner(&root, transaction_id)
            .expect("reserve the exact recovery transaction identity");
        write_pending_history_intent(&root, &intent, &owner)
            .expect("the transaction's original text is durably retained");

        assert_eq!(
            history_recovery_text_for_transaction(&root, transaction_id)
                .expect("read the exact retained pending text"),
            text
        );
        assert!(
            history_recovery_text_for_transaction(&root, "some-other-transaction").is_err(),
            "a recovery ticket cannot read another transaction's text"
        );

        promote_pending_history_intent(&root, transaction_id, &owner)
            .expect("promote the uncertain transaction exactly once");
        let rows = get_history(&root, 10).expect("the text has a durable History row");
        let row = rows.first().expect("the exact transaction promotes once");
        assert_eq!(row.status, "delivery_unconfirmed");
        assert_eq!(row.delivery_method.as_deref(), Some("unknown"));
        assert_eq!(row.final_text, text);
        assert_eq!(
            history_recovery_text_for_transaction(&root, transaction_id)
                .expect("read the exact acknowledged History text"),
            text
        );
        clear_all_data(&root).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pending_history_startup_promotion_is_unknown_idempotent_and_cancel_safe() {
        let root = temp_dir("pending-promotion");
        let mut interrupted = pending_history_intent_for_test(
            "dictation-test-interrupted",
            "delivery_pending",
            "unknown",
            "the text from an interrupted delivery",
        );
        let interrupted_owner = register_pending_history_owner(&root, &interrupted.transaction_id)
            .expect("reserve the interrupted delivery identity");
        write_pending_history_intent(&root, &interrupted, &interrupted_owner)
            .expect("durable pre-delivery intent");
        drop(interrupted_owner);
        let path = pending_history_path(&root, &interrupted.transaction_id).unwrap();
        let stage = path.parent().unwrap().join(".intent.pending");
        fs::rename(&path, &stage).expect("simulate crash after stage fsync but before rename");
        assert_eq!(promote_all_pending_history_intents(&root).unwrap(), 1);
        let rows = get_history(&root, 10).expect("interrupted intent becomes visible in History");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].raw_text, "the text from an interrupted delivery");
        assert_eq!(rows[0].status, "delivery_unconfirmed");
        assert_eq!(rows[0].delivery_method.as_deref(), Some("unknown"));
        assert_eq!(
            rows[0].fallback_reason.as_deref(),
            Some("delivery_state_unknown_after_restart")
        );

        // Replaying the stable transaction key cannot create a second row.
        interrupted.status = "ok".into();
        interrupted.delivery_method = "paste".into();
        let stale_owner = register_pending_history_owner(&root, &interrupted.transaction_id)
            .expect("reacquire the stable acknowledgement identity");
        write_pending_history_intent(&root, &interrupted, &stale_owner)
            .expect("stale acknowledgement sidecar");
        drop(stale_owner);
        assert_eq!(promote_all_pending_history_intents(&root).unwrap(), 1);
        assert_eq!(get_history(&root, 10).unwrap().len(), 1);

        let cancelled = pending_history_intent_for_test(
            "dictation-test-cancelled",
            "delivery_pending",
            "unknown",
            "do not resurrect this canceled text",
        );
        let cancelled_owner = register_pending_history_owner(&root, &cancelled.transaction_id)
            .expect("reserve the cancellation candidate identity");
        write_pending_history_intent(&root, &cancelled, &cancelled_owner)
            .expect("durable cancellation candidate");
        drop(cancelled_owner);
        let cancelled_path = pending_history_path(&root, &cancelled.transaction_id).unwrap();
        let cancelled_dir = cancelled_path.parent().unwrap();
        let mut marker = File::create(cancelled_dir.join(".delivery.cancelled")).unwrap();
        marker
            .write_all(cancelled.transaction_id.as_bytes())
            .unwrap();
        marker.sync_all().unwrap();
        File::open(cancelled_dir).unwrap().sync_all().unwrap();
        assert_eq!(promote_all_pending_history_intents(&root).unwrap(), 0);
        let rows = get_history(&root, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!cancelled_dir.exists());
        clear_all_data(&root).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pending_history_promotion_persists_observed_cleanup_provenance() {
        let root = temp_dir("pending-cleanup-provenance");
        let mut intent = pending_history_intent_for_test(
            "dictation-cleanup-provenance",
            "ok",
            "paste",
            "accepted transformed text",
        );
        intent.cleanup_attempt_kind = Some("success".into());
        intent.cleanup_transformations = Some(vec![
            "strip_internal_cleanup_metadata".into(),
            "restore_spoken_layout".into(),
            "restore_prepared_preamble".into(),
        ]);
        intent.cleanup_final_guard_reason = Some("final_protected_token_changed".into());
        let owner = register_pending_history_owner(&root, &intent.transaction_id)
            .expect("reserve the actual transaction identity");
        write_pending_history_intent(&root, &intent, &owner)
            .expect("persist the serialized History intent");

        promote_pending_history_intent(&root, &intent.transaction_id, &owner)
            .expect("promote the owned intent through the public store entry point");
        let page = get_history_page(&root, 10, None, None).expect("read the promoted row");
        assert_eq!(page.items.len(), 1);
        assert_eq!(
            page.items[0].cleanup_attempt_kind.as_deref(),
            Some("success")
        );
        assert_eq!(
            page.items[0].cleanup_transformations.as_deref(),
            Some(
                [
                    "strip_internal_cleanup_metadata".to_owned(),
                    "restore_spoken_layout".to_owned(),
                    "restore_prepared_preamble".to_owned(),
                ]
                .as_slice()
            )
        );
        assert_eq!(
            page.items[0].cleanup_final_guard_reason.as_deref(),
            Some("final_protected_token_changed")
        );
        let exported: serde_json::Value =
            serde_json::from_str(&export_history_json(&root).unwrap()).unwrap();
        assert_eq!(exported[0]["cleanup_attempt_kind"], "success");
        assert_eq!(
            exported[0]["cleanup_transformations"],
            serde_json::json!([
                "strip_internal_cleanup_metadata",
                "restore_spoken_layout",
                "restore_prepared_preamble"
            ])
        );

        drop(owner);
        assert_eq!(
            promote_all_pending_history_intents(&root).unwrap(),
            0,
            "the promoted intent is removed and cannot create a duplicate after reopening"
        );
        assert_eq!(
            get_history_page(&root, 10, None, None).unwrap().items.len(),
            1
        );
        clear_all_data(&root).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    fn write_spool_f32_chunk(root: &Path, session_id: &str, index: usize, samples: &[f32]) {
        let mut bytes = Vec::with_capacity(std::mem::size_of_val(samples));
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let relative = PathBuf::from(session_id)
            .join("chunks")
            .join(format!("{index:08}.f32"));
        write_spool_file(root, &relative, &bytes).unwrap();
    }

    #[test]
    fn failed_short_history_retry_insert_is_idempotent_for_acknowledged_audio() {
        let dir = temp_dir("failed-short-idempotent");
        let path = write_spool_file(&dir, Path::new("failed-job.wav"), b"complete wav")
            .expect("audio artifact is durably installed");
        let context = crate::context::ContextSnapshot::general();

        insert_failed_history_with_context_once(
            &dir,
            1.25,
            &path,
            &context,
            "on_device:qwen3-asr-0.6b",
        )
        .expect("History row is inserted and acknowledged");
        insert_failed_history_with_context_once(
            &dir,
            1.25,
            &path,
            &context,
            "on_device:qwen3-asr-0.6b",
        )
        .expect("a replay after a lost acknowledgement reuses the same row");

        let rows = get_history(&dir, 10).expect("History reads after insert");
        assert_eq!(rows.len(), 1);
        let connection = open_history(&dir).expect("History database is available");
        let stored_audio: String = connection
            .query_row(
                "SELECT raw_audio_path FROM dictations WHERE id=?",
                [rows[0].id],
                |row| row.get(0),
            )
            .expect("the History row references its acknowledged recovery WAV");
        assert_eq!(PathBuf::from(stored_audio), path);
        assert!(path.is_file());

        remove_spool_artifact(&dir, &path);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_failed_short_history_removes_flat_artifact_without_resurrection() {
        let dir = temp_dir("delete-failed-short-flat");
        let path = write_spool_file(&dir, Path::new("failed-job.wav"), b"short recording")
            .expect("persist the flat failed-short artifact");
        insert_failed_history_with_context_once(
            &dir,
            0.2,
            &path,
            &crate::context::ContextSnapshot::general(),
            "on_device:test",
        )
        .unwrap();

        let id = get_history(&dir, 10).unwrap()[0].id;
        delete_history(&dir, id).unwrap();

        assert!(get_history(&dir, 10).unwrap().is_empty());
        assert!(!path.exists());
        assert!(recover_spool(&dir, 7).unwrap().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deleting_history_with_retention_removed_artifact_does_not_resurrect() {
        let dir = temp_dir("delete-expired-flat-artifact");
        let path = write_spool_file(&dir, Path::new("expired-job.wav"), b"retained audio")
            .expect("persist a flat managed artifact");
        insert_failed_history_with_context_once(
            &dir,
            0.3,
            &path,
            &crate::context::ContextSnapshot::general(),
            "on_device:test",
        )
        .unwrap();
        fs::remove_file(&path).unwrap();

        let id = get_history(&dir, 10).unwrap()[0].id;
        delete_history(&dir, id).unwrap();

        assert!(get_history(&dir, 10).unwrap().is_empty());
        assert!(!path.exists());
        assert!(recover_spool(&dir, 7).unwrap().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn spool_io_commit_is_root_scoped_and_rejects_alias_and_prefix_roots() {
        let root = temp_dir("root-scoped-commit");
        let alias = root.join("unused-child").join("..");
        let child = root.join("child-root");
        let sibling = root.with_file_name(format!(
            "{}-sibling",
            root.file_name().unwrap().to_string_lossy()
        ));

        let owner = try_spool_io_commit(&root).expect("first data root is admitted");
        assert!(
            try_spool_io_commit(&alias).is_err(),
            "lexical aliases cannot bypass the root-scoped quota/clear permit"
        );
        assert!(
            try_spool_io_commit(&child).is_err(),
            "a nested path cannot mutate while its parent data root is committing"
        );
        let sibling_owner = try_spool_io_commit(&sibling)
            .expect("independent sibling data roots can make progress concurrently");
        drop(sibling_owner);
        drop(owner);

        let child_owner = try_spool_io_commit(&child)
            .expect("the nested root is admitted after the parent commit closes");
        drop(child_owner);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(sibling);
    }

    fn decode_wav_samples(path: &Path) -> Vec<i16> {
        let wav = read_spool_file(path).unwrap();
        hound::WavReader::new(std::io::Cursor::new(wav))
            .unwrap()
            .into_samples::<i16>()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn usage_upsert_by_day() {
        let dir = temp_dir("usage");
        insert_history(&dir, "a", "A", 2.0, false).unwrap();
        insert_history(&dir, "b", "B", 3.0, true).unwrap();
        let q = crate::queue::RequestGate::new(None).snapshots();
        let u = get_usage(&dir, q).unwrap();
        assert_eq!(u.asr_requests, 2);
        assert_eq!(u.llm_requests, 2);
        assert_eq!(u.audio_seconds, 5.0);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn schema_20_defaults_cleanup_intensity_auto() {
        let settings = Settings::default();
        assert_eq!(settings.cleanup_intensity, "auto");
        assert_eq!(settings.cascade_timeout_ms, 5000);
        assert_eq!(settings.cascade_proper_noun_threshold, 3);
        assert!(settings.accurate_asr_model.is_empty());
        assert_eq!(
            settings.accurate_asr_provider,
            crate::engine::EngineProvider::Groq
        );
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        let view = SettingsView::from(&settings);
        assert_eq!(view.cleanup_intensity, "auto");
        assert_eq!(view.cascade_timeout_ms, 5000);
        assert_eq!(view.cascade_proper_noun_threshold, 3);
        assert!(view.accurate_asr_model.is_empty());
        assert!(!settings.accurate_asr_configured());
        assert!(!settings.window_ocr_enabled);
        assert!(!view.window_ocr_enabled);
        assert!(settings.screen_action_hotkey.is_empty());
        assert!(settings.vision_provider.is_empty());
        assert!(settings.vision_model.is_empty());
        assert!(!settings.vision_configured());
        assert!(view.screen_action_hotkey.is_empty());
        assert!(view.vision_model.is_empty());
    }
    #[test]
    fn migration_keeps_an_explicit_saved_cleanup_intensity() {
        let mut settings = Settings {
            schema_version: 19,
            cleanup_intensity: "heavy".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.cleanup_intensity, "heavy");
    }

    #[test]
    fn accurate_asr_helpers_use_model_and_provider_keychain() {
        let mut settings = Settings::default();
        assert!(!settings.accurate_asr_configured());
        assert_eq!(settings.accurate_asr_credential(), "");
        settings.accurate_asr_model = "whisper-large-v3".into();
        settings.accurate_asr_provider = crate::engine::EngineProvider::OpenAi;
        settings
            .provider_api_keys
            .insert("openai".into(), "sk-accurate".into());
        assert!(settings.accurate_asr_configured());
        assert_eq!(settings.accurate_asr_credential(), "sk-accurate");
        assert!(settings
            .accurate_asr_endpoint()
            .expect("HTTP accurate endpoint")
            .contains("transcriptions"));
    }

    #[test]
    fn accurate_asr_configured_requires_nonempty_credential() {
        let mut settings = Settings {
            api_key: "gsk-groq".into(),
            accurate_asr_provider: crate::engine::EngineProvider::Custom,
            accurate_asr_model: "qwen3-asr-flash".into(),
            ..Settings::default()
        };
        assert_eq!(settings.accurate_asr_credential().trim(), "");
        assert!(!settings.accurate_asr_configured());

        settings
            .provider_api_keys
            .insert("custom".into(), "   ".into());
        assert_eq!(settings.accurate_asr_credential().trim(), "");
        assert!(!settings.accurate_asr_configured());

        settings
            .provider_api_keys
            .insert("custom".into(), "sk-bailian".into());
        assert_eq!(settings.accurate_asr_credential(), "sk-bailian");
        assert!(settings.accurate_asr_configured());

        settings.accurate_asr_provider = crate::engine::EngineProvider::Groq;
        settings.accurate_asr_model = "whisper-large-v3".into();
        settings.provider_api_keys.remove("custom");
        assert!(settings.accurate_asr_configured());
        assert_eq!(settings.accurate_asr_credential(), "gsk-groq");
    }

    #[test]
    fn vision_configured_needs_provider_model_and_key() {
        let mut settings = Settings {
            vision_model: "gpt-4o".into(),
            ..Settings::default()
        };
        assert!(!settings.vision_configured());
        settings.vision_provider = "openai".into();
        assert!(!settings.vision_configured());
        settings
            .provider_api_keys
            .insert("openai".into(), "sk-vision".into());
        assert!(settings.vision_configured());
        settings.vision_provider = "ollama".into();
        settings.vision_model = "llava".into();
        assert!(settings.vision_configured());
    }

    fn write_sensevoice_fixture(models_root: &Path) {
        let dir = models_root.join("sensevoice-small");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model.int8.onnx"), b"fixture-onnx").unwrap();
        std::fs::write(dir.join("tokens.txt"), b"fixture-tokens").unwrap();
        std::fs::write(
            crate::ondevice_asr::archive_sha_path(models_root, "sensevoice-small"),
            crate::ondevice_models::SENSEVOICE_ARCHIVE_SHA256,
        )
        .unwrap();
    }

    fn on_device_settings(onboarded: bool) -> Settings {
        Settings {
            asr_provider: crate::engine::EngineProvider::OnDevice,
            asr_model: "sensevoice-small".into(),
            api_key: String::new(),
            onboarded,
            cleanup_enabled: false,
            ..Settings::default()
        }
    }

    #[test]
    fn on_device_files_ready_do_not_allow_onboarding_without_inference() {
        let dir = temp_dir("on-device-ready-validate");
        let models_root = dir.join("models");
        write_sensevoice_fixture(&models_root);
        let settings = on_device_settings(true);
        assert!(settings
            .validate_with_models_root(Some(&models_root))
            .is_err());
        assert!(!settings.on_device_asr_ready(Some(&models_root)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn onboarded_on_device_empty_groq_key_rejected_when_model_missing() {
        let dir = temp_dir("on-device-missing-validate");
        let models_root = dir.join("models");
        std::fs::create_dir_all(&models_root).unwrap();
        let settings = on_device_settings(true);
        assert!(settings
            .validate_with_models_root(Some(&models_root))
            .is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn validate_without_models_root_rejects_on_device_empty_key() {
        let dir = temp_dir("on-device-validate-no-root");
        write_sensevoice_fixture(&dir.join("models"));
        let settings = on_device_settings(true);
        assert!(
            settings.validate().is_err(),
            "validate() must not grant the empty-key exception without a Ready check"
        );
        assert!(settings.validate_with_models_root(None).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn load_settings_does_not_keep_onboarding_for_files_only_on_device() {
        let dir = temp_dir("on-device-load-ready");
        std::fs::create_dir_all(&dir).unwrap();
        write_sensevoice_fixture(&dir.join("models"));
        std::fs::write(
            dir.join("settings.json"),
            r#"{"schema_version":19,"onboarded":true,"asr_provider":"on_device","asr_model":"sensevoice-small","cleanup_enabled":false}"#,
        )
        .unwrap();
        let (settings, _) = load_settings(&dir);
        assert_eq!(
            settings.asr_provider,
            crate::engine::EngineProvider::OnDevice
        );
        assert!(!settings.onboarded);
        assert!(settings.api_key.trim().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn vision_and_cleanup_reject_on_device() {
        let mut settings = on_device_settings(false);
        settings.cleanup_enabled = true;
        settings.cleanup_provider = crate::engine::EngineProvider::OnDevice;
        settings.cleanup_model = "llama3.2".into();
        assert!(settings.validate().is_err());

        settings.cleanup_enabled = false;
        settings.vision_provider = "on_device".into();
        settings.vision_model = "sensevoice-small".into();
        assert!(!settings.vision_configured());
        assert!(settings.vision_endpoint().is_none());
    }

    #[test]
    fn accurate_asr_rejects_on_device_and_empty_key() {
        let settings = Settings {
            accurate_asr_provider: crate::engine::EngineProvider::OnDevice,
            accurate_asr_model: "sensevoice-small".into(),
            ..Settings::default()
        };
        assert!(!settings.accurate_asr_configured());
        assert!(settings.accurate_asr_endpoint().is_none());
    }

    #[test]
    fn schema_20_copies_legacy_style_example_into_pairs() {
        let mut settings = Settings {
            cleanup_intensity: String::new(),
            cascade_timeout_ms: 0,
            cascade_proper_noun_threshold: 0,
            context_mappings: vec![crate::context::AppMapping {
                id: "wechat".into(),
                label: "微信".into(),
                family: crate::context::ContextFamily::PersonalChat,
                mode_id: None,
                bundle_id: Some("com.tencent.xinWeChat".into()),
                executable: None,
                browser_host: None,
                browser_path_prefix: None,
                focused_field: None,
                source_permissions: Default::default(),
                style_example_input: Some("好的哈哈".into()),
                style_example_output: Some("好的哈哈。".into()),
                style_example_pairs: Vec::new(),
                style_examples_approved: false,
                enabled: true,
                cleanup_effort: None,
                cleanup_intensity: None,
                cleanup_enabled: true,
                dictionary_learn_enabled: true,
            }],
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.cleanup_intensity, "auto");
        assert_eq!(settings.cascade_timeout_ms, 5000);
        assert_eq!(settings.cascade_proper_noun_threshold, 3);
        assert_eq!(settings.context_mappings[0].style_example_pairs.len(), 1);
        assert_eq!(
            settings.context_mappings[0].style_example_pairs[0].input,
            "好的哈哈"
        );
        assert_eq!(
            settings.context_mappings[0].style_example_pairs[0].output,
            "好的哈哈。"
        );
    }

    #[test]
    fn schema_20_preserves_only_explicit_legacy_local_ocr_choice() {
        let mapping = crate::context::AppMapping {
            id: "notes".into(),
            label: "Notes".into(),
            family: crate::context::ContextFamily::Document,
            mode_id: None,
            bundle_id: Some("com.example.Notes".into()),
            executable: None,
            browser_host: None,
            browser_path_prefix: None,
            focused_field: None,
            source_permissions: Default::default(),
            style_example_input: None,
            style_example_output: None,
            style_example_pairs: Vec::new(),
            style_examples_approved: false,
            enabled: true,
            cleanup_effort: None,
            cleanup_intensity: None,
            cleanup_enabled: true,
            dictionary_learn_enabled: true,
        };
        let mut settings = Settings {
            schema_version: 20,
            window_ocr_enabled: true,
            context_mappings: vec![mapping],
            ..Settings::default()
        };
        settings.normalize();
        let grants = settings.context_mappings[0].source_permissions;
        assert!(grants.local_ocr);
        assert!(!grants.ax_text);
        assert!(!grants.cloud_vision);
        assert!(!grants.context_text_to_providers);

        let legacy: crate::context::AppMapping = serde_json::from_str(
            r#"{"id":"legacy","label":"Legacy","family":"document","bundle_id":"com.example.Editor","style_example_input":"private","style_example_output":"private."}"#,
        )
        .unwrap();
        assert_eq!(legacy.style_example_input.as_deref(), Some("private"));
        assert!(!legacy.style_examples_approved);
        assert_eq!(legacy.source_permissions, Default::default());
    }

    #[test]
    fn old_settings_get_new_defaults() {
        let dir = temp_dir("settings");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"schema_version":8,"api_key":"test","selected_action_hotkey":"","selected_actions_enabled":false,"output_mode":"plain"}"#,
        )
        .unwrap();
        let (settings, _) = load_settings(&dir);
        assert!(!settings.onboarded);
        assert!(settings.cleanup_enabled);
        assert_eq!(settings.cleanup_intensity, "auto");
        assert_eq!(settings.cascade_timeout_ms, 5000);
        assert_eq!(settings.cascade_proper_noun_threshold, 3);
        assert!(settings.accurate_asr_model.is_empty());
        assert!(settings.show_tray_icon);
        assert_eq!(settings.cleanup_model, crate::llm::MODEL);
        assert_eq!(settings.activation_mode, "tap");
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.ui_language, "system");
        assert_eq!(settings.theme, "system");
        assert_eq!(settings.output_mode, "auto");
        assert_eq!(settings.translation_target_language, "en");
        assert_eq!(settings.long_output_mode, "paste");
        assert_eq!(settings.hotkey, "CmdOrControl+Alt+Space");
        assert_eq!(settings.selected_action_hotkey, "CmdOrControl+Alt+Slash");
        assert!(settings.selected_actions_enabled);
        assert!(settings.dictionary_learn_enabled);
        assert!(settings.input_device.is_empty());
        assert_eq!(settings.input_gain, 1.0);
        assert!(!settings.keep_success_audio);
        assert!(settings.asr_base_url.is_empty());
        assert!(settings.asr_api_key.is_empty());
        assert!(settings
            .writing_modes
            .iter()
            .any(|mode| mode.id == "general" && mode.builtin));
        let backup: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.join("settings.json.pre-migration.bak")).unwrap(),
        )
        .unwrap();
        assert_eq!(backup["api_key"], "");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn schema_14_replaces_legacy_shift_slash_defaults_and_option_divide() {
        let mut settings = Settings {
            schema_version: 13,
            hotkey: "CmdOrControl+Shift+Space".into(),
            selected_action_hotkey: "CmdOrControl+Alt+÷".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.hotkey, "CmdOrControl+Alt+Space");
        assert_eq!(settings.selected_action_hotkey, "CmdOrControl+Alt+Slash");

        let mut legacy_slash = Settings {
            schema_version: 13,
            selected_action_hotkey: "CmdOrControl+Shift+Slash".into(),
            ..Settings::default()
        };
        legacy_slash.normalize();
        assert_eq!(
            legacy_slash.selected_action_hotkey,
            "CmdOrControl+Alt+Slash"
        );

        let mut previous_default = Settings {
            schema_version: 14,
            selected_action_hotkey: "CmdOrControl+Alt+Backslash".into(),
            ..Settings::default()
        };
        previous_default.normalize();
        assert_eq!(
            previous_default.selected_action_hotkey,
            "CmdOrControl+Alt+Slash"
        );

        let mut custom = Settings {
            schema_version: 13,
            hotkey: "CmdOrControl+Shift+K".into(),
            selected_action_hotkey: "CmdOrControl+Alt+K".into(),
            ..Settings::default()
        };
        custom.normalize();
        assert_eq!(custom.hotkey, "CmdOrControl+Shift+K");
        assert_eq!(custom.selected_action_hotkey, "CmdOrControl+Alt+K");
    }

    #[test]
    fn settings_schema_migrates_retired_gestures_to_tap_idempotently_without_rebinding() {
        for mode in ["double_tap", "hybrid", "hold", "tap"] {
            for hotkey in ["Fn", "CmdOrControl+Alt+K", "Shift"] {
                let mut settings = Settings {
                    schema_version: 24,
                    activation_mode: mode.into(),
                    hotkey: hotkey.into(),
                    ..Settings::default()
                };
                settings.normalize();
                assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
                assert_eq!(settings.activation_mode, "tap");
                assert_eq!(settings.hotkey, hotkey);
                settings.normalize();
                assert_eq!(settings.activation_mode, "tap");
                settings.validate().unwrap();
            }
        }
    }
    #[test]
    fn both_current_modes_support_fn_and_combinations() {
        assert_eq!(Settings::default().activation_mode, "tap");
        for mode in ["tap", "hold_to_talk"] {
            for hotkey in ["Fn", "CmdOrControl+Alt+Space"] {
                let mut settings = Settings {
                    activation_mode: mode.into(),
                    hotkey: hotkey.into(),
                    ..Settings::default()
                };
                settings.normalize();
                assert_eq!(settings.activation_mode, mode);
                settings.validate().unwrap();
            }
        }
    }
    #[test]
    fn changing_binding_preserves_mode_and_rejects_new_standalone_modifiers() {
        let previous = Settings {
            activation_mode: "hold_to_talk".into(),
            ..Settings::default()
        };
        let mut next = previous.clone();
        next.hotkey = "Fn".into();
        next.validate_binding_changes(&previous).unwrap();
        assert_eq!(next.activation_mode, "hold_to_talk");
        next.hotkey = "Shift".into();
        assert!(next.validate_binding_changes(&previous).is_err());
        let legacy = next.clone();
        next.validate_binding_changes(&legacy).unwrap();
    }
    #[test]
    fn new_global_bindings_do_not_take_over_typing_or_navigation() {
        let previous = Settings::default();
        for binding in ["A", "Enter", "Tab", "Space", "Shift+A", "Shift+Space"] {
            let mut next = previous.clone();
            next.hotkey = binding.into();
            next.validate_bindings().unwrap();
            assert!(
                next.validate_binding_changes(&previous).is_err(),
                "{binding}"
            );
            next.hotkey = previous.hotkey.clone();
            next.selected_action_hotkey = binding.into();
            assert!(
                next.validate_binding_changes(&previous).is_err(),
                "{binding}"
            );
        }
    }
    #[test]
    fn new_combinations_and_function_keys_remain_available() {
        let previous = Settings::default();
        for binding in [
            "Fn",
            "CmdOrControl+Shift+1",
            "Alt+A",
            "Control+K",
            "F13",
            "Shift+F13",
        ] {
            let mut next = previous.clone();
            next.hotkey = binding.into();
            next.validate_binding_changes(&previous).unwrap();
        }
    }
    #[test]
    fn mode_changes_preserve_existing_plain_key_bindings() {
        let previous = Settings {
            hotkey: "Shift+A".into(),
            ..Settings::default()
        };
        let mut next = previous.clone();
        next.activation_mode = "hold_to_talk".into();
        next.validate_binding_changes(&previous).unwrap();
        assert_eq!(next.hotkey, "Shift+A");
    }
    #[test]
    fn configuration_validation_allows_missing_unchanged_keys_but_readiness_does_not() {
        let settings = Settings {
            onboarded: true,
            activation_mode: "hold_to_talk".into(),
            ..Settings::default()
        };
        assert!(settings.validate_configuration().is_ok());
        assert!(settings.validate().is_err());
        let dir = temp_dir("binding-without-key");
        let backend = FakeCredentialBackend::default();
        save_settings_with_validation(&dir, &settings, &backend, false).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(saved["activation_mode"], "hold_to_talk");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn non_service_configuration_save_preserves_deferred_credentials_without_store_access() {
        let dir = temp_dir("non-service-save-deferred-credentials");
        let sidecar_value = "deferred-sidecar-fixture";
        write_secret_sidecar(&dir, "asr_api_key", sidecar_value).unwrap();
        let sidecar_path = secret_sidecar_path(&dir, "asr_api_key");
        let sidecar_before = std::fs::read(&sidecar_path).unwrap();
        let settings_credential = "deferred-settings-fixture";
        let settings = Settings {
            onboarded: true,
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_model: "whisper-1".into(),
            asr_api_key: sidecar_value.into(),
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_model: "router-cleanup-model".into(),
            cleanup_api_key: settings_credential.into(),
            custom_base_url: "https://router.example.test/v1".into(),
            input_device: "MacBook Pro Microphone".into(),
            ..Settings::default()
        };
        let mut settings = settings;
        settings
            .unverified_credential_sources
            .insert("sidecar:asr_api_key".into(), sidecar_value.into());
        settings.unverified_credential_sources.insert(
            "settings:cleanup_api_key".into(),
            settings_credential.into(),
        );
        settings
            .credential_baselines
            .insert("provider_custom".into(), "existing-baseline-fixture".into());

        save_configuration(&dir, &settings)
            .expect("a structurally valid device preference does not require provider readiness");

        let on_disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(on_disk["input_device"], "MacBook Pro Microphone");
        assert_eq!(on_disk["custom_base_url"], "https://router.example.test/v1");
        assert_eq!(on_disk["asr_api_key"], "");
        assert_eq!(on_disk["cleanup_api_key"], settings_credential);
        assert_eq!(on_disk["provider_api_keys"], serde_json::json!({}));
        assert_eq!(std::fs::read(&sidecar_path).unwrap(), sidecar_before);
        assert_eq!(
            settings.credential_baselines.get("provider_custom"),
            Some(&"existing-baseline-fixture".to_owned())
        );
        assert!(settings.validate_configuration().is_ok());
        let missing_credentials = Settings {
            onboarded: true,
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_model: "whisper-1".into(),
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_model: "router-cleanup-model".into(),
            custom_base_url: "https://router.example.test/v1".into(),
            ..Settings::default()
        };
        assert!(missing_credentials.validate_configuration().is_ok());
        assert!(missing_credentials.validate_with_models_root(None).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_local_model_does_not_block_structural_preferences_but_readiness_still_fails() {
        let dir = temp_dir("non-service-save-missing-local-model");
        let settings = Settings {
            onboarded: true,
            asr_provider: crate::engine::EngineProvider::OnDevice,
            cleanup_enabled: false,
            input_device: "MacBook Pro Microphone".into(),
            ..Settings::default()
        };

        save_configuration(&dir, &settings)
            .expect("local model availability is not a prerequisite for saving preferences");
        assert!(settings
            .validate_with_models_root(Some(&dir.join("models")))
            .is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn remote_keyless_local_route_stays_readiness_gated_during_preference_saves() {
        let dir = temp_dir("non-service-save-remote-keyless-local-route");
        let settings = Settings {
            onboarded: true,
            asr_provider: crate::engine::EngineProvider::LocalWhisper,
            local_whisper_base_url: "https://remote.example.test/v1".into(),
            cleanup_enabled: false,
            input_device: "MacBook Pro Microphone".into(),
            ..Settings::default()
        };

        save_configuration(&dir, &settings)
            .expect("structurally valid preferences may be saved without route readiness");
        assert!(settings
            .validate_with_models_root(Some(&dir.join("models")))
            .is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn structurally_invalid_audio_preferences_are_still_rejected() {
        let mut settings = Settings {
            input_device: "x".repeat(513),
            ..Settings::default()
        };
        assert!(settings.validate_configuration().is_err());

        settings.input_device = "MacBook Pro Microphone".into();
        settings.input_gain = 4.1;
        assert!(settings.validate_configuration().is_err());
    }

    #[test]
    fn selected_action_migration_preserves_custom_shortcuts() {
        let mut settings = Settings {
            schema_version: 8,
            selected_action_hotkey: "CmdOrControl+Alt+K".into(),
            selected_actions_enabled: false,
            ..Settings::default()
        };

        settings.normalize();

        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.selected_action_hotkey, "CmdOrControl+Alt+K");
        assert!(!settings.selected_actions_enabled);
    }

    #[test]
    fn input_device_preference_is_trimmed_and_keeps_default_empty() {
        let mut settings = Settings {
            input_device: "  EarPods Microphone  ".into(),
            ..Settings::default()
        };

        settings.normalize();

        assert_eq!(settings.input_device, "EarPods Microphone");
        assert!(Settings::default().input_device.is_empty());
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn long_recording_defaults_to_auto_paste() {
        assert_eq!(Settings::default().long_output_mode, "paste");
        assert_eq!(Settings::default().delivery_policy, "auto");

        let mut settings = Settings {
            long_output_mode: "unsupported".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.long_output_mode, "paste");
        assert_eq!(settings.delivery_policy, "auto");
    }

    #[test]
    fn legacy_long_recording_default_migrates_to_auto_paste() {
        let mut settings = Settings {
            schema_version: 11,
            long_output_mode: "clipboard".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.long_output_mode, "paste");
        assert_eq!(settings.delivery_policy, "auto");

        let mut current_choice = Settings {
            schema_version: SETTINGS_SCHEMA_VERSION,
            long_output_mode: "clipboard".into(),
            ..Settings::default()
        };
        current_choice.normalize();
        assert_eq!(current_choice.long_output_mode, "clipboard");

        let mut explicit_delivery = Settings {
            delivery_policy: "clipboard_only".into(),
            ..Settings::default()
        };
        explicit_delivery.normalize();
        assert_eq!(explicit_delivery.delivery_policy, "clipboard_only");
    }

    #[test]
    fn invalid_ui_language_and_theme_fall_back_to_safe_defaults() {
        let mut settings = Settings {
            ui_language: "fr".into(),
            theme: "sepia".into(),
            ..Settings::default()
        };

        settings.normalize();

        assert_eq!(settings.ui_language, "system");
        assert_eq!(settings.theme, "system");
    }

    #[test]
    fn unfinished_setup_keeps_explicit_chinese_after_later_schema_bumps() {
        let mut current = Settings {
            schema_version: 25,
            onboarded: false,
            ui_language: "zh".into(),
            ..Settings::default()
        };
        current.normalize();
        assert_eq!(current.ui_language, "zh");

        let mut legacy = Settings {
            schema_version: 6,
            onboarded: false,
            ui_language: "zh".into(),
            ..Settings::default()
        };
        legacy.normalize();
        assert_eq!(legacy.ui_language, "system");
    }

    #[test]
    fn deprecated_single_modifier_bindings_remain_visible_after_migration() {
        let mut settings = Settings {
            selected_action_hotkey: "Shift".into(),
            verbatim_hotkey: "Alt".into(),
            translation_hotkey: "Control".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.selected_action_hotkey, "Shift");
        assert_eq!(settings.verbatim_hotkey, "Alt");
        assert_eq!(settings.translation_hotkey, "Control");
        settings.validate().unwrap();
    }

    #[test]
    fn selected_actions_can_be_enabled_before_a_hotkey_is_assigned() {
        let settings = Settings {
            selected_actions_enabled: true,
            ..Settings::default()
        };

        assert!(settings.validate().is_ok());
    }

    #[test]
    fn unsupported_cleanup_model_falls_back_to_default() {
        let mut settings = Settings {
            cleanup_model: "retired/model".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.cleanup_model, crate::llm::MODEL);
        assert!(Settings {
            cleanup_model: "retired/model".into(),
            ..Settings::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn cleanup_default_moves_off_retired_llama_without_rewriting_saved_choices() {
        assert_eq!(Settings::default().cleanup_model, "openai/gpt-oss-20b");
        let dir = temp_dir("cleanup-model-retirement");
        std::fs::create_dir_all(&dir).unwrap();
        for model in [
            "llama-3.1-8b-instant",
            "llama-3.3-70b-versatile",
            "openai/gpt-oss-120b",
        ] {
            std::fs::write(
                dir.join("settings.json"),
                serde_json::to_vec(&serde_json::json!({
                    "schema_version": SETTINGS_SCHEMA_VERSION,
                    "onboarded": false,
                    "cleanup_enabled": false,
                    "cleanup_provider": "groq",
                    "cleanup_model": model,
                }))
                .unwrap(),
            )
            .unwrap();
            let (settings, _) = load_settings(&dir);
            assert_eq!(settings.cleanup_model, model);
        }

        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": SETTINGS_SCHEMA_VERSION,
                "onboarded": false,
                "cleanup_enabled": false,
                "cleanup_provider": "groq",
            }))
            .unwrap(),
        )
        .unwrap();
        let (settings, _) = load_settings(&dir);
        assert_eq!(settings.cleanup_model, "openai/gpt-oss-20b");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn normalize_keeps_custom_asr_url_when_the_key_is_empty() {
        let mut settings = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            asr_api_key: String::new(),
            asr_model: "my-local-whisper".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.asr_provider, crate::engine::EngineProvider::Custom);
        assert_eq!(settings.asr_base_url, "http://127.0.0.1:8000/v1");
        assert_eq!(settings.asr_model, "my-local-whisper");
    }

    #[test]
    fn load_settings_repairs_incomplete_custom_asr_left_on_disk() {
        let dir = temp_dir("incomplete-custom-asr");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"schema_version":16,"asr_provider":"custom","asr_base_url":"https://relay.example.com/v1","asr_model":"whisper-1"}"#,
        )
        .unwrap();
        let (settings, needs_persist) = load_settings(&dir);
        assert_eq!(settings.asr_provider, crate::engine::EngineProvider::Groq);
        assert!(settings.asr_base_url.is_empty());
        assert_eq!(settings.asr_model, crate::asr::MODEL);
        assert!(settings.validate().is_ok());
        assert!(needs_persist);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn schema_16_keeps_groq_urls_on_the_groq_provider() {
        let mut settings = Settings {
            schema_version: 15,
            asr_base_url: "https://api.groq.com/openai/v1".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.asr_provider, crate::engine::EngineProvider::Groq);
        assert!(settings.asr_base_url.is_empty());
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn schema_16_infers_custom_asr_from_a_saved_url() {
        let mut settings = Settings {
            schema_version: 15,
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            asr_api_key: "local-key".into(),
            asr_model: "my-local-whisper".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.asr_provider, crate::engine::EngineProvider::Custom);
        assert_eq!(settings.asr_model, "my-local-whisper");
        assert_eq!(
            settings.cleanup_provider,
            crate::engine::EngineProvider::Groq
        );
        assert!(settings.cleanup_base_url.is_empty());
    }

    #[test]
    fn groq_provider_clears_custom_url_and_resets_unknown_asr_model() {
        let mut settings = Settings {
            asr_provider: crate::engine::EngineProvider::Groq,
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            asr_model: "whisper-1".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert!(settings.asr_base_url.is_empty());
        assert_eq!(settings.asr_model, crate::asr::MODEL);
    }

    #[test]
    fn custom_cleanup_keeps_unknown_models_and_does_not_reuse_the_groq_key() {
        let settings = Settings {
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_base_url: "https://api.openai.com/v1".into(),
            cleanup_model: "gpt-4o-mini".into(),
            api_key: "gsk_fallback".into(),
            cleanup_api_key: String::new(),
            onboarded: true,
            ..Settings::default()
        };
        assert_eq!(settings.cleanup_credential(), "");
        assert_eq!(
            settings.validate().unwrap_err().to_string(),
            "自定义整理地址需要填写整理密钥。"
        );

        let mut kept = settings.clone();
        kept.cleanup_api_key = "sk-openai".into();
        kept.normalize();
        assert_eq!(kept.cleanup_model, "gpt-4o-mini");
        assert_eq!(kept.cleanup_request_model(), "gpt-4o-mini");
        assert_eq!(
            kept.cleanup_endpoint(),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(kept.cleanup_credential(), "sk-openai");
        assert!(kept.validate().is_ok());
    }

    #[test]
    fn load_and_public_view_keep_keychain_unavailable_distinct_from_missing() {
        let dir = temp_dir("credential-read-state");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": SETTINGS_SCHEMA_VERSION,
                "asr_provider": "custom",
                "custom_base_url": "https://relay.example.test/v1",
                "cleanup_enabled": true,
                "cleanup_provider": "custom",
            }))
            .unwrap(),
        )
        .unwrap();
        let backend = FakeCredentialBackend::default();
        backend.script_reads(
            "provider_custom",
            [crate::keychain::ApiKeyState::Unavailable(
                "synthetic keychain failure".into(),
            )],
        );
        backend.script_reads("asr_api_key", [crate::keychain::ApiKeyState::Missing]);
        backend.script_reads("cleanup_api_key", [crate::keychain::ApiKeyState::Missing]);
        backend.script_reads("groq_api_key", [crate::keychain::ApiKeyState::Missing]);

        let (settings, _) = load_settings_with_backend(&dir, &backend);
        let view = SettingsView::from(&settings);

        assert_eq!(
            settings.credential_read_states.get("provider_custom"),
            Some(&CredentialReadState::Unavailable)
        );
        assert_eq!(
            settings.credential_read_states.get("asr_api_key"),
            Some(&CredentialReadState::Missing)
        );
        assert_eq!(
            view.provider_keys["custom"].keychain_state,
            CredentialReadState::Unavailable
        );
        assert_eq!(
            view.asr_api_key_keychain_state,
            CredentialReadState::Missing
        );
        assert_eq!(
            view.cleanup_api_key_keychain_state,
            CredentialReadState::Missing
        );
        assert_eq!(view.custom_base_url, "https://relay.example.test/v1");
        assert!(settings.asr_api_key.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unavailable_keychain_keeps_an_unverified_session_value_without_claiming_it_is_durable() {
        let dir = temp_dir("unavailable-custom-session-value");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": SETTINGS_SCHEMA_VERSION,
                "asr_provider": "custom",
                "asr_base_url": "https://relay.example.test/v1",
                "custom_base_url": "https://relay.example.test/v1",
                "asr_api_key": "synthetic-deferred-key",
                "cleanup_enabled": false,
            }))
            .unwrap(),
        )
        .unwrap();
        let backend = FakeCredentialBackend::default();
        backend.script_reads("provider_custom", [crate::keychain::ApiKeyState::Missing]);
        backend.script_reads(
            "asr_api_key",
            [crate::keychain::ApiKeyState::Unavailable(
                "synthetic keychain failure".into(),
            )],
        );
        backend.script_reads("cleanup_api_key", [crate::keychain::ApiKeyState::Missing]);
        backend.script_reads("groq_api_key", [crate::keychain::ApiKeyState::Missing]);

        let (settings, _) = load_settings_with_backend(&dir, &backend);
        let view = SettingsView::from(&settings);

        assert_eq!(settings.asr_api_key, "synthetic-deferred-key");
        assert!(settings
            .unverified_credential_sources
            .contains_key("settings:asr_api_key"));
        assert!(view.provider_keys["custom"].configured);
        assert_eq!(
            view.provider_keys["custom"].keychain_state,
            CredentialReadState::Unavailable
        );
        assert_eq!(
            view.asr_api_key_keychain_state,
            CredentialReadState::Unavailable
        );
        assert!(backend.writes.borrow().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unknown_asr_origin_keeps_legacy_slot_unavailable_outside_custom_recovery_scope() {
        let dir = temp_dir("unknown-origin-custom-session-value");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": SETTINGS_SCHEMA_VERSION,
                "asr_provider": "custom",
                "custom_base_url": "https://relay.example.test/v1",
                "asr_api_key": "synthetic-deferred-key",
                "cleanup_enabled": false,
            }))
            .unwrap(),
        )
        .unwrap();
        let backend = FakeCredentialBackend::default();
        backend.script_reads("provider_custom", [crate::keychain::ApiKeyState::Missing]);
        backend.script_reads(
            "asr_api_key",
            [crate::keychain::ApiKeyState::Unavailable(
                "synthetic keychain failure".into(),
            )],
        );
        backend.script_reads("cleanup_api_key", [crate::keychain::ApiKeyState::Missing]);
        backend.script_reads("groq_api_key", [crate::keychain::ApiKeyState::Missing]);

        let (settings, _) = load_settings_with_backend(&dir, &backend);
        let view = SettingsView::from(&settings);

        let asr_scope =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).unwrap();
        assert!(!asr_scope.include_asr_legacy);
        assert_eq!(settings.asr_api_key, "synthetic-deferred-key");
        assert!(settings
            .unverified_credential_sources
            .contains_key("settings:asr_api_key"));
        assert!(view.provider_keys["custom"].configured);
        assert_eq!(
            view.provider_keys["custom"].keychain_state,
            CredentialReadState::Missing
        );
        assert_eq!(
            view.asr_api_key_keychain_state,
            CredentialReadState::Unavailable
        );
        assert!(backend.writes.borrow().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn custom_recovery_scope_excludes_non_custom_purpose_aliases() {
        let mut settings = Settings {
            asr_provider: crate::engine::EngineProvider::Groq,
            cleanup_enabled: true,
            cleanup_provider: crate::engine::EngineProvider::Custom,
            custom_base_url: "https://router.example.test/v1/chat/completions".into(),
            cleanup_base_url: "https://router.example.test/v1/cleanup".into(),
            ..Settings::default()
        };
        settings.asr_base_url = "https://router.example.test/v1/asr".into();
        let cleanup_scope =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Cleanup).unwrap();
        assert_eq!(cleanup_scope.origin, "https://router.example.test");
        assert_eq!(cleanup_scope.purpose, CustomCredentialPurpose::Cleanup);
        assert!(!cleanup_scope.include_asr_legacy);
        assert!(cleanup_scope.include_cleanup_legacy);
        assert!(
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).is_none()
        );

        settings.asr_provider = crate::engine::EngineProvider::Custom;
        let both_custom =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).unwrap();
        assert!(both_custom.include_asr_legacy);
        assert!(both_custom.include_cleanup_legacy);

        settings.cleanup_enabled = false;
        let asr_only =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).unwrap();
        assert!(asr_only.include_asr_legacy);
        assert!(!asr_only.include_cleanup_legacy);

        settings.custom_base_url = "file:///tmp/invalid".into();
        assert!(
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).is_none()
        );
    }

    #[test]
    fn foreground_recovery_omits_a_legacy_alias_from_a_different_origin() {
        let settings = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router-a.example.test/v1".into(),
            cleanup_enabled: true,
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_base_url: "https://router-b.example.test/v1".into(),
            custom_base_url: "https://router-a.example.test/v1".into(),
            ..Settings::default()
        };
        let scope =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).unwrap();
        assert_eq!(scope.origin, "https://router-a.example.test");
        assert!(scope.include_asr_legacy);
        assert!(!scope.include_cleanup_legacy);
        let cleanup_scope =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Cleanup).unwrap();
        assert!(!cleanup_scope.include_cleanup_legacy);
    }

    #[test]
    fn unavailable_custom_readiness_does_not_claim_the_key_is_missing() {
        let mut settings = Settings {
            onboarded: true,
            asr_provider: crate::engine::EngineProvider::Custom,
            custom_base_url: "https://router.example.test/v1".into(),
            ..Settings::default()
        };
        settings
            .credential_read_states
            .insert("provider_custom".into(), CredentialReadState::Unavailable);
        assert_eq!(
            settings.validate().unwrap_err().to_string(),
            "钥匙串暂时不可读取；当前设置未更改，请在前台设置中重试读取。"
        );
    }

    #[test]
    fn custom_recovery_adoption_is_session_only_and_does_not_fan_out_on_save() {
        let dir = temp_dir("session-only-custom-recovery");
        let backend = FakeCredentialBackend::default();
        let mut settings = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router.example.test/v1".into(),
            ..Settings::default()
        };
        let scope =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).unwrap();
        adopt_interactive_custom_credential(
            &mut settings,
            "synthetic-session-only-key".into(),
            scope,
        );

        assert!(settings.session_only_custom_credential);
        assert_eq!(
            settings.provider_api_keys.get("custom").map(String::as_str),
            Some("synthetic-session-only-key")
        );
        assert_eq!(
            settings.provider_secret(crate::engine::EngineProvider::Custom),
            ""
        );
        settings.vision_provider = "custom".into();
        assert_eq!(settings.vision_credential(), "");
        let public = SettingsView::from(&settings);
        assert!(public.provider_keys["custom"].configured);
        assert_eq!(
            public.provider_keys["custom"].hint.as_deref(),
            Some("••••y-key")
        );
        assert!(backend.writes.borrow().is_empty());
        save_settings_with_backend(&dir, &settings, &backend).unwrap();
        assert!(backend.writes.borrow().is_empty());
        let on_disk = fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(!on_disk.contains("synthetic-session-only-key"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn parsed_preference_patch_can_preserve_nonsecret_credential_state() {
        let mut previous = Settings::default();
        previous
            .credential_read_states
            .insert("provider_custom".into(), CredentialReadState::Unavailable);
        previous.session_only_custom_credential = true;
        previous.session_custom_credential_scope = Some(CustomCredentialRecoveryScope {
            origin: "https://router.example.test".into(),
            purpose: CustomCredentialPurpose::Asr,
            asr_active: true,
            cleanup_active: false,
            include_asr_legacy: true,
            include_cleanup_legacy: false,
        });
        previous.unverified_credential_sources.insert(
            "settings:cleanup_api_key".into(),
            "synthetic-deferred".into(),
        );
        let parsed: Settings =
            serde_json::from_value(serde_json::to_value(&previous).unwrap()).unwrap();
        let mut next = parsed;

        next.preserve_credential_runtime_metadata_from(&previous);

        assert_eq!(
            next.credential_read_states.get("provider_custom"),
            Some(&CredentialReadState::Unavailable)
        );
        assert!(next.session_only_custom_credential);
        assert_eq!(
            next.session_custom_credential_scope,
            previous.session_custom_credential_scope
        );
        assert_eq!(
            next.unverified_credential_sources
                .get("settings:cleanup_api_key"),
            Some(&"synthetic-deferred".to_owned())
        );
    }

    #[test]
    fn public_settings_view_never_contains_the_full_runtime_credential() {
        let secret = "synthetic-full-secret-12345";
        let settings = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            custom_base_url: "https://router.example.test/v1".into(),
            provider_api_keys: [("custom".to_owned(), secret.to_owned())]
                .into_iter()
                .collect(),
            ..Settings::default()
        };
        let serialized = serde_json::to_string(&SettingsView::from(&settings)).unwrap();
        assert!(!serialized.contains(secret));
        assert!(serialized.contains("configured"));
        assert!(serialized.contains("keychain_state"));
    }

    #[test]
    fn settings_view_masks_every_short_ascii_and_unicode_credential() {
        for secret in ["x", "ab", "abc", "abcd", "abcde", "🔑秘密"] {
            let settings = Settings {
                api_key: secret.into(),
                provider_api_keys: [("custom".to_owned(), secret.into())].into_iter().collect(),
                ..Settings::default()
            };
            let view = SettingsView::from(&settings);
            let serialized = serde_json::to_string(&view).unwrap();
            assert_eq!(view.api_key_hint.as_deref(), Some("••••"));
            assert_eq!(view.provider_keys["custom"].hint.as_deref(), Some("••••"));
            let public: serde_json::Value = serde_json::from_str(&serialized).unwrap();
            assert_eq!(public["api_key_hint"], "••••");
            assert_eq!(public["provider_keys"]["custom"]["hint"], "••••");
        }
    }

    #[test]
    fn session_custom_credential_is_bound_to_origin_across_a_settings_patch_round_trip() {
        let dir = temp_dir("session-custom-origin-patch");
        let backend = FakeCredentialBackend::default();
        let mut current = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router-a.example.test/v1".into(),
            cleanup_enabled: false,
            ..Settings::default()
        };
        let scope =
            custom_credential_recovery_scope(&current, CustomCredentialPurpose::Asr).unwrap();
        adopt_interactive_custom_credential(
            &mut current,
            "synthetic-session-origin-key".into(),
            scope,
        );
        assert_eq!(current.asr_credential(), "synthetic-session-origin-key");

        // Match update_settings_patch's serde merge plus runtime-metadata
        // restoration; the recovered value is not sent by the frontend.
        let mut merged = serde_json::to_value(&current).unwrap();
        merged.as_object_mut().unwrap().insert(
            "asr_base_url".into(),
            serde_json::Value::String("https://router-b.example.test/v1".into()),
        );
        let mut next: Settings = serde_json::from_value(merged).unwrap();
        next.preserve_credential_runtime_metadata_from(&current);
        next.session_only_custom_credential = current.session_only_custom_credential;
        assert!(next.clear_stale_session_custom_credential());
        assert!(!next.session_only_custom_credential);
        assert!(next.session_custom_credential_scope.is_none());
        assert!(next.session_custom_credential_rejected);
        assert_eq!(
            next.credential_read_states.get("provider_custom"),
            Some(&CredentialReadState::Configured),
            "revoking a process-only route lease must not rewrite the last Keychain read result"
        );
        assert!(!next.provider_api_keys.contains_key("custom"));
        assert_eq!(next.asr_credential(), "");

        save_settings_with_backend(&dir, &next, &backend).unwrap();
        assert!(backend.writes.borrow().is_empty());
        let on_disk = fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(!on_disk.contains("synthetic-session-origin-key"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn revoked_custom_value_stays_rejected_through_full_and_patch_round_trips() {
        let dir = temp_dir("revoked-custom-full-patch-round-trip");
        let backend = FakeCredentialBackend::default();
        let mut previous = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router-a.example.test/v1".into(),
            custom_base_url: "https://router-a.example.test/v1".into(),
            asr_api_key: "synthetic-deferred-asr-alias".into(),
            cleanup_enabled: false,
            ..Settings::default()
        };
        previous.unverified_credential_sources.insert(
            "settings:asr_api_key".into(),
            "synthetic-deferred-asr-alias".into(),
        );
        previous
            .credential_baselines
            .insert("asr_api_key".into(), "synthetic-older-asr-value".into());
        previous
            .credential_read_states
            .insert("asr_api_key".into(), CredentialReadState::Unavailable);

        let scope =
            custom_credential_recovery_scope(&previous, CustomCredentialPurpose::Asr).unwrap();
        adopt_interactive_custom_credential(
            &mut previous,
            "synthetic-session-custom-value".into(),
            scope,
        );
        previous.cleanup_enabled = true;
        previous.cleanup_provider = crate::engine::EngineProvider::Custom;
        previous.cleanup_base_url = "https://router-b.example.test/v1".into();
        assert!(previous.clear_stale_session_custom_credential());
        assert!(previous.session_custom_credential_rejected);
        assert!(!previous.provider_api_keys.contains_key("custom"));

        // Full settings payloads do not serialize the rejection/read-state
        // metadata and may omit aliases. The production compatibility helper
        // must restore provenance before bind_* restores unchanged-route values.
        let mut full: Settings =
            serde_json::from_value(serde_json::to_value(&previous).unwrap()).unwrap();
        full.asr_api_key.clear();
        crate::preserve_session_custom_credential_for_full_settings_roundtrip(&previous, &mut full);
        full.asr_api_key = bind_asr_key_to_host(
            full.asr_provider,
            &previous.asr_base_url,
            &full.asr_base_url,
            &full.asr_api_key,
            &previous.asr_api_key,
        );
        assert_eq!(full.asr_api_key, "synthetic-deferred-asr-alias");
        assert!(full.session_custom_credential_rejected);
        assert_eq!(full.asr_credential(), "");
        assert_eq!(
            full.credential_read_states.get("asr_api_key"),
            Some(&CredentialReadState::Unavailable)
        );
        assert_eq!(
            full.unverified_credential_sources
                .get("settings:asr_api_key")
                .map(String::as_str),
            Some("synthetic-deferred-asr-alias")
        );
        assert_eq!(
            full.credential_baselines
                .get("asr_api_key")
                .map(String::as_str),
            Some("synthetic-older-asr-value")
        );

        let mut patch = serde_json::to_value(&full).unwrap();
        patch
            .as_object_mut()
            .unwrap()
            .insert("ui_language".into(), serde_json::Value::String("en".into()));
        let mut after_patch: Settings = serde_json::from_value(patch).unwrap();
        crate::restore_session_custom_credential_after_patch(&full, &mut after_patch, false);
        assert_eq!(after_patch.ui_language, "en");
        assert!(after_patch.session_custom_credential_rejected);
        assert_eq!(after_patch.asr_credential(), "");
        assert_eq!(
            after_patch.credential_read_states.get("asr_api_key"),
            Some(&CredentialReadState::Unavailable)
        );
        save_configuration(&dir, &after_patch).unwrap();
        assert!(backend.writes.borrow().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn omitted_full_settings_secret_preserves_unavailable_deferred_source_without_keychain_write() {
        let dir = temp_dir("full-settings-deferred-unavailable");
        let backend = FakeCredentialBackend::default();
        let mut previous = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router.example.test/v1".into(),
            custom_base_url: "https://router.example.test/v1".into(),
            asr_api_key: "synthetic-deferred-key".into(),
            cleanup_enabled: false,
            ..Settings::default()
        };
        previous.unverified_credential_sources.insert(
            "settings:asr_api_key".into(),
            "synthetic-deferred-key".into(),
        );
        previous
            .credential_baselines
            .insert("asr_api_key".into(), "synthetic-prior-baseline".into());
        previous
            .credential_read_states
            .insert("provider_custom".into(), CredentialReadState::Unavailable);
        previous
            .credential_read_states
            .insert("asr_api_key".into(), CredentialReadState::Unavailable);

        let mut full: Settings =
            serde_json::from_value(serde_json::to_value(&previous).unwrap()).unwrap();
        full.asr_api_key.clear();
        crate::preserve_session_custom_credential_for_full_settings_roundtrip(&previous, &mut full);
        full.asr_api_key = bind_asr_key_to_host(
            full.asr_provider,
            &previous.asr_base_url,
            &full.asr_base_url,
            &full.asr_api_key,
            &previous.asr_api_key,
        );
        assert_eq!(full.asr_api_key, "synthetic-deferred-key");
        assert_eq!(
            full.credential_read_states.get("provider_custom"),
            Some(&CredentialReadState::Unavailable)
        );
        assert_eq!(
            full.credential_read_states.get("asr_api_key"),
            Some(&CredentialReadState::Unavailable)
        );
        assert_eq!(
            full.unverified_credential_sources
                .get("settings:asr_api_key")
                .map(String::as_str),
            Some("synthetic-deferred-key")
        );

        // Ordinary preference persistence must not touch secure storage.
        save_configuration(&dir, &full).unwrap();
        let persisted_before = fs::read(dir.join("settings.json")).unwrap();
        assert!(backend.writes.borrow().is_empty());

        // The compatibility service-save path sees the original unavailable
        // read state and refuses migration without writing or relabeling it.
        backend.script_reads(
            "asr_api_key",
            [crate::keychain::ApiKeyState::Unavailable(
                "synthetic Keychain unavailable".into(),
            )],
        );
        assert!(save_settings_with_backend(&dir, &full, &backend).is_err());
        assert!(backend.writes.borrow().is_empty());
        assert_eq!(
            fs::read(dir.join("settings.json")).unwrap(),
            persisted_before
        );
        assert_eq!(
            full.credential_read_states.get("asr_api_key"),
            Some(&CredentialReadState::Unavailable)
        );
        assert_eq!(
            full.unverified_credential_sources
                .get("settings:asr_api_key")
                .map(String::as_str),
            Some("synthetic-deferred-key")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn explicit_full_settings_custom_key_clears_revocation_and_persists() {
        let dir = temp_dir("explicit-custom-after-revocation");
        let backend = FakeCredentialBackend::default();
        let previous = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router.example.test/v1".into(),
            custom_base_url: "https://router.example.test/v1".into(),
            session_custom_credential_rejected: true,
            ..Settings::default()
        };
        let mut incoming: Settings =
            serde_json::from_value(serde_json::to_value(&previous).unwrap()).unwrap();
        incoming
            .provider_api_keys
            .insert("custom".into(), "synthetic-explicit-new-key".into());
        crate::preserve_session_custom_credential_for_full_settings_roundtrip(
            &previous,
            &mut incoming,
        );
        assert!(!incoming.session_custom_credential_rejected);
        assert!(!incoming.session_only_custom_credential);
        assert_eq!(
            incoming.provider_secret(crate::engine::EngineProvider::Custom),
            "synthetic-explicit-new-key"
        );

        save_settings_with_backend(&dir, &incoming, &backend).unwrap();
        assert_eq!(
            backend.writes.borrow().as_slice(),
            &[(
                "provider_custom".into(),
                "synthetic-explicit-new-key".into()
            )]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn revoked_session_custom_credential_cannot_fall_back_to_a_legacy_alias() {
        let mut settings = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router-a.example.test/v1".into(),
            asr_api_key: "synthetic-legacy-a-key".into(),
            cleanup_api_key: "synthetic-legacy-cleanup-key".into(),
            cleanup_enabled: false,
            ..Settings::default()
        };
        let scope =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).unwrap();
        adopt_interactive_custom_credential(
            &mut settings,
            "synthetic-session-origin-key".into(),
            scope,
        );
        assert_eq!(settings.asr_credential(), "synthetic-session-origin-key");

        settings.asr_base_url = "https://router-b.example.test/v1".into();
        assert!(settings.clear_stale_session_custom_credential());
        assert!(settings.session_custom_credential_rejected);
        assert_eq!(settings.asr_credential(), "");
        assert_eq!(
            settings.provider_secret(crate::engine::EngineProvider::Custom),
            ""
        );
    }

    #[test]
    fn session_custom_value_is_limited_to_the_captured_active_purpose_set() {
        let mut settings = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router.example.test/v1/asr".into(),
            cleanup_enabled: true,
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_base_url: "https://router.example.test/v1/cleanup".into(),
            custom_base_url: "https://router.example.test/v1".into(),
            ..Settings::default()
        };
        let scope =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).unwrap();
        adopt_interactive_custom_credential(
            &mut settings,
            "synthetic-shared-custom-key".into(),
            scope,
        );
        assert_eq!(settings.asr_credential(), "synthetic-shared-custom-key");
        assert_eq!(settings.cleanup_credential(), "synthetic-shared-custom-key");

        settings.cleanup_provider = crate::engine::EngineProvider::Groq;
        assert!(settings.clear_stale_session_custom_credential());
        assert_eq!(settings.asr_credential(), "");
        assert_eq!(settings.cleanup_credential(), "");
    }

    #[test]
    fn recovered_custom_credential_does_not_cross_to_a_different_accurate_asr_origin() {
        let mut settings = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router-a.example.test/v1".into(),
            accurate_asr_provider: crate::engine::EngineProvider::Custom,
            accurate_asr_base_url: "https://router-b.example.test/v1".into(),
            ..Settings::default()
        };
        let scope =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).unwrap();
        adopt_interactive_custom_credential(&mut settings, "synthetic-router-a-key".into(), scope);
        assert_eq!(settings.asr_credential(), "synthetic-router-a-key");
        assert_eq!(settings.accurate_asr_credential(), "");

        settings.accurate_asr_base_url = "https://router-a.example.test/alternate".into();
        assert_eq!(settings.accurate_asr_credential(), "synthetic-router-a-key");
    }

    #[test]
    fn on_device_mlx_cleanup_is_ready_without_url_or_api_key() {
        let settings = Settings {
            cleanup_provider: crate::engine::EngineProvider::OnDeviceMlx,
            cleanup_model: crate::engine::EngineProvider::OnDeviceMlx
                .default_llm_model()
                .into(),
            cleanup_base_url: String::new(),
            cleanup_api_key: String::new(),
            ..Settings::default()
        };
        settings
            .validate_provider_side(
                crate::engine::EngineProvider::OnDeviceMlx,
                false,
                None,
                true,
            )
            .expect("the bundled native provider has no URL or API key");
        assert_eq!(settings.cleanup_endpoint(), "");
        assert_eq!(settings.cleanup_credential(), "");
    }

    #[test]
    fn schema_27_moves_retired_lmstudio_cleanup_to_the_bundled_mlx_route() {
        let dir = temp_dir("retired-lmstudio-cleanup");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 26,
                "cleanup_enabled": true,
                "cleanup_provider": "lmstudio",
                "cleanup_model": "qwen/qwen3.8-27b@6bit",
                "strict_offline_enabled": true,
            }))
            .unwrap(),
        )
        .unwrap();
        let backend = FakeCredentialBackend::default();

        let (settings, needs_persist) = load_settings_with_backend(&dir, &backend);

        assert!(needs_persist);
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(
            settings.cleanup_provider,
            crate::engine::EngineProvider::OnDeviceMlx
        );
        assert_eq!(settings.cleanup_model, crate::modelstore::MODEL_ID);
        assert!(settings.cleanup_enabled);
        assert!(settings.strict_offline_enabled);
        assert!(dir.join("settings.json.pre-migration.bak").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn schema_27_repairs_only_pre_migration_mlx_selections_of_the_lmstudio_model() {
        let mut legacy = Settings {
            schema_version: 26,
            cleanup_provider: crate::engine::EngineProvider::OnDeviceMlx,
            cleanup_model: RETIRED_LMSTUDIO_CLEANUP_MODEL.into(),
            ..Settings::default()
        };
        legacy.normalize();
        assert_eq!(legacy.cleanup_model, crate::modelstore::MODEL_ID);

        let mut ollama = Settings {
            schema_version: 26,
            cleanup_provider: crate::engine::EngineProvider::Ollama,
            cleanup_model: RETIRED_LMSTUDIO_CLEANUP_MODEL.into(),
            ..Settings::default()
        };
        ollama.normalize();
        assert_eq!(ollama.cleanup_model, RETIRED_LMSTUDIO_CLEANUP_MODEL);

        assert!(
            serde_json::from_value::<crate::engine::EngineProvider>(serde_json::json!("lmstudio"))
                .is_err()
        );
        assert!(serde_json::from_value::<Settings>(serde_json::json!({
            "cleanup_provider": "not_a_provider"
        }))
        .is_err());
    }

    #[test]
    fn history_schema_migration_keeps_a_versioned_backup() {
        let dir = temp_dir("history-migration");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
        .execute_batch(
                "CREATE TABLE dictations (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, duration_secs REAL, raw_text TEXT, final_text TEXT, engine TEXT, degraded INTEGER, status TEXT, raw_audio_path TEXT); INSERT INTO dictations (created_at,raw_text,final_text) VALUES (datetime('now'),'legacy raw','legacy final'); PRAGMA user_version=2;",
            )
            .unwrap();
        drop(connection);

        let history = get_history(&dir, 10).unwrap();

        let backup = dir.join("history.sqlite.v2.bak");
        assert!(backup.is_file());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let backup_db =
            Connection::open_with_flags(&backup, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let integrity: String = backup_db
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        assert_eq!(
            backup_db
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
                .unwrap(),
            2
        );
        assert_eq!(history[0].asr_text, None);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn clear_all_data_removes_backups_created_while_migrating_legacy_history() {
        let dir = temp_dir("clear-legacy-migration-backups");
        fs::create_dir_all(&dir).unwrap();
        let connection = Connection::open(dir.join("history.sqlite")).unwrap();
        connection.execute_batch("CREATE TABLE dictations (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, duration_secs REAL, raw_text TEXT, final_text TEXT, engine TEXT, degraded INTEGER, status TEXT, raw_audio_path TEXT); INSERT INTO dictations (created_at,raw_text,final_text) VALUES (datetime('now'),'private legacy','private legacy'); PRAGMA user_version=2;").unwrap();
        drop(connection);
        fs::write(dir.join("history.sqlite.v1.bak"), "private previous").unwrap();
        fs::write(dir.join("history.sqlite.v2.bak.user"), "user export").unwrap();
        fs::write(dir.join("settings.json"), "user settings").unwrap();
        clear_all_data(&dir).unwrap();
        assert!(!dir.join("history.sqlite.v1.bak").exists());
        assert!(!dir.join("history.sqlite.v2.bak").exists());
        assert!(get_history(&dir, 10).unwrap().is_empty());
        assert_eq!(
            fs::read_to_string(dir.join("history.sqlite.v2.bak.user")).unwrap(),
            "user export"
        );
        assert_eq!(
            fs::read_to_string(dir.join("settings.json")).unwrap(),
            "user settings"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn schema_17_adds_nullable_cleanup_attribution_and_keeps_existing_rows() {
        let dir = temp_dir("schema-17-cleanup-attribution");
        fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("history.sqlite");
        let connection = Connection::open(&db_path).unwrap();
        connection.execute_batch("CREATE TABLE dictations (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, duration_secs REAL, raw_text TEXT, final_text TEXT, cleanup_status TEXT, engine TEXT, degraded INTEGER, degraded_reason TEXT, status TEXT, raw_audio_path TEXT, delivery_method TEXT, fallback_reason TEXT, context_profile_id TEXT, context_policy_json TEXT, asr_text TEXT, provider_cleaned_candidate TEXT, cleanup_rejected_candidate TEXT, cleanup_guard_reason TEXT); CREATE TABLE dictation_revisions (revision_id INTEGER PRIMARY KEY, dictation_id INTEGER NOT NULL, created_at TEXT NOT NULL, final_text TEXT NOT NULL, cleanup_status TEXT, intent_json TEXT, model TEXT, context_policy_json TEXT, revision_reason TEXT NOT NULL); INSERT INTO dictations(id,created_at,duration_secs,raw_text,final_text,cleanup_status,engine,degraded,degraded_reason,status,raw_audio_path,delivery_method,fallback_reason,context_profile_id,context_policy_json,asr_text,provider_cleaned_candidate,cleanup_rejected_candidate,cleanup_guard_reason) VALUES(41,'2026-11-01',3.5,'old raw','old final','local_only','whisper',1,'provider_failure','degraded','spool/session.wav','clipboard','delivery_fallback','profile-7','{\"family\":\"document\"}','old asr','provider candidate','rejected candidate','protected_token_changed'); INSERT INTO dictation_revisions(revision_id,dictation_id,created_at,final_text,cleanup_status,intent_json,model,context_policy_json,revision_reason) VALUES(9,41,'2026-11-02','old revision','local_only','{}','model-7','{\"family\":\"document\"}','manual'); PRAGMA user_version=17;").unwrap();
        let old_columns: Vec<String> = {
            let mut columns_statement =
                connection.prepare("PRAGMA table_info(dictations)").unwrap();
            columns_statement
                .query_map([], |row| row.get(1))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert!(!old_columns
            .iter()
            .any(|column| column == "cleanup_attempt_kind"));
        assert!(!old_columns
            .iter()
            .any(|column| column == "cleanup_transformations_json"));
        assert!(!old_columns
            .iter()
            .any(|column| column == "cleanup_final_guard_reason"));
        assert!(!old_columns
            .iter()
            .any(|column| column == "cleanup_failure_reason"));
        drop(connection);
        let item = get_history_page(&dir, 10, None, None)
            .unwrap()
            .items
            .remove(0);
        assert_eq!(item.raw_text, "old raw");
        assert_eq!(item.asr_text.as_deref(), Some("old asr"));
        assert_eq!(
            item.provider_cleaned_candidate.as_deref(),
            Some("provider candidate")
        );
        assert_eq!(
            item.cleanup_rejected_candidate.as_deref(),
            Some("rejected candidate")
        );
        assert_eq!(
            item.cleanup_guard_reason.as_deref(),
            Some("protected_token_changed")
        );
        assert_eq!(item.context_profile_id.as_deref(), Some("profile-7"));
        let persisted_audio_path: Option<String> = Connection::open(&db_path)
            .unwrap()
            .query_row(
                "SELECT raw_audio_path FROM dictations WHERE id=41",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(persisted_audio_path.as_deref(), Some("spool/session.wav"));
        assert_eq!(item.delivery_method.as_deref(), Some("clipboard"));
        assert_eq!(item.cleanup_attempt_kind, None);
        assert_eq!(item.cleanup_transformations, None);
        assert_eq!(item.revision_count, 1);
        let migrated: Connection = open_history(&dir).unwrap();
        assert_eq!(
            migrated
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
                .unwrap(),
            HISTORY_SCHEMA_VERSION
        );
        migrated.execute("INSERT INTO dictations(id,created_at,raw_text,final_text,cleanup_status,status,cleanup_attempt_kind,cleanup_transformations_json,cleanup_final_guard_reason,cleanup_failure_reason) VALUES(42,'2026-11-03','new raw','new final','cleanup_not_requested','ok','not_called','[]',NULL,NULL)", []).unwrap();
        drop(migrated);
        let reopened = get_history_page(&dir, 10, None, None).unwrap();
        assert_eq!(
            reopened.items[0].cleanup_attempt_kind.as_deref(),
            Some("not_called")
        );
        assert_eq!(
            reopened.items[0].cleanup_transformations.as_deref(),
            Some([].as_slice())
        );
        assert_eq!(reopened.items[0].cleanup_final_guard_reason, None);
        assert_eq!(reopened.items[0].cleanup_failure_reason, None);
        let export: serde_json::Value =
            serde_json::from_str(&export_history_json(&dir).unwrap()).unwrap();
        assert_eq!(export[0]["cleanup_attempt_kind"], "not_called");
        assert_eq!(export[0]["cleanup_transformations"], serde_json::json!([]));
        assert!(export[0]["cleanup_final_guard_reason"].is_null());
        assert!(export[0]["cleanup_failure_reason"].is_null());
        assert!(export[1]["cleanup_attempt_kind"].is_null());
        let backup = dir.join("history.sqlite.v17.bak");
        assert!(backup.is_file());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let backup_db =
            Connection::open_with_flags(&backup, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        assert_eq!(
            backup_db
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(
            backup_db
                .query_row(
                    "SELECT final_text FROM dictation_revisions WHERE revision_id=9",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "old revision"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn newer_history_schema_is_rejected_without_changing_database_bytes() {
        let dir = temp_dir("future-history-schema");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE future_only(id INTEGER PRIMARY KEY, sentinel TEXT); INSERT INTO future_only VALUES(1,'keep'); CREATE TABLE dictations(id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, raw_text TEXT, final_text TEXT); INSERT INTO dictations VALUES(2,'2026-11-01','keep raw','keep final');").unwrap();
        connection
            .pragma_update(None, "user_version", HISTORY_SCHEMA_VERSION + 1)
            .unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        assert!(open_history(&dir).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(!dir
            .join(format!("history.sqlite.v{}.bak", HISTORY_SCHEMA_VERSION))
            .exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_backups_have_finite_retention_even_with_forever_live_history() {
        let dir = temp_dir("migration-backup-retention");
        fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        for (version, days) in [(1, 8), (2, 3), (3, 0)] {
            let file = File::create(dir.join(format!("history.sqlite.v{version}.bak"))).unwrap();
            file.set_times(
                fs::FileTimes::new().set_modified(now - Duration::from_secs(days * 86_400)),
            )
            .unwrap();
        }
        assert_eq!(purge_history(&dir, 0).unwrap(), 0);
        assert!(!dir.join("history.sqlite.v1.bak").exists());
        assert!(dir.join("history.sqlite.v2.bak").exists());
        assert!(dir.join("history.sqlite.v3.bak").exists());
        purge_history(&dir, 1).unwrap();
        assert!(!dir.join("history.sqlite.v2.bak").exists());
        assert!(dir.join("history.sqlite.v3.bak").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_backup_cleanup_ignores_future_dates_and_nonowned_names() {
        let dir = temp_dir("migration-backup-boundary");
        fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        for name in [
            "history.sqlite.v1.bak",
            "history.sqlite.v02.bak",
            "history.sqlite.v-1.bak",
            "settings.json.pre-migration.bak",
            "history.sqlite.v2.bak.user",
        ] {
            fs::write(dir.join(name), "private").unwrap();
        }
        File::open(dir.join("history.sqlite.v1.bak"))
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(now + Duration::from_secs(86_400)))
            .unwrap();
        fs::create_dir(dir.join("history.sqlite.v3.bak")).unwrap();
        assert_eq!(
            prune_history_migration_backups(&dir, Some(7), now).unwrap(),
            0
        );
        assert_eq!(prune_history_migration_backups(&dir, None, now).unwrap(), 1);
        assert!(dir.join("history.sqlite.v02.bak").exists());
        assert!(dir.join("history.sqlite.v-1.bak").exists());
        assert!(dir.join("settings.json.pre-migration.bak").exists());
        assert!(dir.join("history.sqlite.v2.bak.user").exists());
        assert!(dir.join("history.sqlite.v3.bak").is_dir());
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn migration_backup_links_are_neither_followed_nor_overwritten() {
        use std::os::unix::fs::symlink;
        let dir = temp_dir("migration-backup-links");
        fs::create_dir_all(&dir).unwrap();
        let external = dir.join("user-backup.txt");
        fs::write(&external, "user-owned private backup").unwrap();
        symlink(&external, dir.join("history.sqlite.v1.bak")).unwrap();
        let dangling_target = dir.join("must-not-create.txt");
        symlink(&dangling_target, dir.join("history.sqlite.v2.bak")).unwrap();
        assert_eq!(
            prune_history_migration_backups(&dir, None, SystemTime::now()).unwrap(),
            0
        );
        assert_eq!(
            fs::read_to_string(&external).unwrap(),
            "user-owned private backup"
        );
        let connection = Connection::open(dir.join("history.sqlite")).unwrap();
        connection.pragma_update(None, "user_version", 2).unwrap();
        drop(connection);
        assert!(open_history(&dir).is_err());
        assert!(!dangling_target.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_backup_does_not_overwrite_existing_regular_file() {
        let dir = temp_dir("migration-backup-existing-file");
        fs::create_dir_all(&dir).unwrap();
        let database = Connection::open(dir.join("history.sqlite")).unwrap();
        database.execute_batch("PRAGMA user_version=2;").unwrap();
        drop(database);
        let backup = dir.join("history.sqlite.v2.bak");
        fs::write(&backup, b"preexisting owner data").unwrap();

        open_history(&dir).unwrap();

        assert_eq!(fs::read(&backup).unwrap(), b"preexisting owner data");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_backup_temp_collision_preserves_existing_directory_contents() {
        let dir = temp_dir("migration-temp-collision");
        fs::create_dir_all(&dir).unwrap();
        let occupied = dir.join("occupied-stage");
        fs::create_dir(&occupied).unwrap();
        let sentinel = occupied.join("owner-data.txt");
        fs::write(&sentinel, b"preexisting owner data").unwrap();

        assert!(create_history_backup_temp_dir(&occupied).is_err());

        assert_eq!(fs::read(&sentinel).unwrap(), b"preexisting owner data");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_backup_is_integral_during_concurrent_wal_writes() {
        let dir = temp_dir("migration-backup-wal-writer");
        fs::create_dir_all(&dir).unwrap();
        let database_path = dir.join("history.sqlite");
        let database = Connection::open(&database_path).unwrap();
        database
            .execute_batch(
                "PRAGMA journal_mode=WAL; CREATE TABLE dictations (id INTEGER PRIMARY KEY, value TEXT NOT NULL); INSERT INTO dictations VALUES (1, 'initial'); PRAGMA user_version=2;",
            )
            .unwrap();
        let writer_path = database_path.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            let writer_db = Connection::open(writer_path).unwrap();
            writer_db.busy_timeout(Duration::from_secs(2)).unwrap();
            started_tx.send(()).unwrap();
            let mut revision = 0_u64;
            while stop_rx.try_recv().is_err() {
                revision += 1;
                let _ = writer_db.execute(
                    "UPDATE dictations SET value=?1 WHERE id=1",
                    [revision.to_string()],
                );
                std::thread::yield_now();
            }
        });
        started_rx.recv().unwrap();

        let backup = dir.join("history.sqlite.v2.bak");
        create_history_migration_backup(&database, &dir, &backup).unwrap();

        stop_tx.send(()).unwrap();
        writer.join().unwrap();
        let backup_db =
            Connection::open_with_flags(&backup, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let integrity: String = backup_db
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        let value: String = backup_db
            .query_row("SELECT value FROM dictations WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(!value.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn save_settings_never_writes_plaintext_key() {
        let dir = temp_dir("keyblank");
        let backend = FakeCredentialBackend::default();
        let settings = Settings {
            api_key: "test_key_should_not_be_on_disk".into(),
            provider_api_keys: [("openai".to_owned(), "synthetic-openai-key".to_owned())]
                .into_iter()
                .collect(),
            ..Settings::default()
        };
        save_settings_with_backend(&dir, &settings, &backend).unwrap();
        let on_disk = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            !on_disk.contains("test_key_should_not_be_on_disk"),
            "plaintext API key must never be written to settings.json"
        );
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(parsed["api_key"].as_str().unwrap_or(""), "");
        assert_eq!(
            backend
                .values
                .borrow()
                .get("groq_api_key")
                .map(String::as_str),
            Some("test_key_should_not_be_on_disk")
        );
        assert_eq!(
            backend
                .values
                .borrow()
                .get("provider_openai")
                .map(String::as_str),
            Some("synthetic-openai-key")
        );
        assert!(!on_disk.contains("synthetic-openai-key"));
        assert!(!secret_sidecar_path(&dir, "api_key").exists());
        assert!(!secret_sidecar_path(&dir, "provider_openai").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn secure_store_failure_preserves_sources_and_creates_no_plaintext() {
        let dir = temp_dir("credential-write-failure");
        std::fs::create_dir_all(dir.join("secrets")).unwrap();
        let previous_settings = br#"{"api_key":"legacy-original","onboarded":false}"#;
        std::fs::write(dir.join("settings.json"), previous_settings).unwrap();
        write_secret_sidecar(&dir, "api_key", "legacy-sidecar").unwrap();
        let previous_sidecar = std::fs::read(secret_sidecar_path(&dir, "api_key")).unwrap();
        let backend = FakeCredentialBackend::default();
        backend.fail_write("groq_api_key", "synthetic secure-store failure");
        let settings = Settings {
            api_key: "new-synthetic-key".into(),
            ..Settings::default()
        };

        let error = save_settings_with_backend(&dir, &settings, &backend).unwrap_err();

        assert!(error.to_string().contains("credential_storage"));
        assert_eq!(
            std::fs::read(dir.join("settings.json")).unwrap(),
            previous_settings
        );
        assert_eq!(
            std::fs::read(secret_sidecar_path(&dir, "api_key")).unwrap(),
            previous_sidecar
        );
        assert!(
            !std::fs::read_to_string(secret_sidecar_path(&dir, "api_key"))
                .unwrap()
                .contains("new-synthetic-key")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn same_baseline_key_is_not_rewritten_when_keychain_is_unavailable() {
        let dir = temp_dir("same-baseline-unavailable");
        let backend = FakeCredentialBackend::default();
        backend.script_reads(
            "groq_api_key",
            [crate::keychain::ApiKeyState::Unavailable(
                "synthetic unavailable".into(),
            )],
        );
        let mut settings = Settings::default();
        adopt_interactive_groq_credential(&mut settings, "same-baseline-key".into());

        let result = save_settings_with_backend(&dir, &settings, &backend);

        assert!(result.is_err());
        assert!(backend.writes.borrow().is_empty());
        assert!(!dir.join("settings.json").exists());
        assert!(!secret_sidecar_path(&dir, "api_key").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn interactive_groq_read_adoption_updates_only_ram_baseline() {
        let mut settings = Settings::default();
        settings
            .unverified_credential_sources
            .insert("settings:api_key".into(), "legacy-key".into());

        adopt_interactive_groq_credential(&mut settings, "session-key".into());

        assert_eq!(settings.api_key, "session-key");
        assert_eq!(
            settings.provider_api_keys.get("groq").unwrap(),
            "session-key"
        );
        assert_eq!(
            settings.credential_baselines.get("groq_api_key").unwrap(),
            "session-key"
        );
        assert!(!settings
            .unverified_credential_sources
            .contains_key("settings:api_key"));
        let serialized = serde_json::to_string(&SettingsView::from(&settings)).unwrap();
        assert!(!serialized.contains("session-key"));
    }

    #[test]
    fn verified_settings_migration_scrubs_settings_only_after_secure_readback() {
        let dir = temp_dir("settings-credential-migration");
        std::fs::create_dir_all(&dir).unwrap();
        let legacy_settings =
            br#"{"schema_version":20,"api_key":"synthetic-legacy-key","onboarded":false}"#;
        std::fs::write(dir.join("settings.json"), legacy_settings).unwrap();
        let backend = FakeCredentialBackend::default();

        let (settings, needs_persist) = load_settings_with_backend(&dir, &backend);

        assert_eq!(settings.api_key, "synthetic-legacy-key");
        assert!(needs_persist);
        assert_eq!(
            backend
                .values
                .borrow()
                .get("groq_api_key")
                .map(String::as_str),
            Some("synthetic-legacy-key")
        );
        save_settings_with_backend(&dir, &settings, &backend).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(saved["api_key"].as_str(), Some(""));
        assert!(!std::fs::read_to_string(dir.join("settings.json"))
            .unwrap()
            .contains("synthetic-legacy-key"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn verified_sidecar_migration_removes_only_matching_legacy_file() {
        let dir = temp_dir("sidecar-migration");
        write_secret_sidecar(&dir, "api_key", "synthetic-groq-legacy").unwrap();
        write_secret_sidecar(&dir, "provider_openai", "unrelated-openai-sidecar").unwrap();
        let backend = FakeCredentialBackend::default();
        let mut settings = Settings::default();

        let resolution =
            resolve_legacy_credential(&dir, "groq_api_key", "", None, &mut settings, &backend);

        assert_eq!(resolution.value, "synthetic-groq-legacy");
        assert!(matches!(
            resolution.state,
            crate::keychain::ApiKeyState::Configured(ref key) if key == "synthetic-groq-legacy"
        ));
        assert!(!secret_sidecar_path(&dir, "api_key").exists());
        assert_eq!(
            read_secret_sidecar(&dir, "provider_openai").as_deref(),
            Some("unrelated-openai-sidecar")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn provider_and_custom_alias_sidecars_migrate_to_their_secure_accounts() {
        let provider_dir = temp_dir("provider-sidecar-migration");
        write_secret_sidecar(&provider_dir, "provider_openai", "synthetic-provider-key").unwrap();
        let provider_backend = FakeCredentialBackend::default();
        let mut provider_settings = Settings::default();
        let provider_resolution = resolve_legacy_credential(
            &provider_dir,
            "provider_openai",
            "",
            None,
            &mut provider_settings,
            &provider_backend,
        );
        assert_eq!(provider_resolution.value, "synthetic-provider-key");
        assert_eq!(
            provider_backend
                .values
                .borrow()
                .get("provider_openai")
                .map(String::as_str),
            Some("synthetic-provider-key")
        );
        assert!(!secret_sidecar_path(&provider_dir, "provider_openai").exists());

        let custom_dir = temp_dir("custom-alias-sidecar-migration");
        write_secret_sidecar(&custom_dir, "asr_api_key", "synthetic-custom-asr").unwrap();
        write_secret_sidecar(&custom_dir, "cleanup_api_key", "synthetic-custom-cleanup").unwrap();
        let custom_backend = FakeCredentialBackend::default();
        let (custom_settings, _) = load_settings_with_backend(&custom_dir, &custom_backend);
        assert_eq!(custom_settings.asr_api_key, "synthetic-custom-asr");
        assert_eq!(custom_settings.cleanup_api_key, "synthetic-custom-cleanup");
        assert_eq!(
            custom_backend
                .values
                .borrow()
                .get("asr_api_key")
                .map(String::as_str),
            Some("synthetic-custom-asr")
        );
        assert_eq!(
            custom_backend
                .values
                .borrow()
                .get("cleanup_api_key")
                .map(String::as_str),
            Some("synthetic-custom-cleanup")
        );
        assert!(!secret_sidecar_path(&custom_dir, "asr_api_key").exists());
        assert!(!secret_sidecar_path(&custom_dir, "cleanup_api_key").exists());
        let _ = std::fs::remove_dir_all(provider_dir);
        let _ = std::fs::remove_dir_all(custom_dir);
    }

    #[test]
    fn failed_or_unverified_legacy_migration_retains_settings_and_sidecar_sources() {
        let dir = temp_dir("failed-credential-migration");
        std::fs::create_dir_all(&dir).unwrap();
        let raw =
            br#"{"schema_version":22,"api_key":"synthetic-settings-source","onboarded":false}"#;
        std::fs::write(dir.join("settings.json"), raw).unwrap();
        write_secret_sidecar(&dir, "api_key", "synthetic-sidecar-source").unwrap();
        let backend = FakeCredentialBackend::default();
        backend.script_reads(
            "groq_api_key",
            [
                crate::keychain::ApiKeyState::Missing,
                crate::keychain::ApiKeyState::Missing,
                crate::keychain::ApiKeyState::Missing,
            ],
        );
        backend.fail_write("groq_api_key", "synthetic write failure");

        let (settings, needs_persist) = load_settings_with_backend(&dir, &backend);

        assert_eq!(settings.api_key, "synthetic-settings-source");
        // Schema 23 requests a save, but a failed credential migration must
        // still prevent overwriting the original source configuration.
        assert!(needs_persist);
        assert!(save_settings_with_backend(&dir, &settings, &backend).is_err());
        assert_eq!(std::fs::read(dir.join("settings.json")).unwrap(), raw);
        assert_eq!(
            read_secret_sidecar(&dir, "api_key").as_deref(),
            Some("synthetic-sidecar-source")
        );
        assert!(settings
            .unverified_credential_sources
            .contains_key("settings:api_key"));

        let unverifiable_dir = temp_dir("unverified-credential-migration");
        write_secret_sidecar(&unverifiable_dir, "api_key", "synthetic-unverified-sidecar").unwrap();
        let unverifiable_backend = FakeCredentialBackend::default();
        unverifiable_backend.script_reads(
            "groq_api_key",
            [
                crate::keychain::ApiKeyState::Missing,
                crate::keychain::ApiKeyState::Missing,
                crate::keychain::ApiKeyState::Missing,
            ],
        );
        let mut unresolved = Settings::default();
        let result = resolve_legacy_credential(
            &unverifiable_dir,
            "groq_api_key",
            "",
            None,
            &mut unresolved,
            &unverifiable_backend,
        );
        assert_eq!(result.value, "synthetic-unverified-sidecar");
        assert!(matches!(
            result.state,
            crate::keychain::ApiKeyState::Unavailable(_)
        ));
        assert_eq!(
            read_secret_sidecar(&unverifiable_dir, "api_key").as_deref(),
            Some("synthetic-unverified-sidecar")
        );
        assert!(unresolved
            .unverified_credential_sources
            .contains_key("sidecar:groq_api_key"));
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(unverifiable_dir);
    }

    #[test]
    fn existing_secure_credential_wins_over_stale_sidecar_without_removing_it() {
        let dir = temp_dir("secure-precedence");
        write_secret_sidecar(&dir, "api_key", "stale-sidecar-value").unwrap();
        let backend = FakeCredentialBackend::default();
        backend.seed("groq_api_key", "newer-secure-value");
        let mut settings = Settings::default();

        let resolution =
            resolve_legacy_credential(&dir, "groq_api_key", "", None, &mut settings, &backend);

        assert_eq!(resolution.value, "newer-secure-value");
        assert!(backend.writes.borrow().is_empty());
        assert_eq!(
            read_secret_sidecar(&dir, "api_key").as_deref(),
            Some("stale-sidecar-value")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn explicit_key_replacement_updates_secure_value_and_clears_legacy_alias() {
        let dir = temp_dir("explicit-credential-replacement");
        write_secret_sidecar(&dir, "groq_api_key", "synthetic-old-sidecar").unwrap();
        let backend = FakeCredentialBackend::default();
        backend.seed("groq_api_key", "synthetic-old-secure");
        let mut settings = Settings {
            api_key: "synthetic-new-key".into(),
            ..Settings::default()
        };
        settings
            .credential_baselines
            .insert("groq_api_key".into(), "synthetic-old-secure".into());

        save_settings_with_backend(&dir, &settings, &backend).unwrap();

        assert_eq!(
            backend
                .values
                .borrow()
                .get("groq_api_key")
                .map(String::as_str),
            Some("synthetic-new-key")
        );
        assert!(!secret_sidecar_path(&dir, "api_key").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn explicit_provider_deletion_clears_groq_and_custom_legacy_aliases() {
        let dir = temp_dir("credential-alias-delete");
        for slot in [
            "api_key",
            "provider_custom",
            "asr_api_key",
            "cleanup_api_key",
        ] {
            write_secret_sidecar(&dir, slot, &format!("synthetic-{slot}")).unwrap();
        }

        clear_provider_key_sidecars(&dir, crate::engine::EngineProvider::Groq);
        assert!(!secret_sidecar_path(&dir, "groq_api_key").exists());
        clear_provider_key_sidecars(&dir, crate::engine::EngineProvider::Custom);
        for slot in ["provider_custom", "asr_api_key", "cleanup_api_key"] {
            assert!(!secret_sidecar_path(&dir, slot).exists());
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    fn write_custom_host_settings_fixture(dir: &Path) -> Vec<u8> {
        let original = br#"{"custom_base_url":"https://old.example/v1"}"#.to_vec();
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("settings.json"), &original).unwrap();
        original
    }

    fn seed_custom_host_credentials(backend: &FakeCredentialBackend) {
        backend.seed("provider_custom", "old-custom-key");
        backend.seed("asr_api_key", "old-asr-alias");
        backend.seed("cleanup_api_key", "old-cleanup-alias");
        backend.seed("api_key", "keep-groq-key");
        backend.seed("provider_openai", "keep-openai-key");
    }

    fn assert_custom_host_settings_unchanged(dir: &Path, original: &[u8]) {
        assert_eq!(std::fs::read(dir.join("settings.json")).unwrap(), original);
    }

    #[test]
    fn custom_host_change_clear_failure_keeps_settings_and_unrelated_keys() {
        for failing_slot in CUSTOM_HOST_CHANGE_CREDENTIAL_SLOTS {
            let dir = temp_dir(&format!("custom-host-clear-{failing_slot}"));
            let original = write_custom_host_settings_fixture(&dir);
            let backend = FakeCredentialBackend::default();
            seed_custom_host_credentials(&backend);
            backend.fail_write(failing_slot, "injected delete failure");

            assert!(
                clear_custom_credentials_before_host_change_with_backend(&dir, &backend).is_err()
            );
            assert_custom_host_settings_unchanged(&dir, &original);
            assert_eq!(
                backend.read("api_key"),
                crate::keychain::ApiKeyState::Configured("keep-groq-key".into())
            );
            assert_eq!(
                backend.read("provider_openai"),
                crate::keychain::ApiKeyState::Configured("keep-openai-key".into())
            );
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn custom_host_change_clear_unavailable_readback_keeps_settings() {
        let dir = temp_dir("custom-host-clear-unavailable-readback");
        let original = write_custom_host_settings_fixture(&dir);
        let backend = FakeCredentialBackend::default();
        seed_custom_host_credentials(&backend);
        backend.script_reads(
            "provider_custom",
            [crate::keychain::ApiKeyState::Unavailable(
                "injected readback failure".into(),
            )],
        );

        assert!(clear_custom_credentials_before_host_change_with_backend(&dir, &backend).is_err());
        assert_custom_host_settings_unchanged(&dir, &original);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn custom_host_change_clear_sidecar_failure_keeps_settings() {
        let dir = temp_dir("custom-host-clear-sidecar-failure");
        let original = write_custom_host_settings_fixture(&dir);
        let backend = FakeCredentialBackend::default();
        seed_custom_host_credentials(&backend);
        write_secret_sidecar(&dir, "provider_custom", "old-custom-sidecar").unwrap();
        write_secret_sidecar(&dir, "cleanup_api_key", "old-cleanup-sidecar").unwrap();
        let blocked_sidecar = secret_sidecar_path(&dir, "asr_api_key");
        std::fs::create_dir(&blocked_sidecar).unwrap();

        assert!(clear_custom_credentials_before_host_change_with_backend(&dir, &backend).is_err());
        assert_custom_host_settings_unchanged(&dir, &original);
        assert!(blocked_sidecar.is_dir());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn custom_host_change_clear_removes_all_custom_slots_before_verifying_fallback() {
        let dir = temp_dir("custom-host-clear-success");
        let original = write_custom_host_settings_fixture(&dir);
        let backend = FakeCredentialBackend::default();
        seed_custom_host_credentials(&backend);
        for slot in ["provider_custom", "asr_api_key", "cleanup_api_key"] {
            write_secret_sidecar(&dir, slot, &format!("old-{slot}-sidecar")).unwrap();
        }

        clear_custom_credentials_before_host_change_with_backend(&dir, &backend).unwrap();

        for slot in CUSTOM_HOST_CHANGE_CREDENTIAL_SLOTS {
            assert_eq!(backend.read(slot), crate::keychain::ApiKeyState::Missing);
            assert!(!secret_sidecar_path(&dir, slot).exists());
        }
        assert_eq!(
            backend.read("api_key"),
            crate::keychain::ApiKeyState::Configured("keep-groq-key".into())
        );
        assert_eq!(
            backend.read("provider_openai"),
            crate::keychain::ApiKeyState::Configured("keep-openai-key".into())
        );
        assert_custom_host_settings_unchanged(&dir, &original);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn load_settings_reads_api_key_from_sidecar_when_keychain_is_empty() {
        let dir = temp_dir("sidecar-load");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"onboarded":true,"api_key":""}"#,
        )
        .unwrap();
        write_secret_sidecar(&dir, "api_key", "gsk_from_sidecar").unwrap();
        let (settings, _) = load_settings(&dir);
        assert_eq!(settings.api_key, "gsk_from_sidecar");
        assert!(settings.onboarded);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn credential_hint_shows_last_five_characters() {
        assert_eq!(
            credential_hint("gsk_abcdefghij").as_deref(),
            Some("••••fghij")
        );
        assert_eq!(credential_hint("only").as_deref(), Some("••••"));
        assert_eq!(credential_hint("🔑秘密").as_deref(), Some("••••"));
        assert_eq!(credential_hint("asr_only").as_deref(), Some("••••_only"));
    }

    #[test]
    fn settings_view_redacts_api_key() {
        let settings = Settings {
            api_key: "test_key_1234".into(),
            ..Settings::default()
        };
        let view = SettingsView::from(&settings);
        let json = serde_json::to_string(&view).unwrap();
        assert!(view.api_key_configured);
        assert_eq!(view.api_key_hint.as_deref(), Some("••••_1234"));
        assert_eq!(view.asr_model, crate::asr::MODEL);
        assert_eq!(view.cleanup_model, crate::llm::MODEL);
        assert!(!json.contains("test_key_1234"));
        assert!(!json.contains("secret"));
    }

    #[test]
    fn custom_cleanup_parameters_round_trip_through_settings_view_and_validate() {
        let mut settings = Settings {
            custom_cleanup_parameters: serde_json::from_value(serde_json::json!({
                "temperature": 0,
                "vendor_extension": {"enabled": false, "bias": 0},
            }))
            .expect("parameter object"),
            ..Settings::default()
        };
        settings
            .validate_configuration()
            .expect("bounded parameters validate");
        let view = SettingsView::from(&settings);
        assert_eq!(
            view.custom_cleanup_parameters,
            settings.custom_cleanup_parameters
        );

        settings
            .custom_cleanup_parameters
            .insert("messages".into(), serde_json::Value::Array(Vec::new()));
        assert!(settings.validate_configuration().is_err());
    }

    #[test]
    fn settings_view_exposes_configured_asr_model() {
        let settings = Settings {
            asr_model: "whisper-1".into(),
            ..Settings::default()
        };
        assert_eq!(SettingsView::from(&settings).asr_model, "whisper-1");
    }

    #[test]
    fn onboarding_requires_the_selected_provider_credential() {
        let settings = Settings {
            onboarded: true,
            api_key: String::new(),
            cleanup_enabled: false,
            ..Settings::default()
        };
        assert!(settings.validate().is_err());
    }

    #[test]
    fn onboarding_accepts_selected_provider_key_without_a_groq_key() {
        let mut settings = Settings {
            onboarded: true,
            api_key: String::new(),
            asr_provider: crate::engine::EngineProvider::SiliconFlow,
            asr_model: "FunAudioLLM/SenseVoiceSmall".into(),
            cleanup_provider: crate::engine::EngineProvider::SiliconFlow,
            cleanup_model: "Qwen/Qwen2.5-7B-Instruct".into(),
            ..Settings::default()
        };
        settings
            .provider_api_keys
            .insert("siliconflow".into(), "configured-in-memory".into());
        settings
            .validate()
            .expect("selected ASR/cleanup provider keys validate");
    }

    #[test]
    fn onboarding_accepts_keyless_loopback_asr_without_a_groq_key() {
        let settings = Settings {
            onboarded: true,
            api_key: String::new(),
            asr_provider: crate::engine::EngineProvider::LocalWhisper,
            cleanup_enabled: false,
            ..Settings::default()
        };
        settings
            .validate()
            .expect("loopback ASR has a valid keyless route");
    }

    #[test]
    fn onboarding_rejects_remote_keyless_cleanup_provider() {
        let mut settings = Settings {
            onboarded: true,
            cleanup_provider: crate::engine::EngineProvider::Ollama,
            ollama_base_url: "https://ollama.example/v1".into(),
            cleanup_model: "llama3.2".into(),
            ..Settings::default()
        };
        settings.api_key = "configured-groq-key".into();
        assert!(settings.validate().is_err());
    }

    #[test]
    fn load_settings_keeps_onboarding_for_selected_provider_sidecar() {
        let dir = temp_dir("selected-provider-onboarding");
        std::fs::create_dir_all(dir.join("secrets")).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": SETTINGS_SCHEMA_VERSION,
                "onboarded": true,
                "asr_provider": "openai",
                "asr_model": "whisper-1",
                "cleanup_enabled": false,
                "api_key": ""
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            secret_sidecar_path(&dir, "provider_openai"),
            "selected-provider-secret",
        )
        .unwrap();

        let (settings, _) = load_settings(&dir);
        assert!(settings.onboarded);
        assert_eq!(settings.asr_provider, crate::engine::EngineProvider::OpenAi);
        assert_eq!(settings.asr_credential(), "selected-provider-secret");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_key_clears_stale_onboarded_state_for_persistence() {
        let dir = temp_dir("stale-onboarding");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"onboarded":true,"api_key":""}"#,
        )
        .unwrap();
        let (settings, needs_persist) = load_settings(&dir);
        assert!(!settings.onboarded);
        assert!(needs_persist);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn spool_writes_are_atomic_and_path_bounded() {
        let dir = temp_dir("spool");
        let path = write_spool_file(&dir, Path::new("session/chunk.wav"), b"audio").unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"audio");
        assert!(write_spool_file(&dir, Path::new("../escape.wav"), b"nope").is_err());
        assert!(!dir.join("escape.wav").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("spool/session/chunk.wav"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
            let dir_mode = std::fs::metadata(dir.join("spool/session"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn live_audio_spool_lease_blocks_reuse_reads_and_defers_all_data_deletion() {
        let dir = temp_dir("audio-spool-lease");
        let (session, lease) = begin_audio_spool_session(&dir, "live-session").unwrap();
        assert!(
            !dir.join("spool").exists(),
            "audio reservation performs no filesystem initialization"
        );
        assert!(begin_audio_spool_session(&dir, "nested/session").is_err());
        let alias_root = dir.join("unused-child").join("..");
        assert!(begin_audio_spool_session(&alias_root, "live-session").is_err());
        let chunk = session.join("chunks/00000000.f32");

        assert!(begin_audio_spool_session(&dir, "live-session").is_err());
        assert!(read_spool_file(&chunk)
            .unwrap_err()
            .to_string()
            .contains("already owned by an audio worker"));
        assert!(recover_spool_session(&dir, "live-session", 7)
            .unwrap_err()
            .to_string()
            .contains("still owned"));
        assert!(
            write_spool_file(&dir, Path::new("live-session/chunks/late.f32"), b"late")
                .unwrap_err()
                .to_string()
                .contains("already owned by an audio worker")
        );

        assert!(clear_all_data(&dir)
            .unwrap_err()
            .to_string()
            .contains("cleanup is still pending"));
        assert!(
            !lease.deletion_requested(),
            "a rejected data clear must not schedule the live recording for deletion"
        );
        assert!(
            !dir.join("spool").exists(),
            "clear does not create or mutate a path while initialization is reserved"
        );
        assert!(begin_audio_spool_session(&dir, "live-session").is_err());

        let release_started = std::time::Instant::now();
        drop(lease);
        assert!(release_started.elapsed() < Duration::from_millis(100));
        assert!(
            !session.exists(),
            "a memory-only lease drop does not create or delete files"
        );
        clear_all_data(&dir).unwrap();
        let (reused, lease) = begin_audio_spool_session(&dir, "live-session").unwrap();
        assert!(!reused.exists());
        initialize_audio_spool_session(&lease).unwrap();
        drop(lease);
        assert!(reused.join("manifest.json").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_spool_session_drop_cannot_release_a_successor_generation() {
        let dir = temp_dir("audio-spool-lease-aba");
        let session_dir = dir.join("spool").join("aba-session");
        let stale =
            reserve_spool_session_path(&session_dir, true, SpoolLeaseKind::Session).unwrap();
        let stale_generation = stale.generation();

        // Model a predecessor whose owner slot was revoked while an old clone
        // is still retained. A new generation is allowed to claim the exact
        // same logical session path.
        {
            let mut active = ACTIVE_AUDIO_SPOOL_SESSIONS
                .get_or_init(Default::default)
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert_eq!(
                active
                    .sessions
                    .get(&session_dir)
                    .map(|value| value.generation),
                Some(stale_generation)
            );
            active.sessions.remove(&session_dir);
        }
        let successor =
            reserve_spool_session_path(&session_dir, true, SpoolLeaseKind::Session).unwrap();
        assert_ne!(stale.generation(), successor.generation());
        drop(stale);

        assert!(spool_lease_is_current(&successor));
        assert!(reserve_spool_session_path(&session_dir, true, SpoolLeaseKind::Session).is_err());
        drop(successor);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn root_mutation_and_clear_admissions_are_prefix_scoped_and_memory_only() {
        let dir = temp_dir("audio-spool-root-admission");
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("preserve.bin");
        std::fs::write(&marker, b"clear busy must preserve this file").unwrap();
        let before = std::fs::read(&marker).unwrap();
        let nested_root = dir.join("nested").join("profile");

        let mutation = begin_spool_root_mutation(&nested_root).unwrap();
        let alias_root = nested_root.join("unused-child").join("..");
        assert!(begin_clear_all_data(&alias_root).is_err());
        assert!(clear_all_data(&dir)
            .unwrap_err()
            .to_string()
            .contains("cleanup is still pending"));
        assert_eq!(std::fs::read(&marker).unwrap(), before);
        assert!(
            !nested_root.exists(),
            "admission must not create directories"
        );
        drop(mutation);

        let clear = begin_clear_all_data(&dir).unwrap();
        assert!(begin_spool_root_mutation(&nested_root).is_err());
        assert!(begin_audio_spool_session(&nested_root, "nested-session").is_err());
        assert_eq!(std::fs::read(&marker).unwrap(), before);
        drop(clear);

        let mutation_after_release = begin_spool_root_mutation(&nested_root).unwrap();
        drop(mutation_after_release);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recovery_readers_and_retention_reject_an_active_clear_before_side_effects() {
        let dir = temp_dir("audio-spool-clear-fences-recovery");
        std::fs::create_dir_all(dir.join("spool").join("legacy")).unwrap();
        let audio = dir.join("spool").join("legacy").join("recovery.wav");
        std::fs::write(&audio, b"preserve until clear completes").unwrap();
        let before = std::fs::read(&audio).unwrap();
        let recovery = RecoveredSpool {
            audio_path: audio.clone(),
            duration_secs: 1.0,
        };

        let clear = begin_clear_all_data(&dir).unwrap();
        assert!(read_spool_file(&audio).is_err());
        assert!(recover_spool(&dir, 7).is_err());
        assert!(recover_spool_session(&dir, "legacy", 7).is_err());
        assert!(record_recovered_history(&dir, &recovery).is_err());
        assert!(purge_history(&dir, 0).is_err());
        assert!(purge_gold_audio(&dir, 0).is_err());
        assert_eq!(std::fs::read(&audio).unwrap(), before);
        drop(clear);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_audio_spool_initialization_and_cancel_preserve_preexisting_artifacts() {
        use sha2::Digest;

        let dir = temp_dir("audio-spool-init-existing");
        let session = begin_spool_session(&dir, "preexisting-session").unwrap();
        let marker = session.join("marker.bin");
        std::fs::write(&marker, b"recovery artifact must remain untouched").unwrap();
        let before = sha2::Sha256::digest(std::fs::read(&marker).unwrap());

        let (reserved, lease) = begin_audio_spool_session(&dir, "preexisting-session").unwrap();
        assert_eq!(reserved, session);
        assert!(initialize_audio_spool_session(&lease).is_err());
        assert!(clear_all_data(&dir)
            .unwrap_err()
            .to_string()
            .contains("cleanup is still pending"));
        let release_started = std::time::Instant::now();
        drop(lease);
        assert!(release_started.elapsed() < Duration::from_millis(100));

        let after = sha2::Sha256::digest(std::fs::read(&marker).unwrap());
        assert_eq!(
            before, after,
            "failed initialization cannot erase recovery data"
        );
        assert!(session.join("manifest.json").exists());
        clear_all_data(&dir).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn spool_writes_plaintext_and_rejects_retired_encrypted_envelopes() {
        let dir = temp_dir("plaintext-spool");
        let wav = b"RIFF recovery";
        write_spool_file_internal(&dir, Path::new("session/recovery.wav"), wav, None).unwrap();
        let on_disk = dir.join("spool/session/recovery.wav");
        assert_eq!(std::fs::read(&on_disk).unwrap(), wav);
        assert_eq!(read_spool_file(&on_disk).unwrap(), wav);

        let legacy = dir.join("legacy.wav");
        std::fs::write(&legacy, wav).unwrap();
        assert_eq!(read_spool_file(&legacy).unwrap(), wav);

        let mut envelope = SPOOL_ENVELOPE_MAGIC.to_vec();
        envelope.extend_from_slice(b"ciphertext");
        let managed = dir.join("spool/session/encrypted.wav");
        std::fs::write(&managed, &envelope).unwrap();
        assert!(read_spool_file(&managed).is_err());
        assert!(read_spool_file_limited(&managed, 1024).is_err());
        std::fs::write(&legacy, &envelope).unwrap();
        assert!(read_spool_file(&legacy).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn spool_manifest_tracks_chunks_and_recovers_active_sessions() {
        let dir = temp_dir("manifest");
        let session = begin_spool_session(&dir, "session-test").unwrap();
        record_spool_chunk(&session, 0, 0.0, 15.0, "written").unwrap();
        std::fs::create_dir_all(session.join("chunks")).unwrap();
        std::fs::write(session.join("chunks/00000000.f32"), 0.25_f32.to_le_bytes()).unwrap();
        let manifest: SpoolManifest =
            serde_json::from_slice(&std::fs::read(session.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.status, "active");
        assert_eq!(manifest.chunks[0].status, "written");
        let recovered = recover_spool(&dir, 7).unwrap();
        assert_eq!(recovered.len(), 1);
        assert!(recovered[0].audio_path.is_file());
        assert!(!read_spool_file(&recovered[0].audio_path)
            .unwrap()
            .is_empty());
        let manifest: SpoolManifest =
            serde_json::from_slice(&std::fs::read(session.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.status, "recoverable");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn live_error_recovery_touches_only_the_matching_spool_session() {
        let dir = temp_dir("scoped-live-recovery");
        let matching = begin_spool_session(&dir, "current-session").unwrap();
        write_spool_f32_chunk(&dir, "current-session", 0, &[0.1, 0.2, 0.3]);
        record_spool_chunk(&matching, 0, 0.0, 0.0001875, "written").unwrap();
        let unrelated = begin_spool_session(&dir, "other-session").unwrap();
        write_spool_f32_chunk(&dir, "other-session", 0, &[0.4, 0.5]);
        record_spool_chunk(&unrelated, 0, 0.0, 0.000125, "written").unwrap();

        let recovered = recover_spool_session(&dir, "current-session", 7)
            .unwrap()
            .expect("matching live audio should remain recoverable");

        assert!(recovered.audio_path.is_file());
        assert_eq!(load_manifest(&matching).unwrap().status, "recoverable");
        assert_eq!(load_manifest(&unrelated).unwrap().status, "active");
        assert!(!unrelated.join("recovery.wav").exists());
        assert!(recover_spool_session(&dir, "../other-session", 7).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn dangling_flat_symlink_does_not_interrupt_startup_spool_recovery() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("dangling-flat-link-recovery");
        let session = begin_spool_session(&dir, "valid-session-after-link").unwrap();
        write_spool_f32_chunk(&dir, "valid-session-after-link", 0, &[0.1, 0.2, 0.3]);
        record_spool_chunk(&session, 0, 0.0, 0.0001875, "written").unwrap();
        symlink(
            dir.join("missing-target.wav"),
            dir.join("spool/dangling.wav"),
        )
        .unwrap();

        let recovered = recover_spool(&dir, 7).unwrap();

        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].audio_path, session.join("recovery.wav"));
        assert!(recovered[0].audio_path.is_file());
        assert!(dir.join("spool/dangling.wav").symlink_metadata().is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn zero_day_retention_never_recovers_a_same_second_live_spool() {
        let dir = temp_dir("zero-day-live-recovery");
        let session = begin_spool_session(&dir, "zero-day-session").unwrap();
        write_spool_f32_chunk(&dir, "zero-day-session", 0, &[0.1, 0.2, 0.3]);
        record_spool_chunk(&session, 0, 0.0, 0.0001875, "written").unwrap();

        assert!(recover_spool_session(&dir, "zero-day-session", 0)
            .unwrap()
            .is_none());
        assert!(!session.join("recovery.wav").exists());

        let _ = std::fs::remove_dir_all(dir);
    }

    fn set_spool_created_at_for_test(session: &Path, created_at_ms: u64) {
        let mut manifest = load_manifest(session).unwrap();
        manifest.created_at_ms = created_at_ms;
        save_manifest(session, &manifest).unwrap();
    }

    fn assert_spool_mtime_is_recent(session: &Path) {
        let elapsed = std::fs::metadata(session)
            .unwrap()
            .modified()
            .unwrap()
            .elapsed()
            .unwrap();
        assert!(
            elapsed.as_secs() < 60,
            "test must model a recent directory mtime"
        );
    }

    #[test]
    fn retention_tick_removes_expired_spool() {
        let dir = temp_dir("spool-daily-retention");
        let old = begin_spool_session(&dir, "expired-session").unwrap();
        let fresh = begin_spool_session(&dir, "fresh-session").unwrap();
        let completed = begin_spool_session(&dir, "leased-complete-session").unwrap();
        set_spool_created_at_for_test(&old, now_ms().saturating_sub(8 * 86_400_000));
        let mut completed_manifest = load_manifest(&completed).unwrap();
        completed_manifest.status = "complete".into();
        completed_manifest.created_at_ms = now_ms().saturating_sub(8 * 86_400_000);
        save_manifest(&completed, &completed_manifest).unwrap();
        let _completed_lease = lease_spool_session(&completed).unwrap();
        let flat_old = dir.join("spool/expired.wav");
        let flat_fresh = dir.join("spool/fresh.wav");
        std::fs::write(&flat_old, b"old").unwrap();
        std::fs::write(&flat_fresh, b"fresh").unwrap();
        let old_time = SystemTime::now() - Duration::from_secs(8 * 86_400);
        std::fs::File::options()
            .write(true)
            .open(&flat_old)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old_time))
            .unwrap();
        let injected_now = now_ms();
        set_spool_created_at_for_test(&fresh, injected_now.saturating_sub(3_000));
        let fresh_created_at = load_manifest(&fresh).unwrap().created_at_ms;

        expire_spool(&dir, 7, || injected_now).unwrap();

        assert!(!old.exists());
        assert!(fresh.exists());
        assert!(completed.exists());
        assert!(!flat_old.exists());
        assert!(flat_fresh.exists());
        assert_eq!(
            load_manifest(&fresh).unwrap().created_at_ms,
            fresh_created_at
        );
        drop(_completed_lease);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recovery_expiry_uses_session_creation_time_after_success_or_failure() {
        const KEEP_DAYS: u64 = 7;
        let dir = temp_dir("spool-origin-retention");

        let successful = begin_spool_session(&dir, "successful-old-session").unwrap();
        write_spool_f32_chunk(&dir, "successful-old-session", 0, &[0.1, 0.2, 0.3]);
        record_spool_chunk(&successful, 0, 0.0, 0.0001875, "written").unwrap();
        assert_eq!(recover_spool(&dir, KEEP_DAYS).unwrap().len(), 1);
        set_spool_created_at_for_test(
            &successful,
            now_ms().saturating_sub((KEEP_DAYS + 1) * 86_400_000),
        );
        assert_spool_mtime_is_recent(&successful);

        // A successful startup rewrites recovery.wav and manifest.json. Those
        // filesystem mtimes must not renew the immutable session retention age.
        assert!(recover_spool(&dir, KEEP_DAYS).unwrap().is_empty());
        assert!(
            !successful.exists(),
            "expired source and recovery files are purged"
        );

        let failed = begin_spool_session(&dir, "failed-old-session").unwrap();
        write_spool_f32_chunk(&dir, "failed-old-session", 0, &[0.1, 0.2]);
        write_spool_f32_chunk(&dir, "failed-old-session", 1, &[0.9, 0.3]);
        record_spool_chunk(&failed, 0, 0.0, 1.0, "written").unwrap();
        record_spool_chunk(&failed, 1, 0.5, 1.5, "written").unwrap();
        assert!(recover_spool(&dir, KEEP_DAYS).unwrap().is_empty());
        assert_eq!(load_manifest(&failed).unwrap().status, "abandoned");
        set_spool_created_at_for_test(
            &failed,
            now_ms().saturating_sub((KEEP_DAYS + 1) * 86_400_000),
        );
        assert_spool_mtime_is_recent(&failed);

        // Marking a failed rebuild abandoned changes the directory mtime too,
        // but it does not start a new retention period.
        assert!(recover_spool(&dir, KEEP_DAYS).unwrap().is_empty());
        assert!(
            !failed.exists(),
            "expired unreconstructable chunks are purged"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn zero_day_startup_retention_removes_fresh_sessions_and_legacy_files() {
        let dir = temp_dir("zero-day-spool-retention");
        let session = begin_spool_session(&dir, "fresh-zero-day-session").unwrap();
        write_spool_f32_chunk(&dir, "fresh-zero-day-session", 0, &[0.1, 0.2, 0.3]);
        record_spool_chunk(&session, 0, 0.0, 0.0001875, "written").unwrap();
        let flat = write_spool_file(&dir, Path::new("fresh-failed-job.wav"), b"legacy audio")
            .expect("flat legacy recovery artifact uses its exact file scope");

        assert!(recover_spool(&dir, 0).unwrap().is_empty());
        assert!(
            !session.exists(),
            "zero days removes even a same-second session"
        );
        assert!(
            !flat.exists(),
            "zero days removes fresh flat legacy artifacts too"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recent_recovery_keeps_immutable_creation_time_and_invalid_dates_expire() {
        const KEEP_DAYS: u64 = 7;
        let dir = temp_dir("spool-valid-retention-dates");

        let in_window = begin_spool_session(&dir, "in-window").unwrap();
        let created_at_ms = load_manifest(&in_window).unwrap().created_at_ms;
        write_spool_f32_chunk(&dir, "in-window", 0, &[0.1, 0.2]);
        record_spool_chunk(&in_window, 0, 0.0, 0.000125, "written").unwrap();
        set_spool_created_at_for_test(
            &in_window,
            now_ms().saturating_sub((KEEP_DAYS - 1) * 86_400_000),
        );
        let retained_created_at_ms = load_manifest(&in_window).unwrap().created_at_ms;
        assert_ne!(retained_created_at_ms, created_at_ms);
        assert_eq!(recover_spool(&dir, KEEP_DAYS).unwrap().len(), 1);
        assert_eq!(
            load_manifest(&in_window).unwrap().created_at_ms,
            retained_created_at_ms,
            "successful recovery must preserve its original creation timestamp"
        );
        assert!(in_window.exists());

        for (session_id, created_at_ms) in [
            ("zero-timestamp", 0),
            ("future-timestamp", now_ms().saturating_add(86_400_000)),
        ] {
            let invalid_dir = temp_dir(session_id);
            let invalid = begin_spool_session(&invalid_dir, session_id).unwrap();
            write_spool_f32_chunk(&invalid_dir, session_id, 0, &[0.1, 0.2]);
            record_spool_chunk(&invalid, 0, 0.0, 0.000125, "written").unwrap();
            set_spool_created_at_for_test(&invalid, created_at_ms);
            assert_spool_mtime_is_recent(&invalid);
            assert!(recover_spool(&invalid_dir, u64::MAX).unwrap().is_empty());
            assert!(
                !invalid.exists(),
                "untrustworthy creation timestamps expire fail-closed: {session_id}"
            );
            let _ = std::fs::remove_dir_all(invalid_dir);
        }

        // A schema-one manifest from a build predating the required immutable
        // creation timestamp cannot establish an age, so preserve the existing
        // cleanup behavior for timestamp-less legacy manifests.
        let legacy_dir = temp_dir("legacy-without-creation-time");
        let legacy = legacy_dir.join("spool/legacy-without-creation-time");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join("manifest.json"),
            r#"{"schema_version":1,"session_id":"legacy-without-creation-time","status":"active","chunks":[]}"#,
        )
        .unwrap();
        assert!(recover_spool(&legacy_dir, KEEP_DAYS).unwrap().is_empty());
        assert!(!legacy.exists());

        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(legacy_dir);
    }

    #[test]
    fn chunker_spool_recovery_reconstructs_exact_overlapping_sample_coverage() {
        const SAMPLE_RATE: usize = 16_000;
        const SOURCE_LEN: usize = 50 * SAMPLE_RATE;
        let dir = temp_dir("exact-chunk-recovery");
        let session_id = "exact-chunks";
        let session = begin_spool_session(&dir, session_id).unwrap();

        let mut source = (0..SOURCE_LEN)
            .map(|index| ((index % 997) as f32 / 997.0 - 0.5) * 0.008)
            .collect::<Vec<_>>();
        let config = crate::chunker::ChunkerConfig {
            chunk_length_secs: 15,
        };
        let mut probe = crate::chunker::Chunker::new(config);
        let mut planned = probe.push(&source);
        planned.extend(probe.finish());
        assert!(planned.len() >= 2);
        let boundary_sample = planned[1].source_start_sample + 100;
        source[boundary_sample] = 0.008;

        let mut chunker = crate::chunker::Chunker::new(config);
        let mut chunks = chunker.push(&source);
        chunks.extend(chunker.finish());
        assert!(chunks.len() >= 2);
        assert_eq!(
            chunks[1].source_start_sample,
            planned[1].source_start_sample
        );
        assert!(chunks[1].source_start_sample <= boundary_sample);
        assert!(boundary_sample < chunks[0].source_start_sample + chunks[0].samples.len());

        for chunk in &chunks {
            write_spool_f32_chunk(&dir, session_id, chunk.index, &chunk.samples);
            record_spool_chunk_with_samples(
                &session,
                chunk.index,
                chunk.start_secs,
                chunk.end_secs,
                "written",
                chunk.source_start_sample as u64,
                chunk.samples.len() as u64,
            )
            .unwrap();
        }
        // The older status API is also used as chunks progress; it must not
        // erase the exact interval required for recovery.
        record_spool_chunk(
            &session,
            chunks[0].index,
            chunks[0].start_secs,
            chunks[0].end_secs,
            "transcribed",
        )
        .unwrap();
        let manifest: SpoolManifest =
            serde_json::from_slice(&std::fs::read(session.join("manifest.json")).unwrap()).unwrap();
        let first = manifest
            .chunks
            .iter()
            .find(|chunk| chunk.index == chunks[0].index)
            .unwrap();
        assert_eq!(first.source_start_sample, Some(0));
        assert_eq!(first.sample_count, Some(chunks[0].samples.len() as u64));

        let recovered = rebuild_spool_recovery(&session).unwrap();
        assert_eq!(
            recovered.duration_secs,
            SOURCE_LEN as f64 / SAMPLE_RATE as f64
        );
        let expected = source
            .iter()
            .map(|sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect::<Vec<_>>();
        let recovered_samples = decode_wav_samples(&recovered.audio_path);
        assert_eq!(recovered_samples.len(), SOURCE_LEN);
        assert_eq!(recovered_samples, expected);
        let marker = expected[boundary_sample];
        assert_eq!(
            recovered_samples
                .iter()
                .filter(|sample| **sample == marker)
                .count(),
            1
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_nonoverlapping_spool_chunks_still_concatenate_in_index_order() {
        let dir = temp_dir("legacy-nonoverlap-recovery");
        let session = begin_spool_session(&dir, "legacy-chunks").unwrap();
        let first = [0.1_f32, 0.2];
        let second = [-0.3_f32, 0.4];
        write_spool_f32_chunk(&dir, "legacy-chunks", 0, &first);
        write_spool_f32_chunk(&dir, "legacy-chunks", 1, &second);
        record_spool_chunk(&session, 0, 0.0, 0.000125, "written").unwrap();
        record_spool_chunk(&session, 1, 0.000125, 0.00025, "written").unwrap();

        let recovered = recover_spool(&dir, 7).unwrap();
        assert_eq!(recovered.len(), 1);
        let expected = first
            .iter()
            .chain(&second)
            .map(|sample| (*sample * i16::MAX as f32) as i16)
            .collect::<Vec<_>>();
        assert_eq!(decode_wav_samples(&recovered[0].audio_path), expected);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn exact_spool_intervals_reject_missing_gaps_and_conflicting_samples() {
        let dir = temp_dir("invalid-exact-recovery");

        let missing = begin_spool_session(&dir, "missing").unwrap();
        write_spool_f32_chunk(&dir, "missing", 0, &[0.1, 0.2]);
        write_spool_f32_chunk(&dir, "missing", 1, &[0.3, 0.4]);
        record_spool_chunk_with_samples(&missing, 0, 0.0, 0.000125, "written", 0, 2).unwrap();
        record_spool_chunk(&missing, 1, 0.000125, 0.00025, "written").unwrap();
        let error = rebuild_spool_recovery(&missing).unwrap_err().to_string();
        assert!(error.contains("intervals are missing"), "{error}");

        let unmatched = begin_spool_session(&dir, "unmatched-exact").unwrap();
        record_spool_chunk_with_samples(&unmatched, 0, 0.0, 0.000125, "written", 0, 2).unwrap();
        write_spool_f32_chunk(&dir, "unmatched-exact", 1, &[0.3, 0.4]);
        let error = rebuild_spool_recovery(&unmatched).unwrap_err().to_string();
        assert!(error.contains("intervals are missing"), "{error}");

        let gap = begin_spool_session(&dir, "gap").unwrap();
        write_spool_f32_chunk(&dir, "gap", 0, &[0.1, 0.2]);
        write_spool_f32_chunk(&dir, "gap", 1, &[0.3, 0.4]);
        record_spool_chunk_with_samples(&gap, 0, 0.0, 0.000125, "written", 0, 2).unwrap();
        record_spool_chunk_with_samples(&gap, 1, 0.0001875, 0.0003125, "written", 3, 2).unwrap();
        let error = rebuild_spool_recovery(&gap).unwrap_err().to_string();
        assert!(error.contains("gap in source samples"), "{error}");

        let conflict = begin_spool_session(&dir, "conflict").unwrap();
        write_spool_f32_chunk(&dir, "conflict", 0, &[0.1, 0.2, 0.3]);
        write_spool_f32_chunk(&dir, "conflict", 1, &[0.9, 0.4]);
        record_spool_chunk_with_samples(&conflict, 0, 0.0, 0.0001875, "written", 0, 3).unwrap();
        record_spool_chunk_with_samples(&conflict, 1, 0.000125, 0.00025, "written", 2, 2).unwrap();
        let error = rebuild_spool_recovery(&conflict).unwrap_err().to_string();
        assert!(error.contains("conflicting samples"), "{error}");

        assert!(record_spool_chunk_with_samples(&conflict, 2, 0.0, 0.0, "written", 0, 0).is_err());
        assert!(
            record_spool_chunk_with_samples(&conflict, 2, 0.0, 0.0, "written", u64::MAX, 1,)
                .is_err()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_overlapping_chunks_without_sample_intervals_fail_truthfully() {
        let dir = temp_dir("legacy-overlap-recovery");
        let session = begin_spool_session(&dir, "legacy-overlap").unwrap();
        write_spool_f32_chunk(&dir, "legacy-overlap", 0, &[0.1, 0.2]);
        write_spool_f32_chunk(&dir, "legacy-overlap", 1, &[0.2, 0.3]);
        record_spool_chunk(&session, 0, 0.0, 1.0, "written").unwrap();
        record_spool_chunk(&session, 1, 0.5, 1.5, "written").unwrap();

        let error = rebuild_spool_recovery(&session).unwrap_err().to_string();
        assert!(error.contains("overlap but lack exact source sample intervals"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unreconstructable_recovery_keeps_original_chunks_until_retention() {
        let dir = temp_dir("unreconstructable-recovery-retained");

        let legacy = begin_spool_session(&dir, "legacy-overlap-retained").unwrap();
        let legacy_first = [0.1_f32, 0.2];
        let legacy_second = [0.2_f32, 0.3];
        write_spool_f32_chunk(&dir, "legacy-overlap-retained", 0, &legacy_first);
        write_spool_f32_chunk(&dir, "legacy-overlap-retained", 1, &legacy_second);
        record_spool_chunk(&legacy, 0, 0.0, 1.0, "written").unwrap();
        record_spool_chunk(&legacy, 1, 0.5, 1.5, "written").unwrap();
        let legacy_first_bytes = read_spool_file(&legacy.join("chunks/00000000.f32")).unwrap();
        let legacy_second_bytes = read_spool_file(&legacy.join("chunks/00000001.f32")).unwrap();
        let legacy_manifest_before: SpoolManifest =
            serde_json::from_slice(&std::fs::read(legacy.join("manifest.json")).unwrap()).unwrap();

        let conflict = begin_spool_session(&dir, "conflict-retained").unwrap();
        let conflict_first = [0.1_f32, 0.2, 0.3];
        let conflict_second = [0.9_f32, 0.4];
        write_spool_f32_chunk(&dir, "conflict-retained", 0, &conflict_first);
        write_spool_f32_chunk(&dir, "conflict-retained", 1, &conflict_second);
        record_spool_chunk_with_samples(&conflict, 0, 0.0, 0.0001875, "written", 0, 3).unwrap();
        record_spool_chunk_with_samples(&conflict, 1, 0.000125, 0.00025, "written", 2, 2).unwrap();
        let conflict_first_bytes = read_spool_file(&conflict.join("chunks/00000000.f32")).unwrap();
        let conflict_second_bytes = read_spool_file(&conflict.join("chunks/00000001.f32")).unwrap();
        let conflict_manifest_before: SpoolManifest =
            serde_json::from_slice(&std::fs::read(conflict.join("manifest.json")).unwrap())
                .unwrap();

        let recovered = recover_spool(&dir, 7).unwrap();
        assert!(
            recovered.is_empty(),
            "unreconstructable audio must not be reported recovered"
        );

        for (session, expected, second_expected, manifest_before) in [
            (
                legacy,
                legacy_first_bytes,
                legacy_second_bytes,
                legacy_manifest_before,
            ),
            (
                conflict,
                conflict_first_bytes,
                conflict_second_bytes,
                conflict_manifest_before,
            ),
        ] {
            assert!(session.is_dir());
            assert!(!session.join("recovery.wav").exists());
            assert_eq!(
                read_spool_file(&session.join("chunks/00000000.f32")).unwrap(),
                expected
            );
            assert_eq!(
                read_spool_file(&session.join("chunks/00000001.f32")).unwrap(),
                second_expected
            );
            let manifest_after: SpoolManifest =
                serde_json::from_slice(&std::fs::read(session.join("manifest.json")).unwrap())
                    .unwrap();
            assert_eq!(manifest_after.status, "abandoned");
            assert_eq!(manifest_after.chunks.len(), manifest_before.chunks.len());
            for (before, after) in manifest_before.chunks.iter().zip(&manifest_after.chunks) {
                assert_eq!(before.index, after.index);
                assert_eq!(before.start_secs, after.start_secs);
                assert_eq!(before.end_secs, after.end_secs);
                assert_eq!(before.source_start_sample, after.source_start_sample);
                assert_eq!(before.sample_count, after.sample_count);
                assert_eq!(before.status, after.status);
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_history_with_missing_recovery_wav_completes_manifest_before_commit() {
        let dir = temp_dir("delete-history-missing-session-wav");
        let session = begin_spool_session(&dir, "session-missing-wav").unwrap();
        std::fs::create_dir_all(session.join("chunks")).unwrap();
        std::fs::write(session.join("chunks/00000000.f32"), 0.25_f32.to_le_bytes()).unwrap();
        let recovery = recover_spool(&dir, 7).unwrap().remove(0);
        assert_eq!(load_manifest(&session).unwrap().status, "recoverable");
        record_recovered_history(&dir, &recovery).unwrap();
        std::fs::remove_file(&recovery.audio_path).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;

        let lease = wait_for_history_delete_spool_lease(&dir, &recovery.audio_path).unwrap();
        let connection = open_history(&dir).unwrap();
        let transaction = connection
            .unchecked_transaction()
            .expect("start History delete transaction");
        mark_spool_artifact_complete(&dir, &recovery.audio_path, lease.inner.kind).unwrap();
        assert_eq!(load_manifest(&session).unwrap().status, "complete");
        transaction
            .execute("DELETE FROM dictations WHERE id=?", [id])
            .unwrap();
        transaction.commit().unwrap();
        drop(lease);

        assert!(get_history(&dir, 10).unwrap().is_empty());
        assert!(session.exists());
        assert!(recover_spool(&dir, 7).unwrap().is_empty());
        assert!(session.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cached_recovery_cannot_resurrect_history_after_delete() {
        let dir = temp_dir("cached-recovery-after-delete");
        let session = begin_spool_session(&dir, "cached-delete-session").unwrap();
        std::fs::create_dir_all(session.join("chunks")).unwrap();
        std::fs::write(session.join("chunks/00000000.f32"), 0.25_f32.to_le_bytes()).unwrap();
        let recovery = recover_spool(&dir, 7).unwrap().remove(0);
        record_recovered_history(&dir, &recovery).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;

        delete_history(&dir, id).unwrap();
        assert!(get_history(&dir, 10).unwrap().is_empty());
        assert!(!session.exists());
        assert!(record_recovered_history(&dir, &recovery).is_err());
        assert!(get_history(&dir, 10).unwrap().is_empty());
        assert!(!session.exists());
        assert!(!recovery.audio_path.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn completed_session_with_retained_recovery_wav_cannot_be_promoted() {
        let dir = temp_dir("completed-recovery-not-promoted");
        let session = begin_spool_session(&dir, "completed-recovery-session").unwrap();
        std::fs::create_dir_all(session.join("chunks")).unwrap();
        std::fs::write(session.join("chunks/00000000.f32"), 0.25_f32.to_le_bytes()).unwrap();
        let recovery = recover_spool(&dir, 7).unwrap().remove(0);
        let mut manifest = load_manifest(&session).unwrap();
        manifest.status = "complete".into();
        save_manifest(&session, &manifest).unwrap();

        assert!(recovery.audio_path.exists());
        assert!(record_recovered_history(&dir, &recovery).is_err());
        assert!(get_history(&dir, 10).unwrap().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn flat_managed_recovery_promotes_idempotently_and_cannot_resurrect_after_delete() {
        let dir = temp_dir("flat-recovery-promotion");
        let audio_path =
            write_spool_file(&dir, Path::new("legacy-recovery.wav"), b"legacy audio").unwrap();
        let recovery = RecoveredSpool {
            audio_path: audio_path.clone(),
            duration_secs: 1.0,
        };
        record_recovered_history(&dir, &recovery).unwrap();
        record_recovered_history(&dir, &recovery).unwrap();
        let rows = get_history(&dir, 10).unwrap();
        assert_eq!(rows.len(), 1);

        delete_history(&dir, rows[0].id).unwrap();
        assert!(get_history(&dir, 10).unwrap().is_empty());
        assert!(!audio_path.exists());
        assert!(record_recovered_history(&dir, &recovery).is_err());
        assert!(get_history(&dir, 10).unwrap().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recovered_history_is_idempotent_and_deletes_session_artifacts() {
        let dir = temp_dir("recovered-history");
        let session = begin_spool_session(&dir, "session-recovered").unwrap();
        std::fs::create_dir_all(session.join("chunks")).unwrap();
        std::fs::write(session.join("chunks/00000000.f32"), 0.25_f32.to_le_bytes()).unwrap();
        let recovery = recover_spool(&dir, 7).unwrap().remove(0);
        record_recovered_history(&dir, &recovery).unwrap();
        record_recovered_history(&dir, &recovery).unwrap();
        assert_eq!(get_history(&dir, 10).unwrap().len(), 1);
        let id = get_history(&dir, 10).unwrap()[0].id;
        delete_history(&dir, id).unwrap();
        assert!(!session.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_recovered_history_insert_is_idempotent_for_one_audio_path() {
        let dir = temp_dir("concurrent-recovered-history");
        let session = begin_spool_session(&dir, "session-concurrent-recovered").unwrap();
        std::fs::create_dir_all(session.join("chunks")).unwrap();
        std::fs::write(session.join("chunks/00000000.f32"), 0.25_f32.to_le_bytes()).unwrap();
        let recovery = recover_spool(&dir, 7).unwrap().remove(0);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let dir = dir.clone();
                let recovery = recovery.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    record_recovered_history(&dir, &recovery)
                })
            })
            .collect();
        barrier.wait();
        for handle in handles {
            handle.join().unwrap().unwrap();
        }

        assert_eq!(get_history(&dir, 10).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn degraded_history_exposes_recovery_audio_for_retry() {
        let dir = temp_dir("degraded-retry");
        let session = begin_spool_session(&dir, "session-degraded-retry").unwrap();
        std::fs::create_dir_all(session.join("chunks")).unwrap();
        std::fs::write(session.join("chunks/00000000.f32"), 0.25_f32.to_le_bytes()).unwrap();
        mark_spool_status(&session, "degraded").unwrap();
        let recovery = rebuild_spool_recovery(&session).unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "partial",
            recovery.duration_secs,
            true,
            Some("partial_asr_failure"),
            "degraded",
            "history",
            None,
            &context,
            Some(&recovery.audio_path),
        )
        .unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        assert_eq!(
            failed_spool(&dir, id).unwrap(),
            Some(recovery.audio_path.to_string_lossy().into())
        );
        delete_history(&dir, id).unwrap();
        assert!(!session.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_history_removes_a_degraded_spool_directory() {
        let dir = temp_dir("delete-spool-dir");
        let session = begin_spool_session(&dir, "session-degraded").unwrap();
        std::fs::write(session.join("chunk_0.wav"), b"audio").unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "final",
            1.0,
            true,
            Some("partial_asr_failure"),
            "degraded",
            "history",
            None,
            &context,
            Some(&session),
        )
        .unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        delete_history(&dir, id).unwrap();
        assert!(!session.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_can_be_deleted() {
        let dir = temp_dir("delete");
        insert_history(&dir, "raw", "final", 1.0, false).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        delete_history(&dir, id).unwrap();
        assert!(get_history(&dir, 10).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_history_waits_for_spool_owner_and_leaves_record_on_timeout() {
        let dir = temp_dir("delete-history-busy-spool");
        let session = begin_spool_session(&dir, "delete-busy").unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "final",
            1.0,
            true,
            Some("partial_asr_failure"),
            "degraded",
            "history",
            None,
            &context,
            Some(&session),
        )
        .unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        let owner = lease_spool_session(&session).unwrap();

        let error = delete_history(&dir, id).unwrap_err();

        assert!(error.to_string().contains("timed out"));
        assert_eq!(get_history(&dir, 10).unwrap().len(), 1);
        assert_eq!(load_manifest(&session).unwrap().status, "active");
        drop(owner);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deleting_history_after_spool_release_does_not_resurrect_on_recovery() {
        let dir = temp_dir("delete-history-spool-release");
        let session = begin_spool_session(&dir, "delete-release").unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "final",
            1.0,
            true,
            Some("partial_asr_failure"),
            "degraded",
            "history",
            None,
            &context,
            Some(&session),
        )
        .unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        let owner = lease_spool_session(&session).unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        pause_history_delete_spool_wait_for_test(&session, entered_tx, release_rx);

        let delete_root = dir.clone();
        let deleting = std::thread::spawn(move || delete_history(&delete_root, id));
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(owner);
        release_tx.send(()).unwrap();
        deleting.join().unwrap().unwrap();

        assert!(get_history(&dir, 10).unwrap().is_empty());
        assert!(recover_spool(&dir, 7).unwrap().is_empty());
        assert!(!session.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clear_all_data_scrubs_text_and_keeps_database_usable() {
        let dir = temp_dir("clear-history-scrub-text");
        let secret = "unique_clear_history_secret_7f0d8a2c";
        insert_history(&dir, secret, secret, 1.0, false).unwrap();
        let database = dir.join("history.sqlite");

        clear_all_data(&dir).unwrap();

        assert!(!fs::read(database)
            .unwrap()
            .windows(secret.len())
            .any(|w| w == secret.as_bytes()));
        assert!(get_history(&dir, 10).unwrap().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn history_copy_falls_back_to_raw_text_when_final_text_is_empty() {
        let dir = temp_dir("history-copy-fallback");
        insert_history(&dir, "raw transcript", "", 1.0, true).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        assert_eq!(history_text(&dir, id).unwrap(), "raw transcript");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_keeps_raw_final_and_cleanup_status_separately() {
        let dir = temp_dir("history-cleanup-texts");
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool_and_cleanup(
            &dir,
            "spoken before cleanup",
            "cleaned after cleanup",
            1.0,
            false,
            None,
            "ok",
            "paste",
            None,
            &context,
            None,
            "ai_success",
        )
        .unwrap();

        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.raw_text, "spoken before cleanup");
        assert_eq!(item.final_text, "cleaned after cleanup");
        assert_eq!(item.cleanup_status, "ai_success");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_exports_original_asr_separately_and_preserves_null_for_legacy_rows() {
        let dir = temp_dir("history-asr-text");
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_asr_and_delivery_and_spool_and_cleanup(
            &dir,
            "prepared and dictionary-replaced",
            Some("uh, raw provider output"),
            "cleaned final",
            1.0,
            false,
            None,
            "ok",
            "history",
            None,
            &context,
            None,
            "openai:whisper-1",
            "ai_success",
        )
        .unwrap();

        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.raw_text, "prepared and dictionary-replaced");
        assert_eq!(item.asr_text.as_deref(), Some("uh, raw provider output"));
        assert_eq!(item.engine.as_deref(), Some("openai:whisper-1"));
        assert_eq!(
            get_history_page(&dir, 10, None, Some("raw provider"))
                .unwrap()
                .items
                .len(),
            1
        );
        let export: serde_json::Value =
            serde_json::from_str(&export_history_json(&dir).unwrap()).unwrap();
        assert_eq!(export[0]["asr_text"], "uh, raw provider output");
        assert_eq!(export[0]["engine"], "openai:whisper-1");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_retryable_only_when_recovery_audio_exists() {
        let dir = temp_dir("history-retryable");
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "partial",
            1.0,
            true,
            Some("llm_cleanup_failed"),
            "degraded",
            "paste",
            None,
            &context,
            None,
        )
        .unwrap();
        assert!(!get_history(&dir, 10).unwrap()[0].retryable);

        let path = write_spool_file(&dir, Path::new("failed-short.wav"), b"audio").unwrap();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "partial",
            1.0,
            true,
            Some("llm_cleanup_failed"),
            "degraded",
            "paste",
            None,
            &context,
            Some(&path),
        )
        .unwrap();
        assert!(get_history(&dir, 1).unwrap()[0].retryable);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cleanup_rejection_candidate_is_separate_exportable_and_generation_scoped() {
        let dir = temp_dir("cleanup-rejection-history");
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup_rejection(
            &dir,
            "prepared transcript",
            Some("provider ASR text"),
            Some("ASR-side cleanup candidate"),
            "safe final text",
            1.0,
            true,
            Some("preservation_guard"),
            "degraded",
            "history",
            Some("paste_unverified"),
            &context,
            None,
            "test-asr",
            "preservation_guard",
            None,
            Some("complete cleanup candidate"),
            Some("provider_protected_token_changed"),
        )
        .unwrap();
        let id = get_history(&dir, 1).unwrap()[0].id;
        let old_transforms = [
            "strip_internal_cleanup_metadata".to_owned(),
            "restore_spoken_layout".to_owned(),
        ];
        update_history_revision_state_with_reclean_delivery(
            &dir,
            id,
            true,
            Some("preservation_guard"),
            Some("preservation_guard"),
            Some("prior rejected candidate"),
            Some("provider_protected_token_changed"),
            None,
            Some("network"),
            Some(&old_transforms),
            Some("prior_final_reason"),
            Some("network_failure"),
        )
        .unwrap();
        let item = get_history(&dir, 1).unwrap().remove(0);
        assert_eq!(
            item.provider_cleaned_candidate.as_deref(),
            Some("ASR-side cleanup candidate")
        );
        assert_eq!(
            item.cleanup_rejected_candidate.as_deref(),
            Some("prior rejected candidate")
        );
        assert_eq!(
            item.cleanup_guard_reason.as_deref(),
            Some("provider_protected_token_changed")
        );
        assert_eq!(item.cleanup_attempt_kind.as_deref(), Some("network"));
        assert_eq!(
            item.cleanup_transformations.as_deref(),
            Some(old_transforms.as_slice())
        );
        assert_eq!(
            item.cleanup_final_guard_reason.as_deref(),
            Some("prior_final_reason")
        );
        assert_eq!(
            item.cleanup_failure_reason.as_deref(),
            Some("network_failure")
        );
        assert_eq!(
            get_history_page(&dir, 10, None, Some("prior rejected candidate"))
                .unwrap()
                .items[0]
                .id,
            id
        );

        let export: serde_json::Value =
            serde_json::from_str(&export_history_json(&dir).unwrap()).unwrap();
        assert_eq!(
            export[0]["provider_cleaned_candidate"],
            "ASR-side cleanup candidate"
        );
        assert_eq!(
            export[0]["cleanup_rejected_candidate"],
            "prior rejected candidate"
        );
        assert_eq!(
            export[0]["cleanup_guard_reason"],
            "provider_protected_token_changed"
        );

        mark_retried_with_texts_and_cleanup_rejection(
            &dir,
            id,
            Some("new prepared transcript"),
            Some("new ASR text"),
            Some("on_device:qwen3-asr-0.6b"),
            "new final text",
            true,
            Some("preservation_guard"),
            Some("preservation_guard"),
            RetryDelivery::ClipboardChanged,
            Some("new cleanup candidate"),
            Some("final_source_script_changed"),
            Some("final_guard"),
            Some(&[]),
            Some("final_explicit_layout_changed"),
            Some("final_guard_explicit_layout_changed"),
        )
        .unwrap();
        let item = get_history(&dir, 1).unwrap().remove(0);
        assert_eq!(item.provider_cleaned_candidate, None);
        assert_eq!(item.asr_text.as_deref(), Some("new ASR text"));
        assert_eq!(
            item.cleanup_rejected_candidate.as_deref(),
            Some("new cleanup candidate")
        );
        assert_eq!(
            item.cleanup_guard_reason.as_deref(),
            Some("final_source_script_changed")
        );
        assert_eq!(item.cleanup_attempt_kind.as_deref(), Some("final_guard"));
        assert_eq!(item.cleanup_transformations.as_deref(), Some([].as_slice()));
        assert_eq!(
            item.cleanup_final_guard_reason.as_deref(),
            Some("final_explicit_layout_changed")
        );
        assert_eq!(
            item.cleanup_failure_reason.as_deref(),
            Some("final_guard_explicit_layout_changed")
        );
        assert_eq!(item.raw_text, "new prepared transcript");
        assert_eq!(item.final_text, "new final text");

        let manual = save_manual_history_revision(&dir, id, "manually edited final").unwrap();
        assert!(manual.changed);
        assert_eq!(manual.learning_before_text, "new final text");
        let item = get_history(&dir, 1).unwrap().remove(0);
        assert_eq!(item.cleanup_rejected_candidate, None);
        assert_eq!(item.cleanup_guard_reason, None);
        assert_eq!(item.cleanup_attempt_kind, None);
        assert_eq!(item.cleanup_transformations, None);
        assert_eq!(item.cleanup_final_guard_reason, None);
        assert_eq!(item.cleanup_failure_reason, None);
        assert_eq!(item.raw_text, "new prepared transcript");
        assert_eq!(item.asr_text.as_deref(), Some("new ASR text"));
        assert_eq!(item.final_text, "manually edited final");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learn_pair_scope_keeps_unknown_distinct_from_a_foreign_scope() {
        let row = LearnPairRecord {
            pair_key: "pair".into(),
            before_surface: "LOM".into(),
            after_surface: "LLM".into(),
            hits: 3,
            promoted: true,
            last_at: "2026-01-01".into(),
            family: Some("work_chat".into()),
            mapping_id: Some("codex".into()),
            browser_host: None,
            native_bundle: Some("com.example.codex".into()),
            last_used_at: None,
            pinned: false,
            tombstoned_at: None,
            ignored: false,
            promote_hits: 0,
            manual_scoped: false,
            auto_scoped: false,
        };
        let matching = LearnPairScope {
            family: Some("work_chat".into()),
            mapping_id: Some("codex".into()),
            browser_host: None,
            native_bundle: Some("com.example.codex".into()),
        };
        assert_eq!(matching.assess_record(&row), LearnPairScopeMatch::Match);
        assert_eq!(
            LearnPairScope {
                native_bundle: Some("com.example.other".into()),
                ..matching.clone()
            }
            .assess_record(&row),
            LearnPairScopeMatch::Mismatch
        );
        assert_eq!(
            LearnPairScope {
                mapping_id: None,
                ..matching.clone()
            }
            .assess_record(&row),
            LearnPairScopeMatch::Unknown
        );
        let selected = LearnPairScope {
            mapping_id: None,
            native_bundle: None,
            ..matching
        }
        .applicable_promoted_records(std::slice::from_ref(&row));
        assert!(selected.is_empty());
    }

    #[test]
    fn existing_family_only_promoted_rows_no_longer_apply_globally() {
        let dir = temp_dir("family-only-learn-pair-migration");
        let connection = open_history(&dir).unwrap();
        let family_key = crate::dictionary_learn::pair_key("知呼", "知乎");
        let mapping_key = crate::dictionary_learn::pair_key("派森", "Python");
        let global_key = crate::dictionary_learn::pair_key("大模型", "LLM");
        let empty_family_key = crate::dictionary_learn::pair_key("空族", "EmptyFamily");
        connection
            .execute(
                "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, family)
                 VALUES (?1, '知呼', '知乎', 3, 1, datetime('now'), 'work_chat')",
                [&family_key],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at)
                 VALUES (?1, '大模型', 'LLM', 3, 1, datetime('now'))",
                [&global_key],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, family)
                 VALUES (?1, '空族', 'EmptyFamily', 4, 1, datetime('now'), '')",
                [&empty_family_key],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, family, mapping_id, last_used_at, pinned)
                 VALUES (?1, '派森', 'Python', 3, 1, datetime('now'), 'work_chat', 'codex', '2026-11-03', 1)",
                [&mapping_key],
            )
            .unwrap();
        drop(connection);

        let reopened = open_history(&dir).unwrap();
        let version: i32 = reopened
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, HISTORY_SCHEMA_VERSION);
        let family_only = get_learn_pair(&dir, &family_key).unwrap().unwrap();
        let mapping_scoped = get_learn_pair(&dir, &mapping_key).unwrap().unwrap();
        let global = get_learn_pair(&dir, &global_key).unwrap().unwrap();
        let empty_family = get_learn_pair(&dir, &empty_family_key).unwrap().unwrap();
        assert!(!family_only.is_live_promoted());
        assert_eq!(family_only.before_surface, "知呼");
        assert_eq!(family_only.after_surface, "知乎");
        assert_eq!(family_only.hits, 3);
        assert!(mapping_scoped.is_live_promoted());
        assert_eq!(mapping_scoped.before_surface, "派森");
        assert_eq!(mapping_scoped.after_surface, "Python");
        assert_eq!(mapping_scoped.hits, 3);
        assert_eq!(mapping_scoped.mapping_id.as_deref(), Some("codex"));
        assert_eq!(mapping_scoped.last_used_at.as_deref(), Some("2026-11-03"));
        assert!(mapping_scoped.pinned);
        assert!(global.is_live_promoted());
        assert!(empty_family.is_live_promoted());
        assert_eq!(empty_family.family.as_deref(), Some(""));
        assert_eq!(empty_family.before_surface, "空族");
        assert_eq!(empty_family.after_surface, "EmptyFamily");
        assert_eq!(empty_family.hits, 4);

        let same_family_other_app = LearnPairScope {
            family: Some("work_chat".into()),
            mapping_id: Some("slack".into()),
            ..LearnPairScope::default()
        };
        assert!(same_family_other_app
            .applicable_promoted_records(&[family_only.clone(), mapping_scoped.clone()])
            .is_empty());
        assert_eq!(
            LearnPairScope {
                family: Some("work_chat".into()),
                mapping_id: Some("codex".into()),
                ..LearnPairScope::default()
            }
            .applicable_promoted_records(&[mapping_scoped])
            .len(),
            1
        );
        drop(reopened);

        let reopened_again = open_history(&dir).unwrap();
        assert!(!get_learn_pair(&dir, &family_key)
            .unwrap()
            .unwrap()
            .is_live_promoted());
        assert!(get_learn_pair(&dir, &empty_family_key)
            .unwrap()
            .unwrap()
            .is_live_promoted());
        drop(reopened_again);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reclean_revision_and_attribution_roll_back_together_on_sql_failure() {
        let dir = temp_dir("reclean-atomic-adoption");
        insert_history(&dir, "raw source", "old adopted text", 1.0, false).unwrap();
        let c = open_history(&dir).unwrap();
        c.execute("UPDATE dictations SET cleanup_attempt_kind='network',cleanup_failure_reason='network_failure' WHERE id=1", []).unwrap();
        c.execute_batch("CREATE TRIGGER reject_reclean_facts BEFORE UPDATE OF cleanup_attempt_kind ON dictations BEGIN SELECT RAISE(ABORT, 'injected attribution failure'); END;").unwrap();
        drop(c);
        let intent = crate::llm::CleanupIntent::selected_text(
            crate::llm::CleanupOperation::Cleanup,
            "raw source",
        );
        let result = save_reclean_revision_with_facts(
            &dir,
            RecleanHistorySave {
                dictation_id: 1,
                final_text: "new adopted text",
                cleanup_status: "ai_success",
                intent: &intent,
                model: "model",
                context_policy: &crate::context::ContextPolicy::default(),
                cleanup_rejected_candidate: None,
                cleanup_guard_reason: None,
                cleanup_attempt_kind: Some("success"),
                cleanup_transformations: Some(&[]),
                cleanup_final_guard_reason: None,
                cleanup_failure_reason: None,
            },
        );
        assert!(result.is_err());
        let item = get_history(&dir, 1).unwrap().remove(0);
        assert_eq!(item.final_text, "old adopted text");
        assert_eq!(item.cleanup_attempt_kind.as_deref(), Some("network"));
        assert_eq!(
            item.cleanup_failure_reason.as_deref(),
            Some("network_failure")
        );
        assert_eq!(item.revision_count, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retry_updates_history_text() {
        let dir = temp_dir("retry");
        insert_failed_history(&dir, "raw", 1.0, None).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        mark_retried_with_texts(
            &dir,
            id,
            Some("retried raw"),
            Some("retry ASR before local filters"),
            Some("deepgram:nova-3"),
            "final",
            false,
            None,
            Some("ai_success"),
            RetryDelivery::Copied,
        )
        .unwrap();
        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.status, "copied");
        assert_eq!(item.raw_text, "retried raw");
        assert_eq!(
            item.asr_text.as_deref(),
            Some("retry ASR before local filters")
        );
        assert_eq!(item.final_text, "final");
        assert_eq!(item.cleanup_status, "ai_success");
        assert_eq!(item.engine.as_deref(), Some("deepgram:nova-3"));
        assert_eq!(item.delivery_method.as_deref(), Some("clipboard"));
        assert_eq!(
            item.fallback_reason.as_deref(),
            Some("retry_clipboard_only")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retry_clipboard_abstention_saves_transcript_as_history() {
        let dir = temp_dir("retry-clipboard-changed");
        insert_failed_history(&dir, "raw", 1.0, None).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        mark_retried_with_texts(
            &dir,
            id,
            Some("retried raw"),
            Some("retry ASR"),
            Some("on_device:qwen3-asr-0.6b"),
            "final",
            false,
            None,
            Some("local_only"),
            RetryDelivery::ClipboardChanged,
        )
        .unwrap();

        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.status, "history");
        assert_eq!(item.final_text, "final");
        assert_eq!(item.delivery_method.as_deref(), Some("history"));
        assert_eq!(
            item.fallback_reason.as_deref(),
            Some("retry_clipboard_changed")
        );
        assert!(failed_spool(&dir, id).unwrap().is_none());
        assert_eq!(history_text(&dir, id).unwrap(), "final");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retry_clipboard_ownership_unknown_and_write_failure_keep_history_status() {
        for delivery in [
            RetryDelivery::ClipboardOwnershipUnverified,
            RetryDelivery::ClipboardWriteFailed,
        ] {
            let dir = temp_dir("retry-clipboard-unverified");
            insert_failed_history(&dir, "raw", 1.0, None).unwrap();
            let id = get_history(&dir, 10).unwrap()[0].id;
            mark_retried_with_texts(
                &dir,
                id,
                Some("retried raw"),
                Some("retry ASR"),
                Some("on_device:qwen3-asr-0.6b"),
                "final",
                false,
                None,
                Some("local_only"),
                delivery,
            )
            .unwrap();

            let item = get_history(&dir, 10).unwrap().remove(0);
            assert_eq!(item.status, "history");
            assert_eq!(item.delivery_method.as_deref(), Some("history"));
            assert_eq!(
                item.fallback_reason.as_deref(),
                Some(match delivery {
                    RetryDelivery::ClipboardOwnershipUnverified => {
                        "retry_clipboard_ownership_unverified"
                    }
                    RetryDelivery::ClipboardWriteFailed => "retry_clipboard_write_failed",
                    _ => unreachable!(),
                })
            );
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn reclean_delivery_records_only_a_current_owned_copy_as_copied() {
        let cases = [
            (
                Some(RecleanDelivery::Copied),
                "copied",
                "clipboard",
                Some("reclean_clipboard_only"),
            ),
            (
                Some(RecleanDelivery::ClipboardChanged),
                "history",
                "history",
                Some("reclean_clipboard_changed"),
            ),
            (
                Some(RecleanDelivery::ClipboardOwnershipUnverified),
                "history",
                "history",
                Some("reclean_clipboard_ownership_unverified"),
            ),
            (
                Some(RecleanDelivery::ClipboardWriteFailed),
                "history",
                "history",
                Some("reclean_clipboard_write_failed"),
            ),
        ];
        for (delivery, status, delivery_method, fallback_reason) in cases {
            let dir = temp_dir("reclean-delivery");
            insert_failed_history(&dir, "raw source", 1.0, None).unwrap();
            let id = get_history(&dir, 10).unwrap()[0].id;
            save_history_revision(
                &dir,
                id,
                "prepared result",
                Some("ai_success"),
                None,
                None,
                None,
                "ai_reclean",
            )
            .unwrap();
            update_history_revision_state_with_reclean_delivery(
                &dir,
                id,
                false,
                None,
                Some("ai_success"),
                None,
                None,
                delivery,
                Some("success"),
                Some(&[]),
                None,
                None,
            )
            .unwrap();

            let item = get_history(&dir, 10).unwrap().remove(0);
            assert_eq!(item.status, status);
            assert_eq!(item.final_text, "prepared result");
            assert_eq!(item.delivery_method.as_deref(), Some(delivery_method));
            assert_eq!(item.fallback_reason.as_deref(), fallback_reason);
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn reclean_degraded_result_stays_in_history_without_a_clipboard_claim() {
        let dir = temp_dir("reclean-degraded-no-copy");
        insert_failed_history(&dir, "raw source", 1.0, None).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        save_history_revision(
            &dir,
            id,
            "raw source",
            Some("ai_failed_raw"),
            None,
            None,
            None,
            "ai_reclean",
        )
        .unwrap();
        update_history_revision_state_with_reclean_delivery(
            &dir,
            id,
            true,
            Some("native_input_too_large"),
            Some("ai_failed_raw"),
            None,
            None,
            None,
            Some("success"),
            Some(&[]),
            None,
            None,
        )
        .unwrap();

        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.status, "degraded");
        assert_eq!(
            item.degraded_reason.as_deref(),
            Some("native_input_too_large")
        );
        assert_eq!(item.delivery_method.as_deref(), Some("history"));
        assert_eq!(item.fallback_reason, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retry_cleanup_failure_stays_degraded() {
        let dir = temp_dir("retry-degraded");
        insert_failed_history(&dir, "raw", 1.0, None).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        mark_retried_with_texts(
            &dir,
            id,
            Some("raw"),
            Some("raw"),
            None,
            "raw",
            true,
            Some("llm_cleanup_failed"),
            None,
            RetryDelivery::ClipboardChanged,
        )
        .unwrap();
        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.status, "degraded");
        assert!(item.degraded);
        assert_eq!(item.degraded_reason.as_deref(), Some("llm_cleanup_failed"));
        assert_eq!(item.delivery_method.as_deref(), Some("history"));
        assert_eq!(
            item.fallback_reason.as_deref(),
            Some("retry_clipboard_changed")
        );
        assert!(failed_spool(&dir, id).unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_revisions_keep_the_parent_and_latest_text() {
        let dir = temp_dir("revisions");
        insert_history(&dir, "raw", "first", 1.0, false).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        let intent =
            crate::llm::CleanupIntent::selected_text(crate::llm::CleanupOperation::Shorten, "raw");
        save_history_revision(
            &dir,
            id,
            "second",
            Some("ai_success"),
            Some(&intent),
            Some(crate::llm::MODEL),
            Some(&crate::context::ContextPolicy::default()),
            "ai_reclean",
        )
        .unwrap();
        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.final_text, "second");
        assert_eq!(item.revision_count, 1);
        assert_eq!(history_text(&dir, id).unwrap(), "second");
        assert_eq!(get_history_revisions(&dir, id).unwrap().len(), 1);
        delete_history(&dir, id).unwrap();
        assert!(get_history_revisions(&dir, id).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retrying_manual_history_save_reuses_original_revision_evidence() {
        let dir = temp_dir("manual-history-learning-retry");
        insert_history(&dir, "raw", "adopted before", 1.0, false).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        let first = save_manual_history_revision(&dir, id, "adopted after").unwrap();
        assert!(first.changed);
        assert_eq!(first.learning_before_text, "adopted before");
        let first_revision_id = first.revision_id.unwrap();

        let retry = save_manual_history_revision(&dir, id, "adopted after").unwrap();
        assert!(!retry.changed);
        assert_eq!(retry.revision_id, Some(first_revision_id));
        assert_eq!(retry.learning_before_text, "adopted before");
        assert_eq!(get_history_revisions(&dir, id).unwrap().len(), 1);

        let next = save_manual_history_revision(&dir, id, "third adopted text").unwrap();
        assert!(next.changed);
        assert_eq!(next.learning_before_text, "adopted after");
        assert_eq!(get_history_revisions(&dir, id).unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn external_revision_lookup_requires_the_exact_dictation_and_revision_pair() {
        let dir = temp_dir("external-revision-pair");
        insert_history(&dir, "raw one", "parent final one", 1.0, false).unwrap();
        insert_history(&dir, "raw two", "parent final two", 1.0, false).unwrap();
        let rows = get_history(&dir, 10).unwrap();
        let first_parent = rows
            .iter()
            .find(|row| row.raw_text == "raw one")
            .unwrap()
            .id;
        let second_parent = rows
            .iter()
            .find(|row| row.raw_text == "raw two")
            .unwrap()
            .id;
        let intent = crate::llm::CleanupIntent::selected_text(
            crate::llm::CleanupOperation::Cleanup,
            "cleanup",
        );
        let first_revision = save_history_revision(
            &dir,
            first_parent,
            "first successful cleanup",
            Some("ai_success"),
            Some(&intent),
            None,
            None,
            "ai_reclean",
        )
        .unwrap();
        let second_revision = save_history_revision(
            &dir,
            first_parent,
            "second successful cleanup",
            Some("ai_success"),
            Some(&intent),
            None,
            None,
            "ai_reclean",
        )
        .unwrap();
        let degraded_revision = save_history_revision(
            &dir,
            first_parent,
            "degraded raw result",
            Some("ai_failed_raw"),
            Some(&intent),
            None,
            None,
            "ai_reclean",
        )
        .unwrap();
        let other_parent_revision = save_history_revision(
            &dir,
            second_parent,
            "another history result",
            Some("ai_success"),
            Some(&intent),
            None,
            None,
            "ai_reclean",
        )
        .unwrap();

        assert_eq!(
            get_history_revision(&dir, first_parent, first_revision)
                .unwrap()
                .unwrap()
                .final_text,
            "first successful cleanup"
        );
        assert_eq!(
            get_history_revision(&dir, first_parent, second_revision)
                .unwrap()
                .unwrap()
                .final_text,
            "second successful cleanup"
        );
        assert_eq!(
            get_history_revision(&dir, first_parent, degraded_revision)
                .unwrap()
                .unwrap()
                .cleanup_status
                .as_deref(),
            Some("ai_failed_raw")
        );
        assert!(
            get_history_revision(&dir, first_parent, other_parent_revision)
                .unwrap()
                .is_none()
        );
        assert!(get_history_revision(&dir, second_parent, first_revision)
            .unwrap()
            .is_none());
        assert!(get_history_revision(&dir, first_parent, i64::MAX)
            .unwrap()
            .is_none());
        delete_history(&dir, first_parent).unwrap();
        assert!(get_history_revision(&dir, first_parent, first_revision)
            .unwrap()
            .is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_reclean_state_does_not_create_a_fake_fallback_error() {
        let dir = temp_dir("revision-state");
        insert_history(&dir, "raw", "first", 1.0, false).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        update_history_revision_state(&dir, id, false, None, "copied", Some("ai_success")).unwrap();
        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.status, "copied");
        assert_eq!(item.fallback_reason, None);
        assert_eq!(item.cleanup_status, "ai_success");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dictionary_round_trips() {
        let mut settings = Settings::default();
        settings.dictionary.push("VoiceFlow".into());
        settings.dictionary.retain(|word| word != "missing");
        settings.dictionary.retain(|word| word != "VoiceFlow");
        assert!(settings.dictionary.is_empty());
    }

    #[test]
    fn default_chunk_settings_match_recommended_values() {
        let settings = Settings::default();
        assert_eq!(settings.chunk_threshold_secs, 25);
        assert_eq!(settings.chunk_length_secs, 35);
    }

    #[test]
    fn input_gain_defaults_to_one_and_clamps() {
        assert_eq!(Settings::default().input_gain, 1.0);
        let parsed: Settings = serde_json::from_str(r#"{"api_key":"x"}"#).unwrap();
        assert_eq!(parsed.input_gain, 1.0);
        assert_eq!(SettingsView::from(&parsed).input_gain, 1.0);
        let mut quiet = Settings {
            input_gain: 0.1,
            ..Settings::default()
        };
        quiet.normalize();
        assert_eq!(quiet.input_gain, 0.5);
        let mut loud = Settings {
            input_gain: 9.0,
            ..Settings::default()
        };
        loud.normalize();
        assert_eq!(loud.input_gain, 4.0);
        let mut invalid = Settings {
            input_gain: f32::NAN,
            ..Settings::default()
        };
        invalid.normalize();
        assert_eq!(invalid.input_gain, 1.0);
    }

    #[test]
    fn dictionary_learn_enabled_defaults_true() {
        assert!(Settings::default().dictionary_learn_enabled);
        assert!(!Settings::default().local_edit_learning_enabled);
        let parsed: Settings = serde_json::from_str(r#"{"api_key":"x"}"#).unwrap();
        assert!(parsed.dictionary_learn_enabled);
        assert!(!parsed.local_edit_learning_enabled);
        let view = SettingsView::from(&parsed);
        assert!(view.dictionary_learn_enabled);
        assert!(!view.local_edit_learning_enabled);
        let opted_in: Settings =
            serde_json::from_str(r#"{"api_key":"x","local_edit_learning_enabled":true}"#).unwrap();
        assert!(opted_in.local_edit_learning_enabled);
    }

    #[test]
    fn asr_base_url_defaults_empty_and_falls_back_to_groq_key() {
        assert!(Settings::default().asr_base_url.is_empty());
        let parsed: Settings = serde_json::from_str(r#"{"api_key":"gsk_fallback"}"#).unwrap();
        assert!(parsed.asr_base_url.is_empty());
        assert!(parsed.asr_api_key.is_empty());
        assert_eq!(parsed.asr_credential(), "gsk_fallback");
        let groq_url = Settings {
            api_key: "gsk_fallback".into(),
            asr_base_url: "https://api.groq.com/openai/v1".into(),
            ..Settings::default()
        };
        assert_eq!(groq_url.asr_credential(), "gsk_fallback");
        let custom_without_asr_key = Settings {
            api_key: "gsk_fallback".into(),
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            custom_base_url: "http://127.0.0.1:8000/v1".into(),
            ..Settings::default()
        };
        assert_eq!(custom_without_asr_key.asr_credential(), "");
        let with_asr_key = Settings {
            api_key: "gsk_fallback".into(),
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_api_key: "asr_only".into(),
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            custom_base_url: "http://127.0.0.1:8000/v1".into(),
            ..Settings::default()
        };
        assert_eq!(with_asr_key.asr_credential(), "asr_only");
        let view = SettingsView::from(&with_asr_key);
        assert_eq!(view.asr_base_url, "http://127.0.0.1:8000/v1");
        assert!(view.asr_api_key_configured);
        assert_eq!(view.asr_api_key_hint.as_deref(), Some("••••_only"));
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("asr_only"));
        assert!(!json.contains("gsk_fallback"));
    }

    #[test]
    fn asr_base_url_validate_rejects_bad_scheme_plaintext_and_keyless_custom() {
        assert!(Settings::default().validate().is_ok());
        let groq = Settings {
            asr_base_url: "https://api.groq.com/openai/v1".into(),
            ..Settings::default()
        };
        assert!(groq.validate().is_ok());
        let loopback_with_key = Settings {
            asr_api_key: "local-key".into(),
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            ..Settings::default()
        };
        assert!(loopback_with_key.validate().is_ok());
        let relative = Settings {
            asr_base_url: "asr.example.com/v1".into(),
            asr_api_key: "k".into(),
            ..Settings::default()
        };
        assert!(relative
            .validate()
            .unwrap_err()
            .to_string()
            .contains("http:// 或 https://"));
        let plaintext = Settings {
            asr_base_url: "http://asr.example.com/v1".into(),
            asr_api_key: "k".into(),
            ..Settings::default()
        };
        assert!(plaintext
            .validate()
            .unwrap_err()
            .to_string()
            .contains("https://"));
        let custom_without_key = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://asr.example.com/v1".into(),
            custom_base_url: "https://asr.example.com/v1".into(),
            onboarded: true,
            ..Settings::default()
        };
        assert_eq!(
            custom_without_key.validate().unwrap_err().to_string(),
            "自定义 ASR 地址需要填写 ASR 密钥。"
        );
        let loopback_without_key = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            custom_base_url: "http://127.0.0.1:8000/v1".into(),
            ..Settings::default()
        };
        assert!(loopback_without_key.validate().is_ok());
    }

    #[test]
    fn schema_17_promotes_openai_cleanup_host_into_the_provider_pool() {
        let mut settings = Settings {
            schema_version: 16,
            api_key: "gsk_keep".into(),
            asr_provider: crate::engine::EngineProvider::Groq,
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_base_url: "https://api.openai.com/v1".into(),
            cleanup_api_key: "sk-openai".into(),
            cleanup_model: "gpt-4o-mini".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(
            settings.cleanup_provider,
            crate::engine::EngineProvider::OpenAi
        );
        assert_eq!(
            settings.provider_api_keys.get("openai").map(String::as_str),
            Some("sk-openai")
        );
        assert_eq!(
            settings.provider_api_keys.get("groq").map(String::as_str),
            Some("gsk_keep")
        );
        assert!(settings.cleanup_base_url.is_empty());
        assert_eq!(settings.cleanup_model, "gpt-4o-mini");
        assert_eq!(settings.cleanup_credential(), "sk-openai");
        assert_eq!(settings.asr_credential(), "gsk_keep");
    }

    #[test]
    fn named_asr_does_not_reuse_the_groq_key() {
        let settings = Settings {
            api_key: "gsk_keep".into(),
            asr_provider: crate::engine::EngineProvider::OpenAi,
            ..Settings::default()
        };
        assert!(settings.asr_credential().is_empty());
        assert_eq!(
            settings.provider_secret(crate::engine::EngineProvider::Groq),
            "gsk_keep"
        );
    }

    #[test]
    fn bind_cleanup_key_clears_on_host_change_unless_a_new_key_is_supplied() {
        assert_eq!(
            bind_cleanup_key_to_host(
                crate::engine::EngineProvider::OpenAi,
                "https://api.groq.com/openai/v1",
                "https://api.openai.com/v1",
                "",
                "old-key"
            ),
            ""
        );
        assert_eq!(
            bind_cleanup_key_to_host(
                crate::engine::EngineProvider::OpenAi,
                "https://api.openai.com/v1",
                "https://api.openai.com/v1",
                "",
                "old-key"
            ),
            "old-key"
        );
    }

    #[test]
    fn custom_cleanup_key_binding_detects_scheme_and_port_changes() {
        use crate::engine::EngineProvider;

        assert_eq!(
            bind_cleanup_key_to_host(EngineProvider::Custom, "", "", "", "old-key"),
            "old-key",
            "an untouched empty Custom endpoint must not clear credentials"
        );
        assert_eq!(
            bind_cleanup_key_to_host(
                EngineProvider::Custom,
                "https://router.example/v1",
                "https://router.example:8443/v1",
                "",
                "old-key"
            ),
            ""
        );
        assert_eq!(
            bind_cleanup_key_to_host(
                EngineProvider::Custom,
                "http://router.example/v1",
                "https://router.example/v1",
                "",
                "old-key"
            ),
            ""
        );
        assert_eq!(
            bind_cleanup_key_to_host(
                EngineProvider::Custom,
                "https://router.example/v1",
                "https://router.example:443/chat/completions",
                "",
                "old-key"
            ),
            "old-key"
        );
    }

    #[test]
    fn bind_asr_key_clears_on_host_change_unless_a_new_key_is_supplied() {
        assert_eq!(
            bind_asr_key_to_host(
                crate::engine::EngineProvider::OpenAi,
                "https://api.groq.com/openai/v1",
                "https://asr.example.com/v1",
                "",
                "old-key"
            ),
            ""
        );
        assert_eq!(
            bind_asr_key_to_host(
                crate::engine::EngineProvider::OpenAi,
                "https://api.groq.com/openai/v1",
                "https://asr.example.com/v1",
                "new-key",
                "old-key"
            ),
            "new-key"
        );
        assert_eq!(
            bind_asr_key_to_host(
                crate::engine::EngineProvider::Groq,
                "",
                "https://api.groq.com/openai/v1",
                "",
                "old-key"
            ),
            "old-key"
        );
        assert_eq!(
            bind_asr_key_to_host(
                crate::engine::EngineProvider::OpenAi,
                "https://asr.example.com/v1",
                "https://asr.example.com/v1",
                "",
                "old-key"
            ),
            "old-key"
        );
    }

    #[test]
    fn custom_asr_key_binding_detects_scheme_and_port_changes() {
        use crate::engine::EngineProvider;

        assert_eq!(
            bind_asr_key_to_host(
                EngineProvider::Custom,
                "https://router.example/v1",
                "https://router.example:8443/v1",
                "",
                "old-key"
            ),
            ""
        );
        assert_eq!(
            bind_asr_key_to_host(
                EngineProvider::Custom,
                "http://router.example/v1",
                "https://router.example/v1",
                "",
                "old-key"
            ),
            ""
        );
        assert_eq!(
            bind_asr_key_to_host(
                EngineProvider::Custom,
                "https://router.example/v1",
                "https://router.example:443/audio/transcriptions",
                "",
                "old-key"
            ),
            "old-key"
        );
    }

    #[test]
    fn save_settings_never_writes_plaintext_asr_key() {
        let dir = temp_dir("asr-keyblank");
        let backend = FakeCredentialBackend::default();
        let settings = Settings {
            asr_api_key: "asr_secret_should_not_be_on_disk".into(),
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            ..Settings::default()
        };
        save_settings_with_backend(&dir, &settings, &backend).unwrap();
        let on_disk = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            !on_disk.contains("asr_secret_should_not_be_on_disk"),
            "plaintext ASR API key must never be written to settings.json"
        );
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(parsed["asr_api_key"].as_str().unwrap_or(""), "");
        assert_eq!(
            parsed["asr_base_url"].as_str().unwrap_or(""),
            "http://127.0.0.1:8000/v1"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_settings_never_writes_plaintext_cleanup_key() {
        let dir = temp_dir("cleanup-keyblank");
        let backend = FakeCredentialBackend::default();
        let settings = Settings {
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_base_url: "https://api.openai.com/v1".into(),
            cleanup_model: "gpt-4o-mini".into(),
            cleanup_api_key: "cleanup_secret_should_not_be_on_disk".into(),
            ..Settings::default()
        };
        save_settings_with_backend(&dir, &settings, &backend).unwrap();
        let on_disk = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            !on_disk.contains("cleanup_secret_should_not_be_on_disk"),
            "plaintext cleanup API key must never be written to settings.json"
        );
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(parsed["cleanup_api_key"].as_str().unwrap_or(""), "");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_page_uses_a_cursor_and_reports_more_rows() {
        let dir = temp_dir("history-page");
        insert_history(&dir, "one", "one", 1.0, false).unwrap();
        insert_history(&dir, "two", "two", 1.0, false).unwrap();
        insert_history(&dir, "three", "three", 1.0, false).unwrap();

        let first = get_history_page(&dir, 2, None, None).unwrap();
        assert_eq!(first.items.len(), 2);
        assert!(first.has_more);
        let before_id = first.items.last().unwrap().id;

        let second = get_history_page(&dir, 2, Some(before_id), None).unwrap();
        assert_eq!(second.items.len(), 1);
        assert!(!second.has_more);
        assert!(second.items[0].id < before_id);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_page_filters_both_text_columns_before_cursor_pagination() {
        let dir = temp_dir("history-search");
        insert_history(&dir, "first raw", "VoiceFlow first", 1.0, false).unwrap();
        insert_history(&dir, "unrelated", "other", 1.0, false).unwrap();
        insert_history(&dir, "VoiceFlow second", "cleaned", 1.0, false).unwrap();

        let first = get_history_page(&dir, 1, None, Some("VoiceFlow")).unwrap();
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].raw_text, "VoiceFlow second");
        assert!(first.has_more);

        let second = get_history_page(&dir, 1, Some(first.items[0].id), Some("VoiceFlow")).unwrap();
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.items[0].final_text, "VoiceFlow first");
        assert!(!second.has_more);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_search_escapes_like_wildcards() {
        let dir = temp_dir("history-search-escape");
        insert_history(&dir, "plain", "plain", 1.0, false).unwrap();
        insert_history(&dir, "100% complete", "done", 1.0, false).unwrap();
        insert_history(&dir, "under_score", "other", 1.0, false).unwrap();
        insert_history(&dir, r"path\to\file", "slash", 1.0, false).unwrap();

        let percent = get_history_page(&dir, 10, None, Some("%")).unwrap();
        assert_eq!(percent.items.len(), 1);
        assert_eq!(percent.items[0].raw_text, "100% complete");

        let underscore = get_history_page(&dir, 10, None, Some("_")).unwrap();
        assert_eq!(underscore.items.len(), 1);
        assert_eq!(underscore.items[0].raw_text, "under_score");

        let slash = get_history_page(&dir, 10, None, Some("\\")).unwrap();
        assert_eq!(slash.items.len(), 1);
        assert_eq!(slash.items[0].raw_text, r"path\to\file");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_retention_removes_old_text_and_audio_reference() {
        let dir = temp_dir("history-retention");
        insert_history(&dir, "old", "old", 1.0, false).unwrap();
        insert_history(&dir, "new", "new", 1.0, false).unwrap();
        let connection = open_history(&dir).unwrap();
        connection
            .execute(
                "UPDATE dictations SET created_at=datetime('now','-400 days') WHERE raw_text='old'",
                [],
            )
            .unwrap();
        drop(connection);

        assert_eq!(purge_history(&dir, 365).unwrap(), 1);
        let page = get_history_page(&dir, 10, None, None).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].raw_text, "new");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn purge_history_prunes_style_drafts_and_usage() {
        let dir = temp_dir("history-retention-related-tables");
        insert_history(&dir, "old", "old", 1.0, false).unwrap();
        insert_history(&dir, "new", "new", 1.0, false).unwrap();
        upsert_style_draft(&dir, "mapping", "style", "excerpt", "before", "after").unwrap();
        upsert_style_draft(&dir, "mapping-new", "style", "excerpt", "before", "after").unwrap();
        upsert_style_draft(
            &dir,
            "mapping-boundary",
            "style",
            "excerpt",
            "before",
            "after",
        )
        .unwrap();
        let connection = open_history(&dir).unwrap();
        connection.execute_batch(
            "UPDATE dictations SET created_at = CASE raw_text WHEN 'old' THEN datetime('now','-400 days') ELSE datetime('now') END;
             UPDATE style_drafts SET created_at = CASE mapping_id WHEN 'mapping' THEN datetime('now','-400 days') WHEN 'mapping-boundary' THEN datetime('now','-365 days','+1 minute') ELSE datetime('now') END;
             INSERT INTO usage(day,asr_requests,llm_requests,audio_seconds) VALUES(date('now','-400 days'),4,2,8),(date('now','-365 days'),2,1,4);",
        ).unwrap();
        drop(connection);

        assert_eq!(purge_history(&dir, 365).unwrap(), 1);
        let connection = open_history(&dir).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM dictations", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM style_drafts", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM usage", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM usage WHERE day=date('now','-400 days')",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM usage WHERE day IN (date('now','-365 days'), date('now'))", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        drop(connection);

        insert_history(&dir, "forever", "forever", 1.0, false).unwrap();
        upsert_style_draft(&dir, "mapping-old", "style", "excerpt", "before", "after").unwrap();
        let connection = open_history(&dir).unwrap();
        connection.execute_batch(
            "UPDATE dictations SET created_at = datetime('now','-4000 days');
             UPDATE style_drafts SET created_at = datetime('now','-4000 days');
             INSERT INTO usage(day,asr_requests,llm_requests,audio_seconds) VALUES(date('now','-4000 days'),4,2,8);",
        ).unwrap();
        drop(connection);
        assert_eq!(purge_history(&dir, 0).unwrap(), 0);
        let connection = open_history(&dir).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM dictations", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM style_drafts", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM usage", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
        drop(connection);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_retention_zero_keeps_history_forever() {
        let dir = temp_dir("history-retention-forever");
        insert_history(&dir, "old", "old", 1.0, false).unwrap();
        let connection = open_history(&dir).unwrap();
        connection
            .execute(
                "UPDATE dictations SET created_at=datetime('now','-4000 days')",
                [],
            )
            .unwrap();
        drop(connection);

        assert_eq!(purge_history(&dir, 0).unwrap(), 0);
        let page = get_history_page(&dir, 10, None, None).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].raw_text, "old");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clear_all_data_removes_history_usage_and_spool() {
        let dir = temp_dir("clear-all-data");
        insert_history(&dir, "raw", "final", 2.0, false).unwrap();
        write_spool_file(&dir, Path::new("recovery.wav"), b"audio").unwrap();
        write_gold_file(&dir, "kept.wav", b"gold").unwrap();

        clear_all_data(&dir).unwrap();

        assert!(get_history_page(&dir, 10, None, None)
            .unwrap()
            .items
            .is_empty());
        assert_eq!(
            get_usage(&dir, crate::queue::RequestGate::new(None).snapshots())
                .unwrap()
                .asr_requests,
            0
        );
        assert!(dir.join("spool").is_dir());
        assert!(std::fs::read_dir(dir.join("spool"))
            .unwrap()
            .next()
            .is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("spool"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700);
        }
        assert!(!dir.join("gold").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clear_all_data_does_not_delete_on_device_models() {
        let dir = temp_dir("clear-keeps-models");
        insert_history(&dir, "raw", "final", 1.0, false).unwrap();
        let models = dir.join("models").join("sensevoice-small");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::write(models.join("model.int8.onnx"), b"onnx").unwrap();
        std::fs::write(models.join("tokens.txt"), b"tokens").unwrap();
        let sidecar = dir.join("models").join("sensevoice-small.archive.sha256");
        std::fs::write(&sidecar, crate::ondevice_models::SENSEVOICE_ARCHIVE_SHA256).unwrap();

        clear_all_data(&dir).unwrap();

        assert_eq!(
            std::fs::read(models.join("model.int8.onnx")).unwrap(),
            b"onnx"
        );
        assert_eq!(std::fs::read(models.join("tokens.txt")).unwrap(), b"tokens");
        assert_eq!(
            std::fs::read_to_string(&sidecar).unwrap(),
            crate::ondevice_models::SENSEVOICE_ARCHIVE_SHA256
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_utf8_backup_precedes_credential_writes_and_backup_failures() {
        let dir = temp_dir("malformed-utf8-credential");
        std::fs::create_dir_all(&dir).unwrap();
        let settings_path = dir.join("settings.json");
        let raw = [0xff, 0xfe, b'{'];
        std::fs::write(&settings_path, raw).unwrap();
        let backend = FakeCredentialBackend::default();
        let (loaded, needs_persist) = load_settings_with_backend(&dir, &backend);
        assert!(!needs_persist);
        assert!(!loaded.onboarded);
        assert_eq!(std::fs::read(&settings_path).unwrap(), raw);
        let settings = Settings {
            api_key: "synthetic-key".into(),
            ..Settings::default()
        };
        persist_settings_with_backend(&dir, &settings, &backend).unwrap();
        assert_eq!(
            std::fs::read(dir.join("settings.json.malformed.bak")).unwrap(),
            raw
        );
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(dir.join("settings.json.malformed.bak"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(
            serde_json::from_slice::<Settings>(&std::fs::read(&settings_path).unwrap()).is_ok()
        );
        assert!(!backend.writes.borrow().is_empty());
        let _ = std::fs::remove_dir_all(&dir);

        let dir = temp_dir("malformed-backup-directory");
        std::fs::create_dir_all(&dir).unwrap();
        let raw = b"{ malformed";
        std::fs::write(dir.join("settings.json"), raw).unwrap();
        std::fs::create_dir(dir.join("settings.json.malformed.bak")).unwrap();
        let backend = FakeCredentialBackend::default();
        let settings = Settings {
            api_key: "must-not-write".into(),
            ..Settings::default()
        };
        assert!(persist_settings_with_backend(&dir, &settings, &backend).is_err());
        assert!(backend.writes.borrow().is_empty());
        assert_eq!(std::fs::read(dir.join("settings.json")).unwrap(), raw);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_settings_are_kept_untouched() {
        let dir = temp_dir("malformed-settings");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let raw = b"{not valid json";
        std::fs::write(&path, raw).unwrap();

        let (settings, needs_persist) =
            load_settings_with_backend(&dir, &FakeCredentialBackend::default());

        assert_eq!(std::fs::read(&path).unwrap(), raw);
        assert!(!needs_persist);
        assert!(!settings.onboarded);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learn_pair_upsert_counts_only_and_leaves_dictionary_empty() {
        let dir = temp_dir("learn-pairs-count");
        std::fs::create_dir_all(&dir).unwrap();
        save_settings(
            &dir,
            &Settings {
                dictionary: vec![],
                ..Settings::default()
            },
        )
        .unwrap();
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        assert_eq!(
            upsert_learn_pair(&dir, &key, "知呼", "知乎")
                .unwrap()
                .unwrap()
                .hits,
            1
        );
        upsert_learn_pair(&dir, &key, "知呼", "知乎").unwrap();
        let third = upsert_learn_pair(&dir, &key, "知呼", "知乎")
            .unwrap()
            .unwrap();
        assert_eq!(third.hits, 3);
        assert!(!third.promoted);
        assert!(load_settings(&dir).0.dictionary.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dictionary_sources_preserve_owners_and_reconcile_case_insensitively() {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.insert(
            "dictionary".into(),
            serde_json::json!(["VoiceFlow", "OldTerm"]),
        );
        object.remove("dictionary_sources");
        let mut settings: Settings = serde_json::from_value(value).unwrap();
        settings.normalize_dictionary_sources();

        assert!(settings.has_dictionary_source("voiceflow", &DictionarySource::LegacyUnknown));
        assert!(settings.has_dictionary_source("oldterm", &DictionarySource::LegacyUnknown));
        assert!(settings
            .add_dictionary_source("voiceflow", DictionarySource::Rule("rule:voiceflo".into())));
        assert!(settings.add_dictionary_source(
            "ScreenTerm",
            DictionarySource::Screen("screen-origin".into())
        ));
        assert!(!settings.has_dictionary_source("screenterm", &DictionarySource::LegacyUnknown));
        assert_eq!(
            add_manual_dictionary_words(
                &mut settings,
                &["voiceflow".into(), "NewTerm".into(), "NEWTERM".into()]
            ),
            2
        );
        assert!(settings.has_dictionary_source("VOICEFLOW", &DictionarySource::Manual));
        assert!(settings
            .has_dictionary_source("VoiceFlow", &DictionarySource::Rule("rule:voiceflo".into())));
        assert_eq!(
            settings
                .dictionary
                .iter()
                .filter(|word| word.eq_ignore_ascii_case("voiceflow"))
                .count(),
            1
        );

        let requested = vec!["voiceflow".into(), "FreshTerm".into(), "FRESHTERM".into()];
        settings.dictionary = requested.clone();
        let removed = reconcile_dictionary_patch(&mut settings, requested);
        assert!(removed.iter().any(|word| word == "OldTerm"));
        assert!(removed.iter().any(|word| word == "ScreenTerm"));
        assert!(removed.iter().any(|word| word == "NewTerm"));
        assert!(settings.has_dictionary_source("VoiceFlow", &DictionarySource::Manual));
        assert!(settings
            .has_dictionary_source("voiceflow", &DictionarySource::Rule("rule:voiceflo".into())));
        assert!(settings.has_dictionary_source("freshterm", &DictionarySource::Manual));
        assert!(!settings.has_dictionary_source("freshterm", &DictionarySource::LegacyUnknown));
        assert!(!settings.has_dictionary_source(
            "screenterm",
            &DictionarySource::Screen("screen-origin".into())
        ));
    }

    #[test]
    fn dictionary_source_migration_merges_ascii_case_variants_and_owners() {
        let mut settings = Settings {
            dictionary: vec!["VoiceFlow".into(), "voiceflow".into(), "OtherTerm".into()],
            dictionary_sources: legacy_dictionary_sources(),
            ..Settings::default()
        };
        settings.dictionary_sources.owners.insert(
            "VOICEFLOW".into(),
            [DictionarySource::Rule("rule:voiceflo".into())]
                .into_iter()
                .collect(),
        );

        settings.normalize_dictionary_sources();

        assert_eq!(settings.dictionary, vec!["VoiceFlow", "OtherTerm"]);
        assert!(settings.has_dictionary_source("voiceflow", &DictionarySource::LegacyUnknown));
        assert!(settings
            .has_dictionary_source("VOICEFLOW", &DictionarySource::Rule("rule:voiceflo".into())));
        assert_eq!(settings.dictionary_sources.owners.len(), 2);
    }

    #[test]
    fn dictionary_projection_and_persistence_are_capped_without_resurrecting_dropped_words() {
        let dir = temp_dir("dictionary-owner-cap");
        let mut settings = Settings {
            dictionary: (0..257).map(|index| format!("term-{index:03}")).collect(),
            ..Settings::default()
        };
        settings.dictionary_sources.owners.insert(
            "term-256".into(),
            [DictionarySource::Screen("late-screen-origin".into())]
                .into_iter()
                .collect(),
        );

        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.json"),
            serde_json::to_vec_pretty(&settings).unwrap(),
        )
        .unwrap();
        let (mut loaded, needs_persist) =
            load_settings_with_backend(&dir, &FakeCredentialBackend::default());
        assert!(needs_persist);
        assert_eq!(loaded.dictionary.len(), 256);
        assert_eq!(loaded.dictionary_sources.owners.len(), 256);
        assert!(!loaded.has_dictionary_source(
            "term-256",
            &DictionarySource::Screen("late-screen-origin".into())
        ));
        assert!(loaded.can_add_dictionary_source("TERM-000"));
        assert!(!loaded.can_add_dictionary_source("term-256"));

        let requested = (0..257).map(|index| format!("term-{index:03}")).collect();
        reconcile_dictionary_patch(&mut loaded, requested);
        assert_eq!(loaded.dictionary.len(), 256);
        assert_eq!(loaded.dictionary_sources.owners.len(), 256);
        assert!(!dictionary_contains_word(&loaded.dictionary, "term-256"));
        assert!(!loaded.dictionary_sources.owners.contains_key("term-256"));

        persist_settings_without_credential_changes(&dir, &loaded).unwrap();
        let (reloaded, _) = load_settings_with_backend(&dir, &FakeCredentialBackend::default());
        assert_eq!(reloaded.dictionary.len(), 256);
        assert_eq!(reloaded.dictionary_sources.owners.len(), 256);
        assert!(!dictionary_contains_word(&reloaded.dictionary, "term-256"));
        assert!(!reloaded.dictionary_sources.owners.contains_key("term-256"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn history_v18_migration_preserves_legacy_receipts_without_counting_them_as_proof() {
        let dir = temp_dir("history-v18-learning-proof-migration");
        fs::create_dir_all(&dir).unwrap();
        let database = dir.join("history.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE dictations (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL);
                 CREATE TABLE learn_observations (
                     observation_key TEXT PRIMARY KEY,
                     source_kind TEXT NOT NULL,
                     source_history_id INTEGER,
                     source_revision_id INTEGER,
                     created_at TEXT NOT NULL
                 );
                 INSERT INTO learn_observations VALUES ('legacy-key', 'paste_session', NULL, NULL, '2026-11-06');
                 PRAGMA user_version = 18;",
            )
            .unwrap();
        drop(connection);

        let migrated = open_history(&dir).unwrap();
        let version: i32 = migrated
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, HISTORY_SCHEMA_VERSION);
        let legacy = migrated
            .query_row(
                "SELECT source_kind, rule_key, dictation_origin, channel, proof_eligible
                 FROM learn_observations WHERE observation_key = 'legacy-key'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(legacy, ("paste_session".into(), None, None, None, 0));
        let proof_index: bool = migrated
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_learn_observations_independent_proof')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(proof_index);
        assert_eq!(
            count_independent_learn_pair_hits(&dir, "legacy-key").unwrap(),
            0
        );
        drop(migrated);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_research_fixture_receipt_replay_keeps_key_without_adding_hit_or_proof() {
        let dir = temp_dir("legacy-research-receipt-replay");
        let pair_key = crate::dictionary_learn::pair_key("VoiceFlo", "VoiceFlow");
        let source_origin = "voiceflow-origin-legacy";
        let observation_id = format!("stable_source:{source_origin}");
        let legacy_receipt =
            LearnPairObservation::research_fixture_edit(&observation_id, &pair_key);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&legacy_receipt),
            )
            .unwrap(),
            LearnPairUpsert::Stored(_)
        ));
        assert_eq!(get_learn_pair(&dir, &pair_key).unwrap().unwrap().hits, 1);
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            0
        );

        let replay = LearnPairObservation::research_fixture_origin_with_observation_id(
            &observation_id,
            source_origin,
            &pair_key,
        );
        assert!(has_learn_pair_observation(&dir, &replay).unwrap());
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&replay),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::DuplicateObservation)
        ));
        assert_eq!(get_learn_pair(&dir, &pair_key).unwrap().unwrap().hits, 1);
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            0
        );
        let connection = open_history(&dir).unwrap();
        let receipt: (String, Option<String>, Option<String>, i64) = connection
            .query_row(
                "SELECT source_kind, rule_key, dictation_origin, proof_eligible
                 FROM learn_observations WHERE observation_key = ?1",
                [&replay.observation_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(receipt, ("research_fixture_edit".into(), None, None, 0));
        drop(connection);
        fs::remove_dir_all(dir).unwrap();
    }

    fn insert_test_history_origin(dir: &Path, text: &str, transaction_id: &str) -> i64 {
        insert_history(dir, text, text, 1.0, false).unwrap();
        let connection = open_history(dir).unwrap();
        let id = connection
            .query_row(
                "SELECT id FROM dictations WHERE raw_text = ?1 ORDER BY id DESC LIMIT 1",
                [text],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        connection
            .execute(
                "UPDATE dictations SET transaction_id = ?1 WHERE id = ?2",
                params![transaction_id, id],
            )
            .unwrap();
        id
    }

    #[test]
    fn paste_proof_links_to_pending_history_and_deletes_with_its_origin() {
        let dir = temp_dir("learning-proof-pending-history-link");
        let transaction_id = "fixture-transaction-001";
        let pair_key = crate::dictionary_learn::pair_key("VoiceFlo", "VoiceFlow");
        let owner = register_pending_history_owner(&dir, transaction_id).unwrap();
        let intent =
            pending_history_intent_for_test(transaction_id, "ok", "history", "I prefer VoiceFlow");
        write_pending_history_intent(&dir, &intent, &owner).unwrap();

        let observation = LearnPairObservation::paste_session_with_dictation_origin(
            "session-001",
            &pair_key,
            transaction_id,
        );
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&observation),
            )
            .unwrap(),
            LearnPairUpsert::Stored(_)
        ));
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            1
        );
        let connection = open_history(&dir).unwrap();
        let pending_link: Option<i64> = connection
            .query_row(
                "SELECT source_history_id FROM learn_observations WHERE dictation_origin = ?1",
                [transaction_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending_link, None);
        drop(connection);

        promote_pending_history_intent(&dir, transaction_id, &owner).unwrap();
        drop(owner);
        let connection = open_history(&dir).unwrap();
        let history_id: i64 = connection
            .query_row(
                "SELECT id FROM dictations WHERE transaction_id = ?1",
                [transaction_id],
                |row| row.get(0),
            )
            .unwrap();
        let linked_id: Option<i64> = connection
            .query_row(
                "SELECT source_history_id FROM learn_observations WHERE dictation_origin = ?1",
                [transaction_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(linked_id, Some(history_id));
        connection
            .execute(
                "INSERT INTO dictation_revisions (dictation_id, created_at, final_text, revision_reason)
                 VALUES (?1, datetime('now'), 'I prefer VoiceFlo', 'manual')",
                [history_id],
            )
            .unwrap();
        let revision_id = connection.last_insert_rowid();
        drop(connection);

        let history_edit =
            LearnPairObservation::history_edit_from_store(&dir, history_id, revision_id, &pair_key)
                .unwrap()
                .unwrap();
        assert!(history_edit.proof_eligible());
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&history_edit),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::DuplicateObservation)
        ));
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            1
        );
        assert_eq!(get_learn_pair(&dir, &pair_key).unwrap().unwrap().hits, 1);
        let connection = open_history(&dir).unwrap();
        let enrichment: (Option<i64>, Option<String>) = connection
            .query_row(
                "SELECT source_revision_id, channel FROM learn_observations WHERE dictation_origin = ?1",
                [transaction_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(enrichment, (Some(revision_id), Some("history_edit".into())));
        drop(connection);

        delete_history(&dir, history_id).unwrap();
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            0
        );
        let connection = open_history(&dir).unwrap();
        let remaining: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM learn_observations WHERE dictation_origin = ?1",
                [transaction_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
        drop(connection);
        let late_paste = LearnPairObservation::paste_session_with_dictation_origin(
            "late-session",
            &pair_key,
            transaction_id,
        );
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&late_paste),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::UnknownDictationOrigin)
        ));
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            0
        );

        let history_first_origin = "history-first-origin-002";
        let history_first_id =
            insert_test_history_origin(&dir, "history-first proof origin", history_first_origin);
        let connection = open_history(&dir).unwrap();
        connection
            .execute(
                "INSERT INTO dictation_revisions (dictation_id, created_at, final_text, revision_reason)
                 VALUES (?1, datetime('now'), 'I prefer VoiceFlo', 'manual')",
                [history_first_id],
            )
            .unwrap();
        let history_first_revision = connection.last_insert_rowid();
        drop(connection);

        let history_first = LearnPairObservation::history_edit_from_store(
            &dir,
            history_first_id,
            history_first_revision,
            &pair_key,
        )
        .unwrap()
        .unwrap();
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&history_first),
            )
            .unwrap(),
            LearnPairUpsert::Stored(_)
        ));
        let paste_replay = LearnPairObservation::paste_session_with_dictation_origin(
            "history-first-session",
            &pair_key,
            history_first_origin,
        );
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&paste_replay),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::DuplicateObservation)
        ));
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            1
        );
        let connection = open_history(&dir).unwrap();
        let history_first_readback: (Option<i64>, Option<i64>, Option<String>) = connection
            .query_row(
                "SELECT source_history_id, source_revision_id, channel
                 FROM learn_observations WHERE dictation_origin = ?1",
                [history_first_origin],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            history_first_readback,
            (
                Some(history_first_id),
                Some(history_first_revision),
                Some("history_edit".into())
            )
        );
        drop(connection);

        delete_history(&dir, history_first_id).unwrap();
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            0
        );
        let connection = open_history(&dir).unwrap();
        let deleted_origin_receipts: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM learn_observations WHERE dictation_origin = ?1",
                [history_first_origin],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(deleted_origin_receipts, 0);
        drop(connection);
        let late_paste = LearnPairObservation::paste_session_with_dictation_origin(
            "history-first-late-session",
            &pair_key,
            history_first_origin,
        );
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&late_paste),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::UnknownDictationOrigin)
        ));
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            0
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn independent_proof_threshold_ignores_receipt_hits_and_replays() {
        let dir = temp_dir("learn-pair-independent-proof-threshold");
        let pair_key = crate::dictionary_learn::pair_key("VoiceFlo", "VoiceFlow");
        for _ in 0..3 {
            upsert_learn_pair(&dir, &pair_key, "VoiceFlo", "VoiceFlow").unwrap();
        }
        assert!(!promote_learn_pair_if_eligible(
            &dir,
            &pair_key,
            "VoiceFlo",
            "VoiceFlow",
            None,
            2,
            false,
        )
        .unwrap());

        let first_id = insert_test_history_origin(&dir, "first proof origin", "origin-first");
        let second_id = insert_test_history_origin(&dir, "second proof origin", "origin-second");
        for (session, origin) in [
            ("session-first", "origin-first"),
            ("session-second", "origin-second"),
        ] {
            let observation = LearnPairObservation::paste_session_with_dictation_origin(
                session, &pair_key, origin,
            );
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&observation),
            )
            .unwrap();
            if origin == "origin-first" {
                assert!(!promote_learn_pair_if_eligible(
                    &dir,
                    &pair_key,
                    "VoiceFlo",
                    "VoiceFlow",
                    None,
                    2,
                    false,
                )
                .unwrap());
            }
        }
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            2
        );
        assert!(promote_learn_pair_if_eligible(
            &dir,
            &pair_key,
            "VoiceFlo",
            "VoiceFlow",
            None,
            2,
            false,
        )
        .unwrap());
        let hits_before_replay = get_learn_pair(&dir, &pair_key).unwrap().unwrap().hits;
        let replay = LearnPairObservation::paste_session_with_dictation_origin(
            "session-first-replayed",
            &pair_key,
            "origin-first",
        );
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&replay),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::DuplicateObservation)
        ));
        assert_eq!(
            get_learn_pair(&dir, &pair_key).unwrap().unwrap().hits,
            hits_before_replay
        );
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            2
        );
        let connection = open_history(&dir).unwrap();
        connection
            .execute(
                "DELETE FROM dictations WHERE id IN (?1, ?2)",
                params![first_id, second_id],
            )
            .unwrap();
        drop(connection);
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            0
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dictionary_effect_versions_reject_stale_ack_and_cancel_pending_work() {
        let dir = temp_dir("dictionary-effect-versions");
        let pair_key = crate::dictionary_learn::pair_key("VoiceFlo", "VoiceFlow");
        ensure_learn_pair_promoted(
            &dir,
            &pair_key,
            "VoiceFlo",
            "VoiceFlow",
            Some(&LearnPairScope::default()),
        )
        .unwrap();
        let add = list_pending_dictionary_effects(&dir).unwrap().remove(0);
        assert_eq!(add.action, DictionaryEffectAction::Add);
        assert!(!acknowledge_dictionary_effect(&dir, &pair_key, add.version - 1).unwrap());
        assert!(acknowledge_dictionary_effect(&dir, &pair_key, add.version).unwrap());

        ensure_learn_pair_promoted(
            &dir,
            &pair_key,
            "VoiceFlo",
            "VoiceFlow",
            Some(&LearnPairScope::default()),
        )
        .unwrap();
        let reconfirmed_add = list_pending_dictionary_effects(&dir).unwrap().remove(0);
        assert_eq!(reconfirmed_add.action, DictionaryEffectAction::Add);
        assert!(reconfirmed_add.version > add.version);
        assert!(cancel_pending_dictionary_effects_for_word(&dir, "voiceflow").unwrap());
        assert!(list_pending_dictionary_effects(&dir).unwrap().is_empty());
        assert!(!acknowledge_dictionary_effect(&dir, &pair_key, reconfirmed_add.version).unwrap());

        ensure_learn_pair_promoted(
            &dir,
            &pair_key,
            "VoiceFlo",
            "VoiceFlow",
            Some(&LearnPairScope::default()),
        )
        .unwrap();
        let next_add = list_pending_dictionary_effects(&dir).unwrap().remove(0);
        assert!(acknowledge_dictionary_effect(&dir, &pair_key, next_add.version).unwrap());
        assert!(tombstone_learn_pair(&dir, &pair_key).unwrap());
        let remove = list_pending_dictionary_effects(&dir).unwrap().remove(0);
        assert_eq!(remove.action, DictionaryEffectAction::Remove);
        assert!(!acknowledge_dictionary_effect(&dir, &pair_key, next_add.version).unwrap());
        assert!(!cancel_pending_dictionary_effects_for_word(&dir, "voiceflow").unwrap());
        assert_eq!(
            list_pending_dictionary_effects(&dir).unwrap(),
            vec![remove.clone()]
        );
        assert!(!acknowledge_dictionary_effect(&dir, &pair_key, remove.version - 1).unwrap());
        assert!(acknowledge_dictionary_effect(&dir, &pair_key, remove.version).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clear_all_data_deletes_learn_pairs() {
        let dir = temp_dir("learn-pairs-clear");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        upsert_learn_pair(&dir, &key, "知呼", "知乎").unwrap();
        clear_all_data(&dir).unwrap();
        assert!(list_learn_pairs(&dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learned_usage_deduplicates_processing_passes_hides_forgotten_terms_and_clears() {
        let dir = temp_dir("learned-usage");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        ensure_learn_pair_promoted(&dir, &key, "知呼", "知乎", Some(&LearnPairScope::default()))
            .unwrap();
        assert!(list_learned_term_usage(&dir).unwrap().is_empty());
        record_learned_term_usage(&dir, &["知乎".into(), "知乎".into()]).unwrap();
        record_learned_term_usage(&dir, &["知乎".into()]).unwrap();
        let usage = list_learned_term_usage(&dir).unwrap();
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].word, "知乎");
        assert_eq!(usage[0].replacement_runs, 2);
        assert!(!usage[0].last_replaced_at.is_empty());
        tombstone_learn_pair(&dir, &key).unwrap();
        assert!(list_learned_term_usage(&dir).unwrap().is_empty());
        clear_all_data(&dir).unwrap();
        let count: u64 = open_history(&dir)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM learned_term_usage", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learned_usage_membership_matches_ascii_case_of_promoted_rule_surface() {
        let dir = temp_dir("learned-usage-ascii-case");
        let key = crate::dictionary_learn::pair_key("VoiceFlo", "VoiceFlow");
        ensure_learn_pair_promoted(
            &dir,
            &key,
            "VoiceFlo",
            "VoiceFlow",
            Some(&LearnPairScope::default()),
        )
        .unwrap();
        record_learned_term_usage(&dir, &["voiceflow".into()]).unwrap();

        let usage = list_learned_term_usage(&dir).unwrap();
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].word, "voiceflow");
        assert_eq!(usage[0].replacement_runs, 1);

        tombstone_learn_pair(&dir, &key).unwrap();
        assert!(list_learned_term_usage(&dir).unwrap().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn automatic_scene_pair_marker_is_opt_in_and_tombstones_still_disarm_it() {
        let dir = temp_dir("auto-scoped-pair");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        let scope = LearnPairScope {
            family: Some("prompt_or_code".into()),
            mapping_id: Some("codex".into()),
            native_bundle: Some("com.openai.codex".into()),
            ..LearnPairScope::default()
        };
        ensure_learn_pair_promoted(&dir, &key, "知呼", "知乎", Some(&scope)).unwrap();
        let legacy = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert!(!legacy.auto_scoped);
        assert!(!legacy.manual_scoped);

        mark_learn_pair_promoted_with_scope_kind(&dir, &key, true).unwrap();
        let learned = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert!(learned.auto_scoped);
        assert!(!learned.manual_scoped);
        tombstone_learn_pair(&dir, &key).unwrap();
        let forgotten = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert!(forgotten.auto_scoped);
        assert!(!forgotten.is_live_promoted());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn guarded_automatic_pair_promotion_requires_exact_live_candidate() {
        let dir = temp_dir("guarded-auto-promotion");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        let scope = LearnPairScope {
            family: Some("prompt_or_code".into()),
            mapping_id: Some("codex".into()),
            native_bundle: Some("com.openai.codex".into()),
            ..LearnPairScope::default()
        };
        upsert_learn_pair_with_observation(&dir, &key, "知呼", "知乎", Some(&scope), None).unwrap();

        assert!(
            !promote_learn_pair_if_eligible(&dir, &key, "知呼", "知乎", Some(&scope), 2, true,)
                .unwrap()
        );
        assert!(!promote_learn_pair_if_eligible(
            &dir,
            &key,
            "wrong",
            "知乎",
            Some(&scope),
            1,
            true,
        )
        .unwrap());
        assert!(!promote_learn_pair_if_eligible(
            &dir,
            &key,
            "知呼",
            "wrong",
            Some(&scope),
            1,
            true,
        )
        .unwrap());
        assert!(
            !promote_learn_pair_if_eligible(&dir, &key, "知呼", "知乎", None, 1, true,).unwrap()
        );

        upsert_learn_pair_with_observation(&dir, &key, "知呼", "知乎", Some(&scope), None).unwrap();
        assert!(tombstone_learn_pair(&dir, &key).unwrap());
        assert!(
            !promote_learn_pair_if_eligible(&dir, &key, "知呼", "知乎", Some(&scope), 2, true,)
                .unwrap()
        );
        let row = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert_eq!(row.hits, 2);
        assert!(!row.promoted);
        assert!(!row.auto_scoped);
        assert!(row.ignored);
        assert!(row.tombstoned_at.is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_v12_migration_preserves_learning_without_inventing_usage() {
        let dir = temp_dir("learned-usage-migration");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        ensure_learn_pair_promoted(&dir, &key, "知呼", "知乎", Some(&LearnPairScope::default()))
            .unwrap();
        let connection = open_history(&dir).unwrap();
        connection
            .execute_batch("DROP TABLE learned_term_usage; PRAGMA user_version = 12;")
            .unwrap();
        drop(connection);
        assert!(list_learned_term_usage(&dir).unwrap().is_empty());
        assert!(list_learn_pairs(&dir).unwrap()[0].is_live_promoted());
        assert!(dir.join("history.sqlite.v12.bak").is_file());
        clear_all_data(&dir).unwrap();
        assert!(!dir.join("history.sqlite.v12.bak").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learn_pair_concurrent_increments_are_not_lost() {
        let dir = temp_dir("learn-pairs-race");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let dir = &dir;
                let key = &key;
                scope.spawn(move || {
                    upsert_learn_pair(dir, key, "知呼", "知乎").unwrap();
                });
            }
        });
        assert_eq!(get_learn_pair(&dir, &key).unwrap().unwrap().hits, 8);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learn_pair_pending_cap_rejects_new_pairs() {
        let dir = temp_dir("learn-pairs-cap");
        for index in 0..256 {
            let before = format!("b{index}");
            let after = format!("a{index}");
            let key = crate::dictionary_learn::pair_key(&before, &after);
            assert!(upsert_learn_pair(&dir, &key, &before, &after)
                .unwrap()
                .is_some());
        }
        let extra = crate::dictionary_learn::pair_key("知呼", "知乎");
        assert!(matches!(
            upsert_learn_pair_with_scope(&dir, &extra, "知呼", "知乎", None).unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::PendingCapacity)
        ));
        let first = crate::dictionary_learn::pair_key("b0", "a0");
        assert_eq!(
            upsert_learn_pair(&dir, &first, "b0", "a0")
                .unwrap()
                .unwrap()
                .hits,
            2
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ignore_writes_tombstone_and_blocks_upsert() {
        let dir = temp_dir("learn-pairs-tombstone");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        upsert_learn_pair(&dir, &key, "知呼", "知乎").unwrap();
        assert!(tombstone_learn_pair(&dir, &key).unwrap());
        let row = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert!(row.ignored);
        assert!(row.tombstoned_at.is_some());
        assert!(!row.is_pending());
        assert!(matches!(
            upsert_learn_pair_with_scope(&dir, &key, "知呼", "知乎", None).unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::TombstoneOrIgnored)
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learning_receipts_count_distinct_history_rows_once_and_follow_tombstones() {
        let dir = temp_dir("learning-observation-receipts");
        for index in 0..4 {
            insert_history(
                &dir,
                &format!("raw {index}"),
                &format!("final {index}"),
                1.0,
                false,
            )
            .unwrap();
        }
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        let scope = LearnPairScope {
            family: Some("work_chat".into()),
            mapping_id: Some("codex".into()),
            native_bundle: Some("com.openai.codex".into()),
            ..LearnPairScope::default()
        };
        let first_revision = LearnPairObservation::history_edit(1, 11, &key);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &key,
                "知呼",
                "知乎",
                Some(&scope),
                Some(&first_revision),
            )
            .unwrap(),
            LearnPairUpsert::Stored(row) if row.hits == 1
        ));

        // A later revision of the same History row is still only one edit
        // source for this exact pair.
        let same_history = LearnPairObservation::history_edit(1, 12, &key);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &key,
                "知呼",
                "知乎",
                Some(&scope),
                Some(&same_history),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::DuplicateObservation)
        ));
        assert_eq!(get_learn_pair(&dir, &key).unwrap().unwrap().hits, 1);

        let second_history = LearnPairObservation::history_edit(2, 21, &key);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &key,
                "知呼",
                "知乎",
                Some(&scope),
                Some(&second_history),
            )
            .unwrap(),
            LearnPairUpsert::Stored(row) if row.hits == 2
        ));
        let conflict = LearnPairScope {
            native_bundle: Some("com.apple.TextEdit".into()),
            ..scope.clone()
        };
        let conflicting_history = LearnPairObservation::history_edit(3, 31, &key);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &key,
                "知呼",
                "知乎",
                Some(&conflict),
                Some(&conflicting_history),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::ScopeConflict)
        ));
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &key,
                "知呼",
                "知乎",
                Some(&conflict),
                Some(&conflicting_history),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::ScopeConflict)
        ));
        assert!(tombstone_learn_pair(&dir, &key).unwrap());
        let after_tombstone = LearnPairObservation::history_edit(4, 41, &key);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &key,
                "知呼",
                "知乎",
                Some(&scope),
                Some(&after_tombstone),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::TombstoneOrIgnored)
        ));
        let row = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert_eq!(row.hits, 2);
        assert!(row.ignored && row.tombstoned_at.is_some());

        delete_history(&dir, 1).unwrap();
        let connection = open_history(&dir).unwrap();
        let remaining_receipts: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM learn_observations WHERE source_history_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining_receipts, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clear_all_data_deletes_style_drafts() {
        let dir = temp_dir("style-drafts-clear");
        upsert_style_draft(&dir, "wechat", "fewer_periods", "你好。", "你好。", "你好").unwrap();
        assert!(!list_style_drafts(&dir).unwrap().is_empty());
        clear_all_data(&dir).unwrap();
        assert!(list_style_drafts(&dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learn_pair_schema_adds_scope_columns_on_existing_db() {
        let dir = temp_dir("learn-pairs-alter");
        std::fs::create_dir_all(&dir).unwrap();
        let connection = rusqlite::Connection::open(dir.join("history.sqlite")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE learn_pairs (
                    pair_key TEXT PRIMARY KEY,
                    before_surface TEXT NOT NULL,
                    after_surface TEXT NOT NULL,
                    hits INTEGER NOT NULL,
                    promoted INTEGER NOT NULL DEFAULT 0,
                    last_at TEXT NOT NULL
                );
                PRAGMA user_version = 6;",
            )
            .unwrap();
        drop(connection);
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        let row = upsert_learn_pair_with_scope(
            &dir,
            &key,
            "知呼",
            "知乎",
            Some(&LearnPairScope {
                family: Some("personal_chat".into()),
                mapping_id: Some("wechat".into()),
                browser_host: None,
                native_bundle: Some("com.tencent.xinWeChat".into()),
            }),
        )
        .unwrap();
        let LearnPairUpsert::Stored(row) = row else {
            panic!("fresh migration fixture was unexpectedly suppressed")
        };
        assert_eq!(row.family.as_deref(), Some("personal_chat"));
        assert_eq!(row.mapping_id.as_deref(), Some("wechat"));
        assert!(!row.pinned);
        assert!(!row.ignored);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_scene_and_confirm_keep_wechat_snapshot_scope() {
        let dir = temp_dir("history-wechat-scope");
        let mut snapshot = crate::context::ContextSnapshot::general();
        snapshot.profile.id = "chat.personal".into();
        snapshot.profile.family = crate::context::ContextFamily::PersonalChat;
        snapshot.target_guard.bundle_id = Some("com.tencent.xinWeChat".into());
        snapshot.target_guard.browser_host = None;
        insert_history_with_context(&dir, "知呼", "知乎", 1.0, false, &snapshot).unwrap();
        let id = get_history_page(&dir, 1, None, None).unwrap().items[0].id;
        let scene = history_scene(&dir, id).unwrap();
        assert_eq!(scene.profile_id.as_deref(), Some("chat.personal"));
        assert_eq!(scene.family.as_deref(), Some("personal_chat"));
        assert_eq!(
            scene.native_bundle.as_deref(),
            Some("com.tencent.xinWeChat")
        );
        assert!(history_context(&dir, id).unwrap().is_some());
        let key = crate::dictionary_learn::pair_key("派森", "Python");
        ensure_learn_pair_promoted(&dir, &key, "派森", "Python", Some(&scene.learn_scope()))
            .unwrap();
        let row = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert!(row.is_live_promoted());
        assert_eq!(row.family.as_deref(), Some("personal_chat"));
        assert_eq!(row.native_bundle.as_deref(), Some("com.tencent.xinWeChat"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn manual_scoped_pair_is_app_bound_and_refuses_scope_collisions() {
        let dir = temp_dir("manual-scoped-pair");
        let key = crate::dictionary_learn::pair_key("L O M", "LLM");
        let scope = LearnPairScope {
            native_bundle: Some("com.openai.codex".into()),
            ..LearnPairScope::default()
        };
        ensure_manual_scoped_learn_pair(&dir, &key, "L O M", "LLM", &scope).unwrap();
        let row = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert!(row.is_live_promoted());
        assert!(row.manual_scoped);
        assert_eq!(row.native_bundle.as_deref(), Some("com.openai.codex"));
        assert_eq!(row.family, None);
        assert_eq!(row.mapping_id, None);
        assert_eq!(row.browser_host, None);

        let observed = LearnPairScope {
            native_bundle: Some("com.openai.codex".into()),
            ..LearnPairScope::default()
        };
        assert_eq!(observed.assess_record(&row), LearnPairScopeMatch::Match);
        let foreign = LearnPairScope {
            native_bundle: Some("com.apple.TextEdit".into()),
            ..LearnPairScope::default()
        };
        assert_eq!(foreign.assess_record(&row), LearnPairScopeMatch::Mismatch);
        let unknown = LearnPairScope::default();
        assert_eq!(unknown.assess_record(&row), LearnPairScopeMatch::Unknown);

        let conflicting_scope = LearnPairScope {
            native_bundle: Some("com.apple.TextEdit".into()),
            ..LearnPairScope::default()
        };
        assert!(
            ensure_manual_scoped_learn_pair(&dir, &key, "L O M", "LLM", &conflicting_scope)
                .is_err()
        );
        assert!(ensure_manual_scoped_learn_pair(
            &dir,
            &key,
            "L O M",
            "LLM",
            &LearnPairScope::default()
        )
        .is_err());
        assert_eq!(
            get_learn_pair(&dir, &key).unwrap().unwrap().native_bundle,
            scope.native_bundle
        );
        assert!(tombstone_learn_pair(&dir, &key).unwrap());
        let rebound =
            ensure_manual_scoped_learn_pair(&dir, &key, "L O M", "LLM", &conflicting_scope)
                .unwrap();
        assert!(rebound.is_live_promoted());
        assert_eq!(rebound.native_bundle.as_deref(), Some("com.apple.TextEdit"));
        assert_eq!(rebound.hits, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_and_revision_policies_keep_only_safe_metadata() {
        let dir = temp_dir("history-context-source-safety");
        let mut snapshot = crate::context::ContextSnapshot::general();
        snapshot.policy.style_examples_approved = true;
        snapshot.policy.style_example_input = Some("private style input".into());
        snapshot.policy.style_example_output = Some("private style output".into());
        snapshot.policy.style_example_pairs = vec![crate::context::StyleExamplePair {
            input: "private pair input".into(),
            output: "private pair output".into(),
        }];
        snapshot
            .evidence
            .items
            .push(crate::screen_text::ContextEvidenceItem {
                source: crate::screen_text::ContextEvidenceSource::Ax,
                kind: crate::screen_text::ContextEvidenceKind::NearbyText,
                value: "private live window excerpt".into(),
                confidence_milli: Some(900),
                truncated: false,
            });

        insert_history_with_context(&dir, "spoken", "final", 1.0, false, &snapshot).unwrap();
        let stored = history_scene(&dir, 1).unwrap().policy.unwrap();
        assert!(!stored.style_examples_approved);
        assert!(stored.style_example_input.is_none());
        assert!(stored.style_example_output.is_none());
        assert!(stored.style_example_pairs.is_empty());

        save_history_revision(
            &dir,
            1,
            "revised",
            None,
            None,
            None,
            Some(&snapshot.policy),
            "manual_edit",
        )
        .unwrap();
        let revision = get_history_revisions(&dir, 1).unwrap().remove(0);
        let serialized = revision.context_policy.unwrap().to_string();
        assert!(!serialized.contains("private style"));
        assert!(!serialized.contains("private pair"));
        assert!(!serialized.contains("private live window"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unsupported_spool_manifest_is_removed_before_it_consumes_quota() {
        let dir = temp_dir("unsupported-spool");
        let session = dir.join("spool").join("unsupported");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(
            session.join("manifest.json"),
            r#"{"schema_version":999,"session_id":"unsupported","created_at_ms":0,"status":"active","chunks":[]}"#,
        )
        .unwrap();

        assert!(recover_spool(&dir, 7).unwrap().is_empty());
        assert!(!session.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn success_gold_audio_is_playable_but_not_retryable() {
        let dir = temp_dir("gold-success");
        let path = write_gold_file(&dir, "12.wav", b"RIFF").unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw spoken",
            "Cleaned text.",
            1.0,
            false,
            None,
            "ok",
            "paste",
            None,
            &context,
            Some(&path),
        )
        .unwrap();
        let item = get_history(&dir, 1).unwrap().remove(0);
        assert!(item.has_audio);
        assert!(!item.retryable);
        assert!(!item.verbatim_reviewed);
        assert_eq!(item.verbatim_text, None);
        assert!(failed_spool(&dir, item.id).unwrap().is_none());
        assert_eq!(history_audio_bytes(&dir, item.id).unwrap(), b"RIFF");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_gold_file_rejects_parent_dir_escape() {
        let dir = temp_dir("gold-escape");
        assert!(write_gold_file(&dir, "../escape.wav", b"nope").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_verbatim_marks_reviewed_and_can_unreview() {
        let dir = temp_dir("verbatim-review");
        insert_history(&dir, "draft words", "Cleaned.", 1.0, false).unwrap();
        let id = get_history(&dir, 1).unwrap()[0].id;
        save_verbatim(&dir, id, "  mouth words  ", true).unwrap();
        let item = get_history(&dir, 1).unwrap().remove(0);
        assert_eq!(item.verbatim_text.as_deref(), Some("mouth words"));
        assert!(item.verbatim_reviewed);
        save_verbatim(&dir, id, "", false).unwrap();
        let item = get_history(&dir, 1).unwrap().remove(0);
        assert_eq!(item.verbatim_text, None);
        assert!(!item.verbatim_reviewed);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn purge_and_delete_remove_gold_files() {
        let dir = temp_dir("gold-purge");
        let path = write_gold_file(&dir, "kept.wav", b"gold").unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "final",
            1.0,
            false,
            None,
            "ok",
            "paste",
            None,
            &context,
            Some(&path),
        )
        .unwrap();
        let id = get_history(&dir, 1).unwrap()[0].id;
        assert_eq!(purge_gold_audio(&dir, 0).unwrap(), 1);
        assert!(!path.exists());
        assert!(!get_history(&dir, 1).unwrap()[0].has_audio);

        let path = write_gold_file(&dir, "again.wav", b"gold").unwrap();
        let c = open_history(&dir).unwrap();
        c.execute(
            "UPDATE dictations SET raw_audio_path=? WHERE id=?",
            params![path.to_string_lossy(), id],
        )
        .unwrap();
        drop(c);
        delete_history(&dir, id).unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn export_gold_corpus_exports_gold_audio_with_raw_text() {
        let dir = temp_dir("gold-export");
        let downloads = dir.join("downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        let gold = write_gold_file(&dir, "kept.wav", b"RIFFGOLD").unwrap();
        let spool = write_spool_file(&dir, Path::new("failed-short.wav"), b"SPOOL").unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "spoken words",
            "Cleaned words.",
            1.0,
            false,
            None,
            "ok",
            "paste",
            None,
            &context,
            Some(&gold),
        )
        .unwrap();
        insert_history_with_delivery_and_spool(
            &dir,
            "failed raw",
            "",
            1.0,
            false,
            Some("asr_failed"),
            "failed",
            "none",
            None,
            &context,
            Some(&spool),
        )
        .unwrap();

        #[cfg(unix)]
        let downloads_mode = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&downloads).unwrap().permissions().mode() & 0o777
        };
        let exported = export_gold_corpus(&dir, &downloads, "zh").unwrap();
        assert_eq!(exported.count, 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&downloads).unwrap().permissions().mode() & 0o777,
                downloads_mode
            );
            assert_eq!(
                std::fs::metadata(&exported.directory)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        let wav_path = Path::new(&exported.directory).join("utt0001.wav");
        let wav = std::fs::read(&wav_path).unwrap();
        assert_eq!(wav, b"RIFFGOLD");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&wav_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&exported.jsonl_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let jsonl = std::fs::read_to_string(&exported.jsonl_path).unwrap();
        assert!(jsonl.contains("language Chinese<asr_text>spoken words"));
        assert!(!jsonl.contains("Cleaned words"));
        assert!(!jsonl.contains("failed raw"));
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod handy_settings_tests {
    use super::*;
    #[test]
    fn translation_shortcut_defaults_off_and_round_trips_in_public_view() {
        let mut settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(settings.translation_hotkey.is_empty());
        settings.translation_hotkey = "Command+Shift+T".into();
        settings.translation_target_language = "ja".into();
        settings.normalize();
        let restored: Settings =
            serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
        assert_eq!(restored.translation_hotkey, "Command+Shift+T");
        let view = serde_json::to_value(SettingsView::from(&restored)).unwrap();
        assert_eq!(view["translation_hotkey"], "Command+Shift+T");
        assert_eq!(view["translation_target_language"], "ja");
    }

    #[test]
    fn translation_shortcut_rejects_alias_conflicts_and_accepts_fn() {
        for field in ["hotkey", "selected", "screen", "verbatim"] {
            let mut settings = Settings {
                translation_hotkey: "Cmd+Shift+T".into(),
                ..Settings::default()
            };
            match field {
                "hotkey" => settings.hotkey = "Command+Shift+T".into(),
                "selected" => settings.selected_action_hotkey = "Command+Shift+T".into(),
                "screen" => settings.screen_action_hotkey = "Command+Shift+T".into(),
                _ => settings.verbatim_hotkey = "Command+Shift+T".into(),
            }
            assert!(settings
                .validate()
                .unwrap_err()
                .to_string()
                .contains("hotkey conflicts"));
        }
        let settings = Settings {
            translation_hotkey: "Fn".into(),
            ..Settings::default()
        };
        settings.validate().unwrap();
        let mut fn_conflict = settings.clone();
        fn_conflict.hotkey = "Globe".into();
        assert!(fn_conflict.validate().is_err());
    }
    #[test]
    fn missing_legacy_mode_deserializes_as_tap() {
        let legacy: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.activation_mode, "tap");
    }
    #[test]
    fn conservative_settings_round_trip_and_clamp() {
        let mut settings = Settings {
            extra_recording_buffer_ms: 9000,
            audio_feedback_volume: f32::NAN,
            verbatim_hotkey: "Cmd+Shift+V".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.extra_recording_buffer_ms, 2000);
        assert_eq!(settings.audio_feedback_volume, 0.6);
        assert!(
            !settings.vad_enabled
                && !settings.audio_feedback_enabled
                && !settings.fuzzy_dictionary_enabled
                && !settings.always_on_microphone
                && !settings.autostart_enabled
                && !settings.debug_mode
        );
        let encoded = serde_json::to_value(&settings).unwrap();
        let restored: Settings = serde_json::from_value(encoded).unwrap();
        assert_eq!(restored.verbatim_hotkey, settings.verbatim_hotkey);
        let view = serde_json::to_value(SettingsView::from(&restored)).unwrap();
        for key in [
            "verbatim_hotkey",
            "extra_recording_buffer_ms",
            "audio_feedback_enabled",
            "audio_feedback_volume",
            "vad_enabled",
            "fuzzy_dictionary_enabled",
            "always_on_microphone",
            "clamshell_microphone",
            "autostart_enabled",
            "whats_new_last_seen_version",
            "debug_mode",
        ] {
            assert!(view.get(key).is_some(), "{key}");
        }
    }
}

#[cfg(test)]
mod guide_acknowledgement_fixtures {
    use super::*;
    #[test]
    fn old_settings_and_independent_explicit_ack_round_trip() {
        let mut settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(
            settings.guide_acknowledgements,
            GuideAcknowledgements::default()
        );
        assert!(settings
            .guide_acknowledgements
            .acknowledge(OnDemandGuideId::ContextScreen, 1)
            .unwrap());
        assert!(!settings
            .guide_acknowledgements
            .acknowledge(OnDemandGuideId::ContextScreen, 1)
            .unwrap());
        assert_eq!(settings.guide_acknowledgements.dictionary_learning, None);
        assert_eq!(settings.guide_acknowledgements.context_screen, Some(1));
        let saved: Settings =
            serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
        assert_eq!(
            saved.guide_acknowledgements,
            settings.guide_acknowledgements
        );
        assert!(saved
            .guide_acknowledgements
            .clone()
            .acknowledge(OnDemandGuideId::ContextScreen, 2)
            .is_err());
        assert!(serde_json::from_str::<OnDemandGuideId>(r#""unknown_guide""#).is_err());
    }
    #[test]
    fn teaching_ack_does_not_become_recording_configuration_or_feature_success() {
        let before = Settings::default();
        let mut after = before.clone();
        after
            .guide_acknowledgements
            .acknowledge(OnDemandGuideId::SelectedTranslation, 1)
            .unwrap();
        assert!(!crate::dictation_configuration_changed(&before, &after));
        assert_eq!(before.onboarded, after.onboarded);
        assert_eq!(
            before.dictionary_learn_enabled,
            after.dictionary_learn_enabled
        );
        after.asr_model.push_str("-changed");
        assert!(crate::dictation_configuration_changed(&before, &after));
    }
}
