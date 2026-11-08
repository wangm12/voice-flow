//! Ownership, observation, and conservative admission for VoiceFlow's two model sidecars.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};

const CAP_NUMERATOR: u128 = 4;
const CAP_DENOMINATOR: u128 = 5;
const ASR_COLD_RESERVE_BYTES: u64 = 2_415_919_104;
const CLEANUP_COLD_RESERVE_BYTES: u64 = 27_648_851_968;
const ASR_MODEL_ID: &str = "qwen3-asr-0.6b";
const ASR_MODEL_REVISION: &str = "eae2b51f96265328f1e7beced788adb0e4536f92";
const CLEANUP_MODEL_ID: &str = "qwen3_8_27b_mlx_6bit";
const CLEANUP_MODEL_REVISION: &str = "05f38c1f39e2f39619653616fe8e96a5d676358e";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnedChildIdentity {
    pub(crate) pid: u32,
    pub(crate) generation: u64,
    pub(crate) start_abstime: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FootprintSample {
    Observed(u64),
    Unknown,
    IdentityMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostMemoryPressure {
    Normal,
    Warning,
    Critical,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadKind {
    Asr,
    Cleanup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryUnavailableReason {
    CriticalPressure,
    UnknownPressure,
    PressureNotNormal,
    UnknownPhysicalMemory,
    UnknownChildFootprint,
    ChildIdentityChanged,
    SnapshotChanged,
    BudgetExceeded,
    ArithmeticOverflow,
    UncalibratedModel,
}

impl MemoryUnavailableReason {
    pub(crate) fn diagnostic_code(self) -> &'static str {
        match self {
            Self::CriticalPressure => "critical_pressure",
            Self::UnknownPressure => "unknown_pressure",
            Self::PressureNotNormal => "pressure_not_normal",
            Self::UnknownPhysicalMemory => "physical_memory_unknown",
            Self::UnknownChildFootprint => "child_footprint_unknown",
            Self::ChildIdentityChanged => "child_identity_changed",
            Self::SnapshotChanged => "snapshot_changed",
            Self::BudgetExceeded => "budget_exceeded",
            Self::ArithmeticOverflow => "arithmetic_overflow",
            Self::UncalibratedModel => "uncalibrated_model",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingLoad {
    nonce: u64,
    target_bytes: Option<u64>,
    identity: OwnedChildIdentity,
}

#[derive(Clone)]
struct ChildRecord {
    identity: OwnedChildIdentity,
    models_root: PathBuf,
    pending_loads: Vec<PendingLoad>,
    sampler: Arc<dyn MemorySampler>,
}

trait MemorySampler: Send + Sync {
    fn pressure(&self) -> HostMemoryPressure;
    fn physical_memory(&self) -> Option<u64>;
    fn footprint(&self, identity: &OwnedChildIdentity) -> FootprintSample;
}

#[cfg(not(test))]
struct SystemMemorySampler;
#[cfg(not(test))]
impl MemorySampler for SystemMemorySampler {
    fn pressure(&self) -> HostMemoryPressure {
        host_memory_pressure()
    }
    fn physical_memory(&self) -> Option<u64> {
        physical_memory_bytes()
    }
    fn footprint(&self, identity: &OwnedChildIdentity) -> FootprintSample {
        process_footprint(identity)
    }
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct FixedMemorySampler {
    pub(crate) pressure: HostMemoryPressure,
    pub(crate) physical_memory: Option<u64>,
    pub(crate) footprint: FootprintSample,
}
#[cfg(test)]
impl MemorySampler for FixedMemorySampler {
    fn pressure(&self) -> HostMemoryPressure {
        self.pressure
    }
    fn physical_memory(&self) -> Option<u64> {
        self.physical_memory
    }
    fn footprint(&self, _identity: &OwnedChildIdentity) -> FootprintSample {
        self.footprint
    }
}

#[derive(Default)]
struct Registry {
    next_key: u64,
    next_nonce: u64,
    epochs: HashMap<PathBuf, u64>,
    children: HashMap<u64, ChildRecord>,
}

fn snapshot_still_current(
    registry: &Registry,
    root: &Path,
    epoch: u64,
    identities: &[(u64, OwnedChildIdentity)],
) -> bool {
    registry.epochs.get(root).copied().unwrap_or_default() == epoch
        && identities.iter().all(|(key, identity)| {
            registry
                .children
                .get(key)
                .is_some_and(|record| record.identity == *identity)
        })
}

fn next_root_epoch(registry: &Registry, root: &Path) -> Option<u64> {
    registry
        .epochs
        .get(root)
        .copied()
        .unwrap_or_default()
        .checked_add(1)
}

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

#[cfg(not(test))]
fn default_sampler() -> Arc<dyn MemorySampler> {
    Arc::new(SystemMemorySampler)
}
#[cfg(test)]
fn default_sampler() -> Arc<dyn MemorySampler> {
    Arc::new(FixedMemorySampler {
        pressure: HostMemoryPressure::Normal,
        physical_memory: Some(64 * 1024 * 1024 * 1024),
        footprint: FootprintSample::Observed(0),
    })
}

#[derive(Clone)]
pub(crate) struct ChildRegistration {
    key: u64,
}

impl ChildRegistration {
    pub(crate) fn register(pid: u32, generation: u64, models_root: &Path) -> Option<Self> {
        if pid == 0 {
            return None;
        }
        let start_abstime = process_start_abstime(pid);
        let models_root = normalized_root(models_root);
        let mut registry = registry().lock().unwrap_or_else(|p| p.into_inner());
        let next_epoch = next_root_epoch(&registry, &models_root)?;
        registry.next_key = registry.next_key.checked_add(1)?;
        let key = registry.next_key;
        registry.children.insert(
            key,
            ChildRecord {
                identity: OwnedChildIdentity {
                    pid,
                    generation,
                    start_abstime,
                },
                models_root: models_root.clone(),
                pending_loads: Vec::new(),
                sampler: default_sampler(),
            },
        );
        registry.epochs.insert(models_root, next_epoch);
        Some(Self { key })
    }

    #[cfg(test)]
    pub(crate) fn register_with_sampler(
        pid: u32,
        generation: u64,
        models_root: &Path,
        sampler: FixedMemorySampler,
    ) -> Option<Self> {
        Self::register_with_memory_sampler(pid, generation, models_root, Arc::new(sampler))
    }

    #[cfg(test)]
    fn register_with_memory_sampler(
        pid: u32,
        generation: u64,
        models_root: &Path,
        sampler: Arc<dyn MemorySampler>,
    ) -> Option<Self> {
        if pid == 0 {
            return None;
        }
        let start_abstime = process_start_abstime(pid);
        let models_root = normalized_root(models_root);
        let mut registry = registry().lock().unwrap_or_else(|p| p.into_inner());
        let next_epoch = next_root_epoch(&registry, &models_root)?;
        registry.next_key = registry.next_key.checked_add(1)?;
        let key = registry.next_key;
        registry.children.insert(
            key,
            ChildRecord {
                identity: OwnedChildIdentity {
                    pid,
                    generation,
                    start_abstime,
                },
                models_root: models_root.clone(),
                pending_loads: Vec::new(),
                sampler,
            },
        );
        registry.epochs.insert(models_root, next_epoch);
        Some(Self { key })
    }

    #[cfg(test)]
    pub(crate) fn set_test_sampler(&self, sampler: FixedMemorySampler) {
        let mut registry = registry().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(record) = registry.children.get_mut(&self.key) {
            record.sampler = Arc::new(sampler);
        }
    }

    pub(crate) fn pressure(&self) -> HostMemoryPressure {
        let sampler = registry()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .children
            .get(&self.key)
            .map(|record| Arc::clone(&record.sampler));
        sampler.map_or(HostMemoryPressure::Unknown, |sampler| sampler.pressure())
    }

    /// Sample all current children and atomically reserve this cold load at the observed epoch.
    pub(crate) fn admit_cold_load(
        &self,
        model_id: &str,
        model_revision: &str,
        kind: LoadKind,
    ) -> Result<PendingLoadReservation, MemoryUnavailableReason> {
        let reserve = cold_reserve(model_id, model_revision, kind)?;
        let (epoch, records) = {
            let registry = registry().lock().unwrap_or_else(|p| p.into_inner());
            let own = registry
                .children
                .get(&self.key)
                .ok_or(MemoryUnavailableReason::ChildIdentityChanged)?;
            let epoch = registry
                .epochs
                .get(&own.models_root)
                .copied()
                .unwrap_or_default();
            if epoch == u64::MAX {
                return Err(MemoryUnavailableReason::ArithmeticOverflow);
            }
            let root = own.models_root.clone();
            let records = registry
                .children
                .iter()
                .filter(|(_, record)| record.models_root == root)
                .map(|(key, record)| (*key, record.clone()))
                .collect::<Vec<_>>();
            (epoch, records)
        };
        let own_sampler = records
            .iter()
            .find(|(key, _)| *key == self.key)
            .map(|(_, record)| Arc::clone(&record.sampler))
            .ok_or(MemoryUnavailableReason::ChildIdentityChanged)?;
        let pressure = own_sampler.pressure();
        match pressure {
            HostMemoryPressure::Critical => return Err(MemoryUnavailableReason::CriticalPressure),
            HostMemoryPressure::Unknown => return Err(MemoryUnavailableReason::UnknownPressure),
            HostMemoryPressure::Warning if kind == LoadKind::Cleanup => {
                return Err(MemoryUnavailableReason::PressureNotNormal)
            }
            _ => {}
        }
        let physical = own_sampler
            .physical_memory()
            .ok_or(MemoryUnavailableReason::UnknownPhysicalMemory)?;
        let mut sampled = Vec::with_capacity(records.len());
        let mut total = 0u64;
        let mut own_footprint = None;
        for (key, record) in records {
            let footprint = record.sampler.footprint(&record.identity);
            let bytes = match footprint {
                FootprintSample::Observed(bytes) => bytes,
                FootprintSample::Unknown => {
                    return Err(MemoryUnavailableReason::UnknownChildFootprint)
                }
                FootprintSample::IdentityMismatch => {
                    return Err(MemoryUnavailableReason::ChildIdentityChanged)
                }
            };
            if key == self.key {
                own_footprint = Some(bytes);
            }
            let pending_target =
                record
                    .pending_loads
                    .iter()
                    .try_fold(None, |max: Option<u64>, pending| {
                        let target = pending
                            .target_bytes
                            .ok_or(MemoryUnavailableReason::UnknownChildFootprint)?;
                        Ok::<_, MemoryUnavailableReason>(Some(
                            max.map_or(target, |current| current.max(target)),
                        ))
                    })?;
            let child_cost = pending_target.map_or(bytes, |target| bytes.max(target));
            total = total
                .checked_add(child_cost)
                .ok_or(MemoryUnavailableReason::ArithmeticOverflow)?;
            sampled.push((key, record.identity, bytes));
        }
        let own_footprint = own_footprint.ok_or(MemoryUnavailableReason::ChildIdentityChanged)?;
        let target = own_footprint
            .checked_add(reserve)
            .ok_or(MemoryUnavailableReason::ArithmeticOverflow)?;
        let cap = memory_cap_bytes(physical).ok_or(MemoryUnavailableReason::ArithmeticOverflow)?;
        let projected = projected_cost(total, own_footprint, target)
            .ok_or(MemoryUnavailableReason::ArithmeticOverflow)?;
        if projected > cap {
            return Err(MemoryUnavailableReason::BudgetExceeded);
        }

        let mut registry = registry().lock().unwrap_or_else(|p| p.into_inner());
        let sampled_identities = sampled
            .iter()
            .map(|(key, identity, _)| (*key, identity.clone()))
            .collect::<Vec<_>>();
        let own_root = registry
            .children
            .get(&self.key)
            .ok_or(MemoryUnavailableReason::ChildIdentityChanged)?
            .models_root
            .clone();
        if !snapshot_still_current(&registry, &own_root, epoch, &sampled_identities) {
            return Err(MemoryUnavailableReason::SnapshotChanged);
        }
        let identity = registry
            .children
            .get(&self.key)
            .ok_or(MemoryUnavailableReason::ChildIdentityChanged)?
            .identity
            .clone();
        if !registry
            .children
            .get(&self.key)
            .ok_or(MemoryUnavailableReason::ChildIdentityChanged)?
            .pending_loads
            .is_empty()
        {
            return Err(MemoryUnavailableReason::SnapshotChanged);
        }
        let next_epoch = next_root_epoch(&registry, &own_root)
            .ok_or(MemoryUnavailableReason::ArithmeticOverflow)?;
        let next_nonce = registry
            .next_nonce
            .checked_add(1)
            .ok_or(MemoryUnavailableReason::ArithmeticOverflow)?;
        registry.next_nonce = next_nonce.max(1);
        let nonce = registry.next_nonce;
        let record = registry
            .children
            .get_mut(&self.key)
            .ok_or(MemoryUnavailableReason::ChildIdentityChanged)?;
        record.pending_loads.push(PendingLoad {
            nonce,
            target_bytes: Some(target),
            identity: identity.clone(),
        });
        registry.epochs.insert(own_root, next_epoch);
        Ok(PendingLoadReservation {
            key: self.key,
            nonce,
            identity,
        })
    }

    /// Unbudgeted registrations exist only for protocol fixtures; production loads use admit_cold_load.
    #[cfg(test)]
    pub(crate) fn reserve_pending_load(&self) -> PendingLoadReservation {
        let mut registry = registry().lock().unwrap_or_else(|p| p.into_inner());
        registry.next_nonce = registry.next_nonce.wrapping_add(1).max(1);
        let nonce = registry.next_nonce;
        let record = registry
            .children
            .get_mut(&self.key)
            .expect("registered child");
        let identity = record.identity.clone();
        let root = record.models_root.clone();
        record.pending_loads.push(PendingLoad {
            nonce,
            target_bytes: None,
            identity: identity.clone(),
        });
        let next_epoch = next_root_epoch(&registry, &root).unwrap_or(u64::MAX);
        registry.epochs.insert(root, next_epoch);
        PendingLoadReservation {
            key: self.key,
            nonce,
            identity,
        }
    }

    pub(crate) fn retire_after_exit(self) {
        let mut registry = registry().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(record) = registry.children.remove(&self.key) {
            let next_epoch = next_root_epoch(&registry, &record.models_root).unwrap_or(u64::MAX);
            registry.epochs.insert(record.models_root, next_epoch);
        }
    }
}

pub(crate) struct PendingLoadReservation {
    key: u64,
    nonce: u64,
    identity: OwnedChildIdentity,
}
impl PendingLoadReservation {
    pub(crate) fn settle(self) {
        let mut registry = registry().lock().unwrap_or_else(|p| p.into_inner());
        let Some(record) = registry.children.get_mut(&self.key) else {
            return;
        };
        if record.identity != self.identity {
            return;
        }
        let root = record.models_root.clone();
        if let Some(index) = record
            .pending_loads
            .iter()
            .position(|pending| pending.nonce == self.nonce && pending.identity == self.identity)
        {
            record.pending_loads.remove(index);
            let next_epoch = next_root_epoch(&registry, &root).unwrap_or(u64::MAX);
            registry.epochs.insert(root, next_epoch);
        }
    }
}

#[cfg(test)]
pub(crate) fn owned_children_for_root(
    root: &Path,
) -> Vec<(OwnedChildIdentity, FootprintSample, bool)> {
    let root = normalized_root(root);
    let records = registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .children
        .values()
        .filter(|record| record.models_root == root)
        .cloned()
        .collect::<Vec<_>>();
    records
        .into_iter()
        .map(|record| {
            let footprint = record.sampler.footprint(&record.identity);
            (record.identity, footprint, !record.pending_loads.is_empty())
        })
        .collect()
}

fn cold_reserve(id: &str, revision: &str, kind: LoadKind) -> Result<u64, MemoryUnavailableReason> {
    match (kind, id, revision) {
        (LoadKind::Asr, ASR_MODEL_ID, ASR_MODEL_REVISION) => Ok(ASR_COLD_RESERVE_BYTES),
        (LoadKind::Cleanup, CLEANUP_MODEL_ID, CLEANUP_MODEL_REVISION) => {
            Ok(CLEANUP_COLD_RESERVE_BYTES)
        }
        _ => Err(MemoryUnavailableReason::UncalibratedModel),
    }
}

fn memory_cap_bytes(physical: u64) -> Option<u64> {
    u64::try_from((physical as u128).checked_mul(CAP_NUMERATOR)? / CAP_DENOMINATOR).ok()
}

fn projected_cost(total: u64, current: u64, target: u64) -> Option<u64> {
    total.checked_sub(current)?.checked_add(current.max(target))
}

fn decode_pressure(value: Option<(u32, usize)>) -> HostMemoryPressure {
    match value {
        Some((1, 4)) => HostMemoryPressure::Normal,
        Some((2, 4)) => HostMemoryPressure::Warning,
        Some((4, 4)) => HostMemoryPressure::Critical,
        _ => HostMemoryPressure::Unknown,
    }
}

fn decode_physical_memory(value: Option<(u64, usize)>) -> Option<u64> {
    match value {
        Some((bytes, len)) if len == std::mem::size_of::<u64>() && bytes != 0 => Some(bytes),
        _ => None,
    }
}

fn normalized_root(root: &Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}

#[cfg(all(target_os = "macos", not(test)))]
fn sysctl_u32(name: &'static [u8]) -> Option<u32> {
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>();
    let result = unsafe {
        libc::sysctlbyname(
            name.as_ptr().cast(),
            (&mut value as *mut u32).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (result == 0 && size == std::mem::size_of::<u32>()).then_some(value)
}

#[cfg(all(target_os = "macos", not(test)))]
pub(crate) fn host_memory_pressure() -> HostMemoryPressure {
    decode_pressure(sysctl_u32(b"kern.memorystatus_vm_pressure_level\0").map(|value| (value, 4)))
}
#[cfg(all(not(target_os = "macos"), not(test)))]
pub(crate) fn host_memory_pressure() -> HostMemoryPressure {
    HostMemoryPressure::Unknown
}

#[cfg(all(target_os = "macos", not(test)))]
pub(crate) fn physical_memory_bytes() -> Option<u64> {
    let mut value = 0u64;
    let mut size = std::mem::size_of::<u64>();
    let result = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&mut value as *mut u64).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    decode_physical_memory((result == 0).then_some((value, size)))
}
#[cfg(all(not(target_os = "macos"), not(test)))]
pub(crate) fn physical_memory_bytes() -> Option<u64> {
    None
}

#[cfg(target_os = "macos")]
fn process_start_abstime(pid: u32) -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
    let result = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V2,
            usage.as_mut_ptr().cast(),
        )
    };
    if result != 0 {
        return None;
    }
    Some(unsafe { usage.assume_init() }.ri_proc_start_abstime)
}
#[cfg(not(target_os = "macos"))]
fn process_start_abstime(_pid: u32) -> Option<u64> {
    None
}

#[cfg(all(target_os = "macos", not(test)))]
fn process_footprint(identity: &OwnedChildIdentity) -> FootprintSample {
    let Some(expected_start) = identity.start_abstime else {
        return FootprintSample::Unknown;
    };
    let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
    let result = unsafe {
        libc::proc_pid_rusage(
            identity.pid as libc::c_int,
            libc::RUSAGE_INFO_V2,
            usage.as_mut_ptr().cast(),
        )
    };
    if result != 0 {
        return FootprintSample::Unknown;
    }
    let usage = unsafe { usage.assume_init() };
    if usage.ri_proc_start_abstime != expected_start {
        return FootprintSample::IdentityMismatch;
    }
    FootprintSample::Observed(usage.ri_phys_footprint)
}
#[cfg(all(not(target_os = "macos"), not(test)))]
fn process_footprint(_identity: &OwnedChildIdentity) -> FootprintSample {
    FootprintSample::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Barrier};
    fn test_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("vf-model-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    fn sampler(
        pressure: HostMemoryPressure,
        physical_memory: Option<u64>,
        footprint: FootprintSample,
    ) -> FixedMemorySampler {
        FixedMemorySampler {
            pressure,
            physical_memory,
            footprint,
        }
    }

    struct AdmissionBarrierSampler {
        barrier: Arc<Barrier>,
        waited: AtomicBool,
    }

    impl MemorySampler for AdmissionBarrierSampler {
        fn pressure(&self) -> HostMemoryPressure {
            HostMemoryPressure::Normal
        }
        fn physical_memory(&self) -> Option<u64> {
            Some(36 * 1024 * 1024 * 1024)
        }
        fn footprint(&self, _identity: &OwnedChildIdentity) -> FootprintSample {
            if !self.waited.swap(true, Ordering::SeqCst) {
                self.barrier.wait();
            }
            FootprintSample::Observed(1024 * 1024 * 1024)
        }
    }

    struct PausedMemorySampler {
        entered: mpsc::Sender<()>,
        resume: Mutex<mpsc::Receiver<()>>,
        paused: AtomicBool,
    }

    impl MemorySampler for PausedMemorySampler {
        fn pressure(&self) -> HostMemoryPressure {
            HostMemoryPressure::Normal
        }
        fn physical_memory(&self) -> Option<u64> {
            Some(36 * 1024 * 1024 * 1024)
        }
        fn footprint(&self, _identity: &OwnedChildIdentity) -> FootprintSample {
            if !self.paused.swap(true, Ordering::SeqCst) {
                self.entered.send(()).unwrap();
                self.resume.lock().unwrap().recv().unwrap();
            }
            FootprintSample::Observed(1024 * 1024 * 1024)
        }
    }

    #[derive(Clone, Copy)]
    enum SnapshotMutation {
        Register,
        Settle,
        ActualExit,
    }

    fn admission_rejects_snapshot_after_registry_mutation(mutation: SnapshotMutation) {
        let root = test_root("admission-snapshot-mutation");
        let other = ChildRegistration::register_with_sampler(
            std::process::id(),
            121,
            &root,
            sampler(
                HostMemoryPressure::Normal,
                Some(36 * 1024 * 1024 * 1024),
                FootprintSample::Observed(1024 * 1024 * 1024),
            ),
        )
        .unwrap();
        let settle_token = if matches!(mutation, SnapshotMutation::Settle) {
            Some(
                other
                    .admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
                    .unwrap(),
            )
        } else {
            None
        };
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let waiting = ChildRegistration::register_with_memory_sampler(
            std::process::id(),
            122,
            &root,
            Arc::new(PausedMemorySampler {
                entered: entered_tx,
                resume: Mutex::new(resume_rx),
                paused: AtomicBool::new(false),
            }),
        )
        .unwrap();
        let waiter = std::thread::spawn({
            let waiting = waiting.clone();
            move || waiting.admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
        });
        entered_rx.recv().unwrap();
        let registered = match mutation {
            SnapshotMutation::Register => Some(
                ChildRegistration::register_with_sampler(
                    std::process::id(),
                    123,
                    &root,
                    sampler(
                        HostMemoryPressure::Normal,
                        Some(36 * 1024 * 1024 * 1024),
                        FootprintSample::Observed(1024 * 1024 * 1024),
                    ),
                )
                .unwrap(),
            ),
            SnapshotMutation::Settle => {
                settle_token.unwrap().settle();
                None
            }
            SnapshotMutation::ActualExit => {
                other.clone().retire_after_exit();
                None
            }
        };
        resume_tx.send(()).unwrap();
        assert_eq!(
            waiter.join().unwrap().err(),
            Some(MemoryUnavailableReason::SnapshotChanged)
        );
        if let Some(registered) = registered {
            registered.retire_after_exit();
        }
        waiting.retire_after_exit();
        if !matches!(mutation, SnapshotMutation::ActualExit) {
            other.retire_after_exit();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_admission_uses_full_budget_and_pressure_policy() {
        let root = test_root("admission");
        let cleanup = ChildRegistration::register_with_sampler(
            std::process::id(),
            101,
            &root,
            sampler(
                HostMemoryPressure::Normal,
                Some(64 * 1024 * 1024 * 1024),
                FootprintSample::Observed(0),
            ),
        )
        .unwrap();
        let token = cleanup
            .admit_cold_load(CLEANUP_MODEL_ID, CLEANUP_MODEL_REVISION, LoadKind::Cleanup)
            .unwrap();
        token.settle();
        cleanup.retire_after_exit();
        std::fs::remove_dir_all(root).unwrap();

        let root = test_root("admission-32");
        let cleanup = ChildRegistration::register_with_sampler(
            std::process::id(),
            102,
            &root,
            sampler(
                HostMemoryPressure::Normal,
                Some(32 * 1024 * 1024 * 1024),
                FootprintSample::Observed(0),
            ),
        )
        .unwrap();
        assert_eq!(
            cleanup
                .admit_cold_load(CLEANUP_MODEL_ID, CLEANUP_MODEL_REVISION, LoadKind::Cleanup)
                .err(),
            Some(MemoryUnavailableReason::BudgetExceeded)
        );
        cleanup.retire_after_exit();
        std::fs::remove_dir_all(root).unwrap();

        let root = test_root("admission-warning");
        let asr = ChildRegistration::register_with_sampler(
            std::process::id(),
            103,
            &root,
            sampler(
                HostMemoryPressure::Warning,
                Some(64 * 1024 * 1024 * 1024),
                FootprintSample::Observed(0),
            ),
        )
        .unwrap();
        let token = asr
            .admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
            .unwrap();
        token.settle();
        assert_eq!(
            asr.admit_cold_load(CLEANUP_MODEL_ID, CLEANUP_MODEL_REVISION, LoadKind::Cleanup)
                .err(),
            Some(MemoryUnavailableReason::PressureNotNormal)
        );
        asr.retire_after_exit();
        std::fs::remove_dir_all(root).unwrap();

        let root = test_root("admission-unknown-pressure");
        let child = ChildRegistration::register_with_sampler(
            std::process::id(),
            109,
            &root,
            sampler(
                HostMemoryPressure::Unknown,
                Some(64 * 1024 * 1024 * 1024),
                FootprintSample::Observed(0),
            ),
        )
        .unwrap();
        assert_eq!(
            child
                .admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
                .err(),
            Some(MemoryUnavailableReason::UnknownPressure)
        );
        child.retire_after_exit();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_admission_fails_closed_for_unknown_samples_identity_and_unmeasured_profiles() {
        let root = test_root("admission-unknown");
        let unknown_mem = ChildRegistration::register_with_sampler(
            std::process::id(),
            104,
            &root,
            sampler(
                HostMemoryPressure::Normal,
                None,
                FootprintSample::Observed(0),
            ),
        )
        .unwrap();
        assert_eq!(
            unknown_mem
                .admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
                .err(),
            Some(MemoryUnavailableReason::UnknownPhysicalMemory)
        );
        unknown_mem.retire_after_exit();
        std::fs::remove_dir_all(root).unwrap();

        let root = test_root("admission-footprint");
        let unknown_footprint = ChildRegistration::register_with_sampler(
            std::process::id(),
            105,
            &root,
            sampler(
                HostMemoryPressure::Normal,
                Some(64 * 1024 * 1024 * 1024),
                FootprintSample::Unknown,
            ),
        )
        .unwrap();
        assert_eq!(
            unknown_footprint
                .admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
                .err(),
            Some(MemoryUnavailableReason::UnknownChildFootprint)
        );
        unknown_footprint.retire_after_exit();
        std::fs::remove_dir_all(root).unwrap();

        let root = test_root("admission-identity");
        let mismatch = ChildRegistration::register_with_sampler(
            std::process::id(),
            106,
            &root,
            sampler(
                HostMemoryPressure::Normal,
                Some(64 * 1024 * 1024 * 1024),
                FootprintSample::IdentityMismatch,
            ),
        )
        .unwrap();
        assert_eq!(
            mismatch
                .admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
                .err(),
            Some(MemoryUnavailableReason::ChildIdentityChanged)
        );
        mismatch.retire_after_exit();
        std::fs::remove_dir_all(root).unwrap();

        let root = test_root("admission-uncalibrated");
        let child = ChildRegistration::register_with_sampler(
            std::process::id(),
            107,
            &root,
            sampler(
                HostMemoryPressure::Normal,
                Some(64 * 1024 * 1024 * 1024),
                FootprintSample::Observed(0),
            ),
        )
        .unwrap();
        assert_eq!(
            child
                .admit_cold_load("qwen3-asr-1.7b", "unknown", LoadKind::Asr)
                .err(),
            Some(MemoryUnavailableReason::UncalibratedModel)
        );
        child.retire_after_exit();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_admission_accounts_for_every_owned_child_and_checked_overflow() {
        let root = test_root("admission-sum");
        let asr = ChildRegistration::register_with_sampler(
            std::process::id(),
            108,
            &root,
            sampler(
                HostMemoryPressure::Normal,
                Some(36 * 1024 * 1024 * 1024),
                FootprintSample::Observed(1024 * 1024 * 1024),
            ),
        )
        .unwrap();
        let cleanup = ChildRegistration::register_with_sampler(
            std::process::id(),
            109,
            &root,
            sampler(
                HostMemoryPressure::Normal,
                Some(36 * 1024 * 1024 * 1024),
                FootprintSample::Observed(1024 * 1024 * 1024),
            ),
        )
        .unwrap();
        let asr_token = asr
            .admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
            .unwrap();
        assert_eq!(
            cleanup
                .admit_cold_load(CLEANUP_MODEL_ID, CLEANUP_MODEL_REVISION, LoadKind::Cleanup)
                .err(),
            Some(MemoryUnavailableReason::BudgetExceeded)
        );
        asr_token.settle();
        asr.retire_after_exit();
        cleanup.retire_after_exit();
        std::fs::remove_dir_all(root).unwrap();

        let root = test_root("admission-overflow");
        let asr = ChildRegistration::register_with_sampler(
            std::process::id(),
            110,
            &root,
            sampler(
                HostMemoryPressure::Normal,
                Some(64 * 1024 * 1024 * 1024),
                FootprintSample::Observed(u64::MAX),
            ),
        )
        .unwrap();
        assert_eq!(
            asr.admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
                .err(),
            Some(MemoryUnavailableReason::ArithmeticOverflow)
        );
        asr.retire_after_exit();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn profiles_are_exact_and_estimates_stay_bytes() {
        assert_eq!(
            cold_reserve(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr),
            Ok(2_415_919_104)
        );
        assert_eq!(
            cold_reserve(CLEANUP_MODEL_ID, CLEANUP_MODEL_REVISION, LoadKind::Cleanup),
            Ok(27_648_851_968)
        );
        assert_eq!(
            cold_reserve(ASR_MODEL_ID, "wrong", LoadKind::Asr),
            Err(MemoryUnavailableReason::UncalibratedModel)
        );
    }
    #[test]
    fn cap_uses_floor_and_checked_projection() {
        assert_eq!(
            memory_cap_bytes(32u64 * 1024 * 1024 * 1024),
            Some(27_487_790_694)
        );
        let cleanup_reserve =
            cold_reserve(CLEANUP_MODEL_ID, CLEANUP_MODEL_REVISION, LoadKind::Cleanup)
                .expect("cleanup reserve is calibrated");
        let cap_32_gib = memory_cap_bytes(32u64 * 1024 * 1024 * 1024)
            .expect("32 GiB memory cap is representable");
        assert_eq!(cleanup_reserve, 27_648_851_968);
        assert_eq!(cap_32_gib, 27_487_790_694);
        assert!(cleanup_reserve > cap_32_gib);
        assert_eq!(
            memory_cap_bytes(64u64 * 1024 * 1024 * 1024),
            Some(54_975_581_388)
        );
        let cap = memory_cap_bytes(64u64 * 1024 * 1024 * 1024).unwrap();
        assert!(projected_cost(cap - 1, cap - 1, cap).unwrap() <= cap);
        assert!(projected_cost(cap, cap, cap + 1).unwrap() > cap);
        assert_eq!(projected_cost(u64::MAX, 1, 2), None);
        assert_eq!(projected_cost(1, 2, 3), None);
    }
    #[test]
    fn pending_cost_uses_max_of_current_footprint_and_target_during_real_admission() {
        let root = test_root("pending-cost");
        let asr = ChildRegistration::register_with_sampler(
            std::process::id(),
            111,
            &root,
            sampler(
                HostMemoryPressure::Normal,
                Some(8 * 1024 * 1024 * 1024),
                FootprintSample::Observed(1024 * 1024 * 1024),
            ),
        )
        .unwrap();
        let other = ChildRegistration::register_with_sampler(
            std::process::id(),
            112,
            &root,
            sampler(
                HostMemoryPressure::Normal,
                Some(8 * 1024 * 1024 * 1024),
                FootprintSample::Observed(1024 * 1024 * 1024),
            ),
        )
        .unwrap();
        let pending = asr
            .admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
            .unwrap();
        assert_eq!(
            other
                .admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
                .err(),
            Some(MemoryUnavailableReason::BudgetExceeded)
        );
        pending.settle();
        let settled_admission = other
            .admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
            .unwrap();
        settled_admission.settle();
        asr.retire_after_exit();
        other.retire_after_exit();
        std::fs::remove_dir_all(root).unwrap();

        let root = test_root("pending-target-not-added-twice");
        let asr = ChildRegistration::register_with_sampler(
            std::process::id(),
            113,
            &root,
            sampler(
                HostMemoryPressure::Normal,
                Some(9 * 1024 * 1024 * 1024),
                FootprintSample::Observed(1024 * 1024 * 1024),
            ),
        )
        .unwrap();
        let other = ChildRegistration::register_with_sampler(
            std::process::id(),
            114,
            &root,
            sampler(
                HostMemoryPressure::Normal,
                Some(9 * 1024 * 1024 * 1024),
                FootprintSample::Observed(1024 * 1024 * 1024),
            ),
        )
        .unwrap();
        let first_pending = asr
            .admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
            .unwrap();
        let second_pending = other
            .admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
            .unwrap();
        first_pending.settle();
        second_pending.settle();
        asr.retire_after_exit();
        other.retire_after_exit();
        std::fs::remove_dir_all(root).unwrap();

        let root = test_root("pending-cost-current-above-target");
        let asr = ChildRegistration::register_with_sampler(
            std::process::id(),
            115,
            &root,
            sampler(
                HostMemoryPressure::Normal,
                Some(9 * 1024 * 1024 * 1024),
                FootprintSample::Observed(1024 * 1024 * 1024),
            ),
        )
        .unwrap();
        let other = ChildRegistration::register_with_sampler(
            std::process::id(),
            116,
            &root,
            sampler(
                HostMemoryPressure::Normal,
                Some(9 * 1024 * 1024 * 1024),
                FootprintSample::Observed(1024 * 1024 * 1024),
            ),
        )
        .unwrap();
        let pending = asr
            .admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
            .unwrap();
        asr.set_test_sampler(sampler(
            HostMemoryPressure::Normal,
            Some(9 * 1024 * 1024 * 1024),
            FootprintSample::Observed(4 * 1024 * 1024 * 1024),
        ));
        assert_eq!(
            other
                .admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
                .err(),
            Some(MemoryUnavailableReason::BudgetExceeded)
        );
        pending.settle();
        asr.retire_after_exit();
        other.retire_after_exit();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn separate_roots_have_independent_admission_budgets() {
        let root_asr = test_root("isolated-admission-asr");
        let root_cleanup = test_root("isolated-admission-cleanup");
        let asr = ChildRegistration::register_with_sampler(
            std::process::id(),
            117,
            &root_asr,
            sampler(
                HostMemoryPressure::Normal,
                Some(36 * 1024 * 1024 * 1024),
                FootprintSample::Observed(1024 * 1024 * 1024),
            ),
        )
        .unwrap();
        let cleanup = ChildRegistration::register_with_sampler(
            std::process::id(),
            118,
            &root_cleanup,
            sampler(
                HostMemoryPressure::Normal,
                Some(36 * 1024 * 1024 * 1024),
                FootprintSample::Observed(1024 * 1024 * 1024),
            ),
        )
        .unwrap();
        let asr_token = asr
            .admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
            .unwrap();
        let cleanup_token = cleanup
            .admit_cold_load(CLEANUP_MODEL_ID, CLEANUP_MODEL_REVISION, LoadKind::Cleanup)
            .unwrap();
        asr_token.settle();
        cleanup_token.settle();
        asr.retire_after_exit();
        cleanup.retire_after_exit();
        std::fs::remove_dir_all(root_asr).unwrap();
        std::fs::remove_dir_all(root_cleanup).unwrap();
    }

    #[test]
    fn admission_rejects_unknown_or_mismatched_other_owned_child() {
        for (name, other_sample, expected) in [
            (
                "unknown-other-child",
                FootprintSample::Unknown,
                MemoryUnavailableReason::UnknownChildFootprint,
            ),
            (
                "mismatch-other-child",
                FootprintSample::IdentityMismatch,
                MemoryUnavailableReason::ChildIdentityChanged,
            ),
        ] {
            let root = test_root(name);
            let asr = ChildRegistration::register_with_sampler(
                std::process::id(),
                119,
                &root,
                sampler(
                    HostMemoryPressure::Normal,
                    Some(64 * 1024 * 1024 * 1024),
                    FootprintSample::Observed(0),
                ),
            )
            .unwrap();
            let other = ChildRegistration::register_with_sampler(
                std::process::id(),
                120,
                &root,
                sampler(
                    HostMemoryPressure::Normal,
                    Some(64 * 1024 * 1024 * 1024),
                    other_sample,
                ),
            )
            .unwrap();
            assert_eq!(
                asr.admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
                    .err(),
                Some(expected)
            );
            asr.retire_after_exit();
            other.retire_after_exit();
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn pressure_sample_requires_exact_four_byte_known_values() {
        assert_eq!(decode_pressure(Some((1, 4))), HostMemoryPressure::Normal);
        assert_eq!(decode_pressure(Some((2, 4))), HostMemoryPressure::Warning);
        assert_eq!(decode_pressure(Some((4, 4))), HostMemoryPressure::Critical);
        for invalid in [
            None,
            Some((0, 4)),
            Some((3, 4)),
            Some((8, 4)),
            Some((1, 8)),
            Some((2, 0)),
        ] {
            assert_eq!(decode_pressure(invalid), HostMemoryPressure::Unknown);
        }
    }
    #[test]
    fn physical_memory_sample_requires_exact_nonzero_uint64() {
        assert_eq!(
            decode_physical_memory(Some((32u64 * 1024 * 1024 * 1024, 8))),
            Some(32u64 * 1024 * 1024 * 1024)
        );
        assert_eq!(decode_physical_memory(Some((0, 8))), None);
        assert_eq!(decode_physical_memory(Some((32, 4))), None);
        assert_eq!(decode_physical_memory(None), None);
    }
    #[test]
    fn roots_are_isolated_and_retiring_children_remain_registered_until_wait() {
        let root = std::env::temp_dir().join(format!("vf-model-root-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let child = ChildRegistration::register(std::process::id(), 17, &root).unwrap();
        let reservation = child.reserve_pending_load();
        assert_eq!(owned_children_for_root(&root).len(), 1);
        assert!(owned_children_for_root(&root.join("other")).is_empty());
        reservation.settle();
        assert!(!owned_children_for_root(&root)[0].2);
        child.retire_after_exit();
        assert!(owned_children_for_root(&root).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stale_nonce_cannot_clear_a_newer_reservation() {
        let root = std::env::temp_dir().join(format!("vf-model-nonce-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let child = ChildRegistration::register(std::process::id(), 23, &root).unwrap();
        let old = child.reserve_pending_load();
        let new = child.reserve_pending_load();
        old.settle();
        assert!(owned_children_for_root(&root)[0].2);
        new.settle();
        assert!(!owned_children_for_root(&root)[0].2);
        child.retire_after_exit();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_real_admissions_of_same_root_reserve_at_most_one_budget() {
        let root = test_root("admission-race");
        let barrier = Arc::new(Barrier::new(2));
        let asr = ChildRegistration::register_with_memory_sampler(
            std::process::id(),
            113,
            &root,
            Arc::new(AdmissionBarrierSampler {
                barrier: Arc::clone(&barrier),
                waited: AtomicBool::new(false),
            }),
        )
        .unwrap();
        let cleanup = ChildRegistration::register_with_memory_sampler(
            std::process::id(),
            114,
            &root,
            Arc::new(AdmissionBarrierSampler {
                barrier,
                waited: AtomicBool::new(false),
            }),
        )
        .unwrap();
        let asr_thread = std::thread::spawn({
            let asr = asr.clone();
            move || asr.admit_cold_load(ASR_MODEL_ID, ASR_MODEL_REVISION, LoadKind::Asr)
        });
        let cleanup_thread = std::thread::spawn({
            let cleanup = cleanup.clone();
            move || {
                cleanup.admit_cold_load(CLEANUP_MODEL_ID, CLEANUP_MODEL_REVISION, LoadKind::Cleanup)
            }
        });
        let asr_result = asr_thread.join().unwrap();
        let cleanup_result = cleanup_thread.join().unwrap();
        assert!(matches!(
            (&asr_result, &cleanup_result),
            (Ok(_), Err(MemoryUnavailableReason::SnapshotChanged))
                | (Err(MemoryUnavailableReason::SnapshotChanged), Ok(_))
        ));
        if let Ok(token) = asr_result {
            token.settle();
        }
        if let Ok(token) = cleanup_result {
            token.settle();
        }
        asr.retire_after_exit();
        cleanup.retire_after_exit();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn register_settle_and_actual_exit_during_sampling_invalidate_old_snapshot() {
        admission_rejects_snapshot_after_registry_mutation(SnapshotMutation::Register);
        admission_rejects_snapshot_after_registry_mutation(SnapshotMutation::Settle);
        admission_rejects_snapshot_after_registry_mutation(SnapshotMutation::ActualExit);
    }

    #[test]
    fn old_generation_reservation_cannot_settle_new_child_token() {
        let root = test_root("generation-token");
        let old = ChildRegistration::register(std::process::id(), 25, &root).unwrap();
        let old_token = old.reserve_pending_load();
        old.retire_after_exit();
        let current = ChildRegistration::register(std::process::id(), 26, &root).unwrap();
        let new_token = current.reserve_pending_load();
        old_token.settle();
        assert!(owned_children_for_root(&root)[0].2);
        new_token.settle();
        assert!(!owned_children_for_root(&root)[0].2);
        current.retire_after_exit();
        std::fs::remove_dir_all(root).unwrap();
    }
}
