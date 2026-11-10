//! Private, content-free terminal dictation performance trace persistence.

use crate::metrics::{
    CacheStatus, DeliveryOutcome, DictationClass, DictationTrace, ReportedStageDuration, Stage,
    StopToInsertTimer,
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1000;
const MAX_BYTES: usize = 10 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 64 * 1024;
const FILE_NAME: &str = "dictation-performance.jsonl";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedTrace {
    timestamp_ms: u64,
    session_generation: u64,
    class: DictationClass,
    outcome: DeliveryOutcome,
    total_ms: u64,
    stages: Vec<(Stage, u64, u64)>,
    reported_stages: Vec<ReportedStageDuration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_status: Option<CacheStatus>,
}

impl PersistedTrace {
    fn from_trace(
        trace: DictationTrace,
        outcome: DeliveryOutcome,
        total_ms: u64,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            timestamp_ms,
            session_generation: trace.session_generation,
            class: trace.class,
            outcome,
            total_ms,
            stages: trace.stages,
            reported_stages: trace.reported_stages,
            cache_status: trace.cache_status,
        }
    }

    fn within_schema_bounds(&self) -> bool {
        self.stages.len().saturating_add(self.reported_stages.len()) <= 256
    }
}

#[derive(Debug, Clone, Copy)]
struct RetentionPolicy {
    age_ms: u64,
    max_bytes: usize,
    max_line_bytes: usize,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            age_ms: RETENTION_MS,
            max_bytes: MAX_BYTES,
            max_line_bytes: MAX_LINE_BYTES,
        }
    }
}

#[derive(Clone)]
pub(crate) struct DictationTraceSink {
    root: PathBuf,
    epoch: Arc<AtomicU64>,
    file_lock: Arc<std::sync::Mutex<()>>,
    retention: RetentionPolicy,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    #[cfg(test)]
    io_gate: Option<Arc<TestIoGate>>,
    #[cfg(test)]
    blocking_test_root_admission: bool,
    #[cfg(test)]
    test_registry: Arc<crate::audio::SpoolOwnerRegistry>,
    #[cfg(test)]
    io_failure: Option<InjectedIoFailure>,
}

#[cfg(test)]
struct TestIoGate {
    entered: std::sync::mpsc::Sender<()>,
    released: std::sync::Mutex<bool>,
    changed: std::sync::Condvar,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
enum InjectedIoFailure {
    Create,
    Write,
    Sync,
    Rename,
}

impl DictationTraceSink {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root: crate::store::normalized_data_root(&root),
            epoch: Arc::new(AtomicU64::new(0)),
            file_lock: Arc::new(std::sync::Mutex::new(())),
            retention: RetentionPolicy::default(),
            clock: Arc::new(now_ms),
            #[cfg(test)]
            io_gate: None,
            #[cfg(test)]
            blocking_test_root_admission: false,
            #[cfg(test)]
            test_registry: crate::audio::SpoolOwnerRegistry::new_test(3),
            #[cfg(test)]
            io_failure: None,
        }
    }

    #[cfg(test)]
    fn with_test_policy(mut self, retention: RetentionPolicy, now: u64) -> Self {
        self.retention = retention;
        self.clock = Arc::new(move || now);
        self
    }

    #[cfg(test)]
    fn with_io_gate(mut self, gate: TestIoGate) -> Self {
        self.io_gate = Some(Arc::new(gate));
        self
    }

    #[cfg(test)]
    fn with_blocking_root_admission_for_test(mut self) -> Self {
        // Persistence fixtures should not fail on unrelated roots' short-lived
        // global admission-map lock; the contention test continues to use `bind`.
        self.blocking_test_root_admission = true;
        self
    }

    #[cfg(test)]
    fn with_io_failure(mut self, failure: InjectedIoFailure) -> Self {
        self.io_failure = Some(failure);
        self
    }

    pub(crate) fn bind(&self, timer: &mut StopToInsertTimer) {
        let sink = self.clone();
        let admitted_epoch = self.epoch.load(Ordering::Acquire);
        timer.bind_terminal_handoff(move |trace, outcome, total_ms| {
            sink.submit(
                trace,
                outcome,
                total_ms,
                admitted_epoch,
                #[cfg(test)]
                None,
            );
        });
    }

    #[cfg(test)]
    fn bind_for_test(
        &self,
        timer: &mut StopToInsertTimer,
    ) -> tokio::sync::oneshot::Receiver<Result<(), String>> {
        let sink = self.clone();
        let admitted_epoch = self.epoch.load(Ordering::Acquire);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        timer.bind_terminal_handoff(move |trace, outcome, total_ms| {
            sink.submit(trace, outcome, total_ms, admitted_epoch, Some(sender));
        });
        receiver
    }

    fn path(&self) -> PathBuf {
        self.root.join("diagnostics").join(FILE_NAME)
    }

    fn submit(
        &self,
        trace: DictationTrace,
        outcome: DeliveryOutcome,
        total_ms: u64,
        epoch: u64,
        #[cfg(test)] completion: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    ) {
        if self.epoch.load(Ordering::Acquire) != epoch {
            #[cfg(test)]
            if let Some(completion) = completion {
                let _ = completion.send(Ok(()));
            }
            return;
        }
        let record = PersistedTrace::from_trace(trace, outcome, total_ms, (self.clock)());
        if !record.within_schema_bounds() {
            #[cfg(test)]
            if let Some(completion) = completion {
                let _ = completion.send(Ok(()));
            }
            return;
        }
        let Ok(mut line) = serde_json::to_vec(&record) else {
            #[cfg(test)]
            if let Some(completion) = completion {
                let _ = completion.send(Err("encode".into()));
            }
            return;
        };
        if line.len() + 1 > self.retention.max_line_bytes {
            #[cfg(test)]
            if let Some(completion) = completion {
                let _ = completion.send(Ok(()));
            }
            return;
        }
        line.push(b'\n');
        let reserved_bytes = line.len();
        #[cfg(test)]
        let mutation_admission = if self.blocking_test_root_admission {
            crate::store::begin_spool_root_mutation(&self.root)
        } else {
            crate::store::try_begin_spool_root_mutation(&self.root)
        };
        #[cfg(not(test))]
        let mutation_admission = crate::store::try_begin_spool_root_mutation(&self.root);

        let mutation = match mutation_admission {
            Ok(mutation) => mutation,
            Err(_error) => {
                #[cfg(test)]
                if let Some(completion) = completion {
                    let _ = completion.send(Err(_error.to_string()));
                }
                return;
            }
        };
        let root = mutation.data_root().to_path_buf();
        let sink = self.clone();
        let file_lock = sink.file_lock.clone();
        let retention = sink.retention;
        #[cfg(test)]
        let completion = std::sync::Arc::new(std::sync::Mutex::new(completion));
        #[cfg(test)]
        let completion_for_job = completion.clone();
        let operation = move |_: &crate::store::SpoolRootMutationLease| {
            #[cfg(test)]
            if let Some(gate) = &sink.io_gate {
                let _ = gate.entered.send(());
            }
            let _file_guard = file_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if sink.epoch.load(Ordering::Acquire) != epoch {
                #[cfg(test)]
                if let Some(completion) = completion_for_job.lock().unwrap().take() {
                    let _ = completion.send(Ok(()));
                }
                return Ok(());
            }
            #[cfg(test)]
            if let Some(gate) = &sink.io_gate {
                let mut released = gate.released.lock().unwrap();
                while !*released {
                    released = gate.changed.wait(released).unwrap();
                }
            }
            let result = write_record_with_failure(
                &sink.path(),
                &line,
                (sink.clock)(),
                retention.max_bytes,
                retention.age_ms,
                #[cfg(test)]
                sink.io_failure,
                #[cfg(not(test))]
                None,
            )
            .map_err(|_| "trace write failed".to_owned());
            #[cfg(test)]
            if let Some(completion) = completion_for_job.lock().unwrap().take() {
                let _ = completion.send(result.clone());
            }
            result
        };
        #[cfg(test)]
        let admission = crate::audio::try_enqueue_retained_spool_store_job_with_registry(
            self.test_registry.clone(),
            "dictation-trace".into(),
            root,
            reserved_bytes,
            mutation,
            operation,
        );
        #[cfg(not(test))]
        let admission = crate::audio::try_enqueue_retained_spool_store_job_with_mutation(
            "dictation-trace".into(),
            root,
            reserved_bytes,
            mutation,
            operation,
        );
        #[cfg(test)]
        if let Err(error) = admission {
            if let Some(completion) = completion.lock().unwrap().take() {
                let _ = completion.send(Err(format!("{error:?}")));
            }
        }
        #[cfg(not(test))]
        let _ = admission;
    }

    pub(crate) fn invalidate_before_clear(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn initialize(&self) {
        let Ok(mutation) = crate::store::begin_spool_root_mutation(&self.root) else {
            return;
        };
        let path = self.path();
        let file_lock = self.file_lock.clone();
        let retention = self.retention;
        let clock = self.clock.clone();
        let operation = move |_: &crate::store::SpoolRootMutationLease| {
            let _file_guard = file_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            prune_file_with_limits(&path, clock(), retention.max_bytes, retention.age_ms)
                .map_err(|_| "trace prune failed".into())
        };
        #[cfg(test)]
        let _ = crate::audio::enqueue_retained_spool_store_job_with_registry(
            self.test_registry.clone(),
            "dictation-trace-prune".into(),
            mutation.data_root().to_path_buf(),
            0,
            mutation,
            operation,
        );
        #[cfg(not(test))]
        let _ = crate::audio::enqueue_retained_spool_store_job_with_mutation(
            "dictation-trace-prune".into(),
            mutation.data_root().to_path_buf(),
            0,
            mutation,
            operation,
        );
    }
}

pub(crate) fn clear_all_data_under_guard(
    root: &Path,
    guard: &crate::store::SpoolClearGuard,
    sink: &DictationTraceSink,
) -> anyhow::Result<()> {
    if !guard.owns_root(root) || crate::store::normalized_data_root(root) != sink.root {
        anyhow::bail!("trace clear owner does not match the admitted data root");
    }
    sink.invalidate_before_clear();
    crate::store::clear_all_data_under_guard(root, guard)?;
    crate::delivery_diagnostics::clear(root)?;
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
fn write_record_with_limits(
    path: &Path,
    incoming: &[u8],
    now: u64,
    max_bytes: usize,
    retention_ms: u64,
) -> std::io::Result<()> {
    write_record_with_failure(path, incoming, now, max_bytes, retention_ms, None)
}

fn write_record_with_failure(
    path: &Path,
    incoming: &[u8],
    now: u64,
    max_bytes: usize,
    retention_ms: u64,
    #[cfg(test)] failure: Option<InjectedIoFailure>,
    #[cfg(not(test))] _unused: Option<()>,
) -> std::io::Result<()> {
    if incoming.len() > MAX_LINE_BYTES || incoming.last() != Some(&b'\n') {
        return Ok(());
    }
    if incoming.len() > max_bytes {
        return prune_file_with_limits(path, now, max_bytes, retention_ms);
    }
    let incoming_record: PersistedTrace =
        serde_json::from_slice(&incoming[..incoming.len() - 1])
            .map_err(|_| std::io::Error::other("invalid incoming trace"))?;
    let cutoff = now.saturating_sub(retention_ms);
    if !incoming_record.within_schema_bounds()
        || incoming_record.timestamp_ms < cutoff
        || incoming_record.timestamp_ms > now
    {
        return Ok(());
    }
    let mut records = read_valid(path, now, max_bytes, retention_ms)?;
    records.push_back(incoming.to_vec());
    let mut records = records.into_iter().collect::<Vec<_>>();
    records.sort_by_key(|line| {
        serde_json::from_slice::<PersistedTrace>(&line[..line.len() - 1])
            .map(|record| record.timestamp_ms)
            .unwrap_or_default()
    });
    let mut records = records.into_iter().collect::<VecDeque<_>>();
    let mut bytes = records.iter().map(Vec::len).sum::<usize>();
    while bytes > max_bytes {
        let Some(oldest) = records.pop_front() else {
            break;
        };
        bytes -= oldest.len();
    }
    let output = records.iter().flatten().copied().collect::<Vec<_>>();
    replace_private_with_failure(
        path,
        &output,
        #[cfg(test)]
        failure,
        #[cfg(not(test))]
        None,
    )
}

fn prune_file_with_limits(
    path: &Path,
    now: u64,
    max_bytes: usize,
    retention_ms: u64,
) -> std::io::Result<()> {
    let records = read_valid(path, now, max_bytes, retention_ms)?;
    let mut retained = records;
    let mut bytes = retained.iter().map(Vec::len).sum::<usize>();
    while bytes > max_bytes {
        let Some(oldest) = retained.pop_front() else {
            break;
        };
        bytes -= oldest.len();
    }
    let output = retained.iter().flatten().copied().collect::<Vec<_>>();
    replace_private(path, &output)
}

fn read_valid(
    path: &Path,
    now: u64,
    max_bytes: usize,
    retention_ms: u64,
) -> std::io::Result<VecDeque<Vec<u8>>> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("missing trace parent"))?;
    match fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(std::io::Error::other(
                "trace parent is not a real directory",
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(VecDeque::new()),
        Err(error) => return Err(error),
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(VecDeque::new()),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_file() {
        return Err(std::io::Error::other("not a regular trace file"));
    }
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(0x0000_0100); // O_NOFOLLOW
    let file = options.open(path)?;
    let mut raw = Vec::new();
    file.take((max_bytes + 1) as u64).read_to_end(&mut raw)?;
    if raw.len() > max_bytes {
        return Ok(VecDeque::new());
    }
    let cutoff = now.saturating_sub(retention_ms);
    let mut retained = raw
        .split_inclusive(|byte| *byte == b'\n')
        .filter_map(|line| {
            if line.len() > MAX_LINE_BYTES || line.last() != Some(&b'\n') {
                return None;
            }
            let record: PersistedTrace = serde_json::from_slice(&line[..line.len() - 1]).ok()?;
            if !record.within_schema_bounds()
                || record.timestamp_ms < cutoff
                || record.timestamp_ms > now
            {
                return None;
            }
            let mut canonical = serde_json::to_vec(&record).ok()?;
            canonical.push(b'\n');
            (canonical.len() <= MAX_LINE_BYTES).then_some(canonical)
        })
        .collect::<Vec<_>>();
    retained.sort_by_key(|line| {
        serde_json::from_slice::<PersistedTrace>(&line[..line.len() - 1])
            .map(|record| record.timestamp_ms)
            .unwrap_or_default()
    });
    Ok(retained.into())
}

fn replace_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    replace_private_with_failure(
        path,
        bytes,
        #[cfg(test)]
        None,
        #[cfg(not(test))]
        None,
    )
}

fn replace_private_with_failure(
    path: &Path,
    bytes: &[u8],
    #[cfg(test)] failure: Option<InjectedIoFailure>,
    #[cfg(not(test))] _unused: Option<()>,
) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("missing trace parent"))?;
    match fs::symlink_metadata(parent) {
        Ok(meta) if meta.file_type().is_dir() => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if meta.permissions().mode() & 0o777 != 0o700 {
                    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
                }
            }
        }
        Ok(_) => return Err(std::io::Error::other("trace parent is not a directory")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new().mode(0o700).create(parent)?;
            }
            #[cfg(not(unix))]
            fs::create_dir(parent)?;
        }
        Err(error) => return Err(error),
    }
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.file_type().is_file() {
            return Err(std::io::Error::other("trace destination is not regular"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o777 != 0o600 {
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            }
        }
    }
    let nonce = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(
        ".dictation-trace-{nonce}-{}.tmp",
        std::process::id()
    ));
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut owns_temp = false;
    let result = (|| {
        #[cfg(test)]
        if matches!(failure, Some(InjectedIoFailure::Create)) {
            return Err(std::io::Error::other("injected create failure"));
        }
        let mut file = opts.open(&temp)?;
        owns_temp = true;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file() {
            return Err(std::io::Error::other("trace temp is not a regular file"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o777 != 0o600 {
                return Err(std::io::Error::other("trace temp mode is not private"));
            }
        }
        #[cfg(test)]
        if matches!(failure, Some(InjectedIoFailure::Write)) {
            file.write_all(&bytes[..bytes.len().min(8)])?;
            return Err(std::io::Error::other("injected write failure"));
        }
        file.write_all(bytes)?;
        #[cfg(test)]
        if matches!(failure, Some(InjectedIoFailure::Sync)) {
            return Err(std::io::Error::other("injected sync failure"));
        }
        file.sync_all()?;
        #[cfg(test)]
        if matches!(failure, Some(InjectedIoFailure::Rename)) {
            return Err(std::io::Error::other("injected rename failure"));
        }
        fs::rename(&temp, path)?;
        let replaced = fs::symlink_metadata(path)?;
        if !replaced.file_type().is_file() {
            return Err(std::io::Error::other("trace replacement is not regular"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if replaced.permissions().mode() & 0o777 != 0o600 {
                return Err(std::io::Error::other(
                    "trace replacement mode is not private",
                ));
            }
        }
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() && owns_temp {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{MetricGroup, Metrics, Stage};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio_util::sync::CancellationToken;

    static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct TestRoot(PathBuf);

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn owned_root() -> TestRoot {
        static ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "voiceflow-trace-test-{}-{nonce}-{}",
            std::process::id(),
            ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("create unique owned test root");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .expect("make fixture root private");
        }
        TestRoot(root)
    }

    #[tokio::test]
    async fn trace_sink_contains_no_text_fields() {
        let _serial = TEST_LOCK.lock().await;
        let root = owned_root();
        let sink = DictationTraceSink::new(root.0.clone()).with_blocking_root_admission_for_test();
        let metrics = Metrics::default();
        let mut timer =
            metrics.stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
        let trace = timer.bind_trace(17);
        let completion = sink.bind_for_test(&mut timer);
        let mut span = trace.begin(Stage::AsrInference);
        span.finish();
        trace.record_reported_seconds(
            Stage::Prefill,
            0.012,
            crate::metrics::ReportedScope::FullPrompt,
        );
        for _ in 0..254 {
            trace.record_reported_seconds(
                Stage::Decode,
                0.001,
                crate::metrics::ReportedScope::Generation,
            );
        }
        assert_eq!(
            trace.snapshot().stages.len() + trace.snapshot().reported_stages.len(),
            256
        );
        trace.record_reported_seconds(
            Stage::Decode,
            0.001,
            crate::metrics::ReportedScope::Generation,
        );
        assert_eq!(trace.snapshot().reported_stages.len(), 255);
        trace.record_cache_status(Some("unknown-value"));
        timer.finish(DeliveryOutcome::PasteConfirmed, None);
        drop(span);
        let frozen = trace.snapshot();
        trace.record_reported_seconds(
            Stage::Decode,
            1.0,
            crate::metrics::ReportedScope::Generation,
        );
        assert_eq!(trace.snapshot(), frozen);
        completion.await.unwrap().unwrap();
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(sink.test_registry.clone())
            .await;
        for (generation, outcome) in [
            DeliveryOutcome::PasteUnconfirmed,
            DeliveryOutcome::Copied,
            DeliveryOutcome::HistoryOnly,
            DeliveryOutcome::PreviewOnly,
        ]
        .into_iter()
        .enumerate()
        {
            let mut timer =
                metrics.stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
            timer.bind_trace(18 + generation as u64);
            let completion = sink.bind_for_test(&mut timer);
            timer.finish(outcome, None);
            completion.await.unwrap().unwrap();
            crate::audio::wait_for_retained_spool_registry_to_exit_for_test(
                sink.test_registry.clone(),
            )
            .await;
        }
        let failed_token = CancellationToken::new();
        let mut failed =
            metrics.stop_to_insert(MetricGroup::unknown("dictation"), failed_token.clone());
        failed.bind_trace(23);
        let completion = sink.bind_for_test(&mut failed);
        drop(failed);
        completion.await.unwrap().unwrap();
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(sink.test_registry.clone())
            .await;
        let cancelled_token = CancellationToken::new();
        let mut cancelled =
            metrics.stop_to_insert(MetricGroup::unknown("dictation"), cancelled_token.clone());
        cancelled.bind_trace(24);
        let completion = sink.bind_for_test(&mut cancelled);
        cancelled_token.cancel();
        drop(cancelled);
        completion.await.unwrap().unwrap();
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(sink.test_registry.clone())
            .await;

        let path = sink.path();
        let contents = fs::read_to_string(&path).expect("terminal owner must append its trace");
        assert_eq!(contents.lines().count(), 7);
        assert!(!contents.contains("private transcript sentinel"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        let value: serde_json::Value =
            serde_json::from_str(contents.lines().next().unwrap()).unwrap();
        let keys = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            keys,
            [
                "cache_status",
                "class",
                "outcome",
                "reported_stages",
                "session_generation",
                "stages",
                "timestamp_ms",
                "total_ms"
            ]
            .into_iter()
            .collect()
        );
        assert_eq!(value["class"], "unclassified");
        assert_eq!(value["session_generation"], 17);
        assert_eq!(value["stages"][0].as_array().unwrap().len(), 3);
        assert_eq!(value["stages"][0][0], "asr_inference");
        assert_eq!(value["reported_stages"][0]["scope"], "full_prompt");
        assert_eq!(value["cache_status"], "unknown");
        let total_ms = value["total_ms"].as_u64().unwrap();
        assert!(value["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| { stage[1].as_u64().unwrap() + stage[2].as_u64().unwrap() <= total_ms }));
        let reported_keys = value["reported_stages"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            reported_keys,
            ["duration_ms", "scope", "stage"].into_iter().collect()
        );
        let outcomes = contents
            .lines()
            .map(|line| {
                serde_json::from_str::<serde_json::Value>(line).unwrap()["outcome"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(outcomes.len(), 7);

        let mut unbound =
            metrics.stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
        unbound.bind_trace(25);
        unbound.finish(DeliveryOutcome::PreviewOnly, None);
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 7);
    }

    #[tokio::test]
    async fn trace_sink_prunes_by_age_and_size() {
        let _serial = TEST_LOCK.lock().await;
        let root = owned_root();
        let path = root.0.join("diagnostics").join(FILE_NAME);
        fs::create_dir(root.0.join("diagnostics")).unwrap();
        let now = 1_000_000_u64;
        let make_line = |timestamp_ms| {
            let record = PersistedTrace {
                timestamp_ms,
                session_generation: timestamp_ms,
                class: DictationClass::Unclassified,
                outcome: DeliveryOutcome::Copied,
                total_ms: 9,
                stages: vec![(
                    if timestamp_ms == now - 99 {
                        Stage::TargetConfirmation
                    } else {
                        Stage::QueueWait
                    },
                    1,
                    2,
                )],
                reported_stages: Vec::new(),
                cache_status: None,
            };
            let mut line = serde_json::to_vec(&record).unwrap();
            line.push(b'\n');
            line
        };
        let exact_cutoff = make_line(now - 100);
        let recent = make_line(now - 99);
        let old = make_line(now - 101);
        let future = make_line(now + 1);
        let unknown = b"{\"timestamp_ms\":1000000,\"unexpected\":true}\n".to_vec();
        let mut unknown_nested = serde_json::to_value(PersistedTrace {
            timestamp_ms: now,
            session_generation: 998,
            class: DictationClass::Unclassified,
            outcome: DeliveryOutcome::Copied,
            total_ms: 1,
            stages: Vec::new(),
            reported_stages: vec![ReportedStageDuration {
                stage: Stage::Decode,
                duration_ms: 1,
                scope: crate::metrics::ReportedScope::Generation,
            }],
            cache_status: None,
        })
        .unwrap();
        unknown_nested["reported_stages"][0]["future_field"] = serde_json::json!("sentinel");
        let mut unknown_nested = serde_json::to_vec(&unknown_nested).unwrap();
        unknown_nested.push(b'\n');
        let forbidden_text = format!(
            "{{\"timestamp_ms\":{now},\"session_generation\":997,\"class\":\"unclassified\",\"outcome\":\"copied\",\"total_ms\":1,\"stages\":[],\"reported_stages\":[],\"transcript\":\"private transcript sentinel\"}}\n"
        )
        .into_bytes();
        let mut too_many = serde_json::to_vec(&PersistedTrace {
            timestamp_ms: now,
            session_generation: 999,
            class: DictationClass::Unclassified,
            outcome: DeliveryOutcome::Copied,
            total_ms: 1,
            stages: vec![(Stage::QueueWait, 0, 1); 257],
            reported_stages: Vec::new(),
            cache_status: None,
        })
        .unwrap();
        too_many.push(b'\n');
        let prior = [
            exact_cutoff.clone(),
            recent.clone(),
            old,
            future,
            unknown,
            unknown_nested,
            forbidden_text,
            too_many,
            b"{truncated}\n".to_vec(),
        ]
        .concat();
        let prior_len = prior.len();
        fs::write(&path, prior).unwrap();
        let incoming = make_line(now);
        let byte_cap = prior_len + incoming.len();
        write_record_with_limits(&path, &incoming, now, byte_cap, 100).unwrap();
        let retained = fs::read(&path).unwrap();
        assert!(retained.len() <= byte_cap);
        let lines = retained
            .split_inclusive(|byte| *byte == b'\n')
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 3);
        assert!(lines
            .iter()
            .all(|line| serde_json::from_slice::<PersistedTrace>(&line[..line.len() - 1]).is_ok()));
        let retained_stages = lines
            .iter()
            .map(|line| {
                serde_json::from_slice::<PersistedTrace>(&line[..line.len() - 1])
                    .unwrap()
                    .stages[0]
                    .0
            })
            .collect::<Vec<_>>();
        assert!(retained_stages.contains(&Stage::TargetConfirmation));
        assert!(retained_stages.contains(&Stage::QueueWait));
        let crash_temp = path.parent().unwrap().join(".dictation-trace-crashed.tmp");
        fs::write(&crash_temp, b"uncommitted sentinel\n").unwrap();
        prune_file_with_limits(&path, now, byte_cap, 100).unwrap();
        assert!(!fs::read(&path)
            .unwrap()
            .windows(10)
            .any(|window| window == b"uncommitte"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
            fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o777)).unwrap();
            prune_file_with_limits(&path, now, byte_cap, 100).unwrap();
            assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
            assert_eq!(
                fs::metadata(path.parent().unwrap()).unwrap().mode() & 0o777,
                0o700
            );
            let sequence = TEMP_SEQUENCE.load(Ordering::Relaxed);
            let collision = path.parent().unwrap().join(format!(
                ".dictation-trace-{sequence}-{}.tmp",
                std::process::id()
            ));
            fs::write(&collision, b"owned collision sentinel").unwrap();
            assert!(replace_private(&path, b"replacement\n").is_err());
            assert_eq!(fs::read(&collision).unwrap(), b"owned collision sentinel");
        }

        let oversized_path = root.0.join("diagnostics").join("oversized.jsonl");
        fs::write(&oversized_path, vec![b'x'; 512]).unwrap();
        prune_file_with_limits(&oversized_path, now, 128, 100).unwrap();
        assert_eq!(fs::metadata(&oversized_path).unwrap().len(), 0);

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let target = root.0.join("private-target");
            fs::create_dir(&target).unwrap();
            let sentinel = target.join(FILE_NAME);
            fs::write(&sentinel, b"symlink target sentinel").unwrap();
            let parent_link = root.0.join("diagnostics-link");
            symlink(&target, &parent_link).unwrap();
            assert!(read_valid(&parent_link.join(FILE_NAME), now, 1024, 100).is_err());
            let file_link = root.0.join("diagnostics").join("linked.jsonl");
            symlink(&sentinel, &file_link).unwrap();
            assert!(read_valid(&file_link, now, 1024, 100).is_err());
            assert_eq!(fs::read(&sentinel).unwrap(), b"symlink target sentinel");
        }

        let size_path = root.0.join("diagnostics").join("size.jsonl");
        let size_cap = recent.len() + incoming.len();
        write_record_with_limits(&size_path, &recent, now, size_cap, 100).unwrap();
        write_record_with_limits(&size_path, &incoming, now, size_cap, 100).unwrap();
        assert_eq!(fs::metadata(&size_path).unwrap().len() as usize, size_cap);
        let out_of_order_path = root.0.join("diagnostics").join("out-of-order.jsonl");
        let newer_existing = make_line(now - 10);
        let older_incoming = make_line(now - 20);
        write_record_with_limits(
            &out_of_order_path,
            &newer_existing,
            now,
            newer_existing.len(),
            100,
        )
        .unwrap();
        write_record_with_limits(
            &out_of_order_path,
            &older_incoming,
            now,
            newer_existing.len(),
            100,
        )
        .unwrap();
        let ordered = read_valid(&out_of_order_path, now, newer_existing.len(), 100).unwrap();
        assert_eq!(ordered.len(), 1);
        assert_eq!(
            serde_json::from_slice::<PersistedTrace>(&ordered[0][..ordered[0].len() - 1])
                .unwrap()
                .timestamp_ms,
            now - 10
        );
        let mut oversized = vec![b'x'; size_cap];
        oversized.push(b'\n');
        write_record_with_limits(&size_path, &oversized, now, size_cap, 100).unwrap();
        assert_eq!(fs::metadata(&size_path).unwrap().len() as usize, size_cap);
        let before_oversized_line = fs::read(&size_path).unwrap();
        let mut oversized_line = vec![b'x'; MAX_LINE_BYTES + 1];
        *oversized_line.last_mut().unwrap() = b'\n';
        write_record_with_limits(&size_path, &oversized_line, now, MAX_BYTES, RETENTION_MS)
            .unwrap();
        assert_eq!(fs::read(&size_path).unwrap(), before_oversized_line);

        let owned = owned_root();
        let startup_fixture = [
            exact_cutoff.clone(),
            recent.clone(),
            make_line(now - 101),
            make_line(now + 1),
            b"{\"timestamp_ms\":1000000,\"unexpected\":true}\n".to_vec(),
            {
                let mut line = serde_json::to_vec(&PersistedTrace {
                    timestamp_ms: now,
                    session_generation: 999,
                    class: DictationClass::Unclassified,
                    outcome: DeliveryOutcome::Copied,
                    total_ms: 1,
                    stages: vec![(Stage::QueueWait, 0, 1); 257],
                    reported_stages: Vec::new(),
                    cache_status: None,
                })
                .unwrap();
                line.push(b'\n');
                line
            },
            b"{truncated}".to_vec(),
        ]
        .concat();
        let configured_cap = startup_fixture.len() + incoming.len();
        let sink = DictationTraceSink::new(owned.0.clone()).with_test_policy(
            RetentionPolicy {
                age_ms: 100,
                max_bytes: configured_cap,
                max_line_bytes: MAX_LINE_BYTES,
            },
            now,
        );
        let sink_path = sink.path();
        fs::create_dir_all(sink_path.parent().unwrap()).unwrap();
        fs::write(&sink_path, startup_fixture).unwrap();
        sink.initialize();
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(sink.test_registry.clone())
            .await;
        let pruned = fs::read(&sink_path).unwrap();
        let pruned_rows = pruned
            .split_inclusive(|byte| *byte == b'\n')
            .collect::<Vec<_>>();
        assert_eq!(pruned_rows.len(), 2);
        let metrics = Metrics::default();
        let mut timer =
            metrics.stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
        let trace = timer.bind_trace(200);
        trace.record_reported_seconds(
            Stage::Prefill,
            0.1,
            crate::metrics::ReportedScope::FullPrompt,
        );
        let completion = sink.bind_for_test(&mut timer);
        timer.finish(DeliveryOutcome::Copied, None);
        completion.await.unwrap().unwrap();
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(sink.test_registry.clone())
            .await;
        let actual = fs::read(&sink_path).unwrap();
        assert!(actual.len() <= configured_cap);
        assert_eq!(actual.split_inclusive(|byte| *byte == b'\n').count(), 3);
        let generations = actual
            .split_inclusive(|byte| *byte == b'\n')
            .map(|line| {
                serde_json::from_slice::<PersistedTrace>(&line[..line.len() - 1])
                    .unwrap()
                    .session_generation
            })
            .collect::<Vec<_>>();
        assert_eq!(generations, [now - 100, now - 99, 200]);

        let size_root = owned_root();
        let expected_new = PersistedTrace {
            timestamp_ms: now,
            session_generation: 300,
            class: DictationClass::Unclassified,
            outcome: DeliveryOutcome::Copied,
            total_ms: 0,
            stages: Vec::new(),
            reported_stages: vec![ReportedStageDuration {
                stage: Stage::Prefill,
                duration_ms: 100,
                scope: crate::metrics::ReportedScope::FullPrompt,
            }],
            cache_status: None,
        };
        let size_cap = recent.len() + serde_json::to_vec(&expected_new).unwrap().len() + 1;
        let size_sink = DictationTraceSink::new(size_root.0.clone()).with_test_policy(
            RetentionPolicy {
                age_ms: 100,
                max_bytes: size_cap,
                max_line_bytes: MAX_LINE_BYTES,
            },
            now,
        );
        fs::create_dir_all(size_sink.path().parent().unwrap()).unwrap();
        fs::write(size_sink.path(), [exact_cutoff, recent].concat()).unwrap();
        let mut size_timer =
            metrics.stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
        let size_trace = size_timer.bind_trace(300);
        size_trace.record_reported_seconds(
            Stage::Prefill,
            0.1,
            crate::metrics::ReportedScope::FullPrompt,
        );
        let size_completion = size_sink.bind_for_test(&mut size_timer);
        size_timer.finish(DeliveryOutcome::Copied, None);
        size_completion.await.unwrap().unwrap();
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(
            size_sink.test_registry.clone(),
        )
        .await;
        let size_actual = fs::read(size_sink.path()).unwrap();
        assert!(size_actual.len() <= size_cap);
        let size_generations = size_actual
            .split_inclusive(|byte| *byte == b'\n')
            .map(|line| {
                serde_json::from_slice::<PersistedTrace>(&line[..line.len() - 1])
                    .unwrap()
                    .session_generation
            })
            .collect::<Vec<_>>();
        assert_eq!(size_generations, [now - 99, 300]);
    }

    #[tokio::test]
    async fn clear_all_data_removes_trace_sink() {
        let _serial = TEST_LOCK.lock().await;
        let root = owned_root();
        let sink = DictationTraceSink::new(root.0.clone());
        let metrics = Metrics::default();
        let other_root = owned_root();
        let wrong_guard = crate::store::begin_clear_all_data(&other_root.0).unwrap();
        assert!(clear_all_data_under_guard(&root.0, &wrong_guard, &sink).is_err());
        drop(wrong_guard);
        let mut admitted_after_wrong_guard =
            metrics.stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
        admitted_after_wrong_guard.bind_trace(2);
        let admitted_receipt = sink.bind_for_test(&mut admitted_after_wrong_guard);
        admitted_after_wrong_guard.finish(DeliveryOutcome::Copied, None);
        admitted_receipt.await.unwrap().unwrap();
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(sink.test_registry.clone())
            .await;
        assert!(
            sink.path().exists(),
            "wrong-root guard must not revoke epoch"
        );

        let mut timer =
            metrics.stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
        timer.bind_trace(3);
        let completion = sink.bind_for_test(&mut timer);
        let guard = crate::store::begin_clear_all_data(&root.0).unwrap();
        clear_all_data_under_guard(&root.0, &guard, &sink).unwrap();
        drop(guard);
        timer.finish(DeliveryOutcome::PasteConfirmed, None);
        completion.await.unwrap().unwrap();
        assert!(!sink.path().exists());

        let mut fresh_timer =
            metrics.stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
        fresh_timer.bind_trace(4);
        let fresh_receipt = sink.bind_for_test(&mut fresh_timer);
        fresh_timer.finish(DeliveryOutcome::Copied, None);
        fresh_receipt.await.unwrap().unwrap();
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(sink.test_registry.clone())
            .await;
        assert!(
            sink.path().exists(),
            "new epoch admits a fresh terminal trace"
        );
    }

    #[tokio::test]
    async fn retained_clear_timeout_keeps_owner_guard_until_real_completion() {
        let _serial = TEST_LOCK.lock().await;
        let root = owned_root();
        let other = owned_root();
        let sink = DictationTraceSink::new(root.0.clone());
        let mut stale = Metrics::default()
            .stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
        stale.bind_trace(710);
        let stale_receipt = sink.bind_for_test(&mut stale);
        stale.finish(DeliveryOutcome::Copied, None);
        stale_receipt.await.unwrap().unwrap();
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(sink.test_registry.clone())
            .await;
        assert!(sink.path().exists());

        let mut late = Metrics::default()
            .stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
        late.bind_trace(711);
        let late_receipt = sink.bind_for_test(&mut late);
        let clear_guard = crate::store::begin_clear_all_data(&root.0).unwrap();
        let sink_for_owner = sink.clone();
        let root_for_owner = root.0.clone();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let owner = tokio::spawn(crate::audio::run_retained_spool_clear_job_with_registry(
            sink.test_registry.clone(),
            "dictation-trace-clear-test".into(),
            root.0.clone(),
            move || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                clear_all_data_under_guard(&root_for_owner, &clear_guard, &sink_for_owner)
                    .map_err(|error| error.to_string())
            },
        ));
        tokio::task::spawn_blocking(move || {
            entered_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("retained clear owner reached the barrier")
        })
        .await
        .unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), owner)
                .await
                .is_err(),
            "caller timeout must not release the retained owner"
        );
        assert!(crate::store::begin_clear_all_data(&root.0).is_err());
        assert!(sink.path().exists());
        release_tx.send(()).unwrap();
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(sink.test_registry.clone())
            .await;
        assert!(!sink.path().exists());
        assert!(other.0.exists(), "clear owner is root-scoped");
        assert!(crate::store::begin_clear_all_data(&root.0).is_ok());

        late.finish(DeliveryOutcome::Copied, None);
        late_receipt.await.unwrap().unwrap();
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(sink.test_registry.clone())
            .await;
        assert!(!sink.path().exists());

        let mut fresh = Metrics::default()
            .stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
        fresh.bind_trace(712);
        let fresh_receipt = sink.bind_for_test(&mut fresh);
        fresh.finish(DeliveryOutcome::Copied, None);
        fresh_receipt.await.unwrap().unwrap();
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(sink.test_registry.clone())
            .await;
        assert!(sink.path().exists());
        let repeated_guard = crate::store::begin_clear_all_data(&root.0).unwrap();
        clear_all_data_under_guard(&root.0, &repeated_guard, &sink).unwrap();
        drop(repeated_guard);
        assert!(!sink.path().exists(), "repeated clear remains idempotent");
    }

    #[tokio::test]
    async fn post_start_clear_error_is_truthful_and_does_not_reactivate_old_tokens() {
        let _serial = TEST_LOCK.lock().await;
        let root = owned_root();
        let sink = DictationTraceSink::new(root.0.clone());
        let mut stale = Metrics::default()
            .stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
        stale.bind_trace(713);
        let stale_receipt = sink.bind_for_test(&mut stale);
        fs::write(root.0.join("diagnostics"), b"diagnostics obstruction").unwrap();
        let guard = crate::store::begin_clear_all_data(&root.0).unwrap();
        let root_for_owner = root.0.clone();
        let sink_for_owner = sink.clone();
        let error = crate::audio::run_retained_spool_clear_job_with_registry(
            sink.test_registry.clone(),
            "dictation-trace-clear-error-test".into(),
            root.0.clone(),
            move || {
                clear_all_data_under_guard(&root_for_owner, &guard, &sink_for_owner)
                    .map_err(|error| error.to_string())
            },
        )
        .await
        .expect_err("post-start diagnostics removal error remains an error");
        assert!(error.contains("Not a directory"));
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(sink.test_registry.clone())
            .await;
        assert_eq!(
            fs::read(root.0.join("diagnostics")).unwrap(),
            b"diagnostics obstruction"
        );
        stale.finish(DeliveryOutcome::Copied, None);
        stale_receipt.await.unwrap().unwrap();
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(sink.test_registry.clone())
            .await;
        assert!(!root.0.join("diagnostics").join(FILE_NAME).exists());
    }

    #[tokio::test]
    async fn replacement_failures_preserve_the_valid_destination_and_budget_drops() {
        let _serial = TEST_LOCK.lock().await;
        let now = now_ms();
        let prior_record = PersistedTrace {
            timestamp_ms: now,
            session_generation: 700,
            class: DictationClass::Unclassified,
            outcome: DeliveryOutcome::Copied,
            total_ms: 5,
            stages: Vec::new(),
            reported_stages: Vec::new(),
            cache_status: None,
        };
        let mut prior = serde_json::to_vec(&prior_record).unwrap();
        prior.push(b'\n');
        for failure in [
            InjectedIoFailure::Create,
            InjectedIoFailure::Write,
            InjectedIoFailure::Sync,
            InjectedIoFailure::Rename,
        ] {
            let root = owned_root();
            let sink = DictationTraceSink::new(root.0.clone()).with_io_failure(failure);
            fs::create_dir_all(sink.path().parent().unwrap()).unwrap();
            fs::write(sink.path(), &prior).unwrap();
            let mut timer = Metrics::default()
                .stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
            timer.bind_trace(701);
            let receipt = sink.bind_for_test(&mut timer);
            timer.finish(DeliveryOutcome::Copied, None);
            assert!(
                receipt.await.unwrap().is_err(),
                "{failure:?} must report I/O error"
            );
            crate::audio::wait_for_retained_spool_registry_to_exit_for_test(
                sink.test_registry.clone(),
            )
            .await;
            assert_eq!(fs::read(sink.path()).unwrap(), prior, "{failure:?}");
        }

        let root = owned_root();
        let sink = DictationTraceSink::new(root.0.clone()).with_test_policy(
            RetentionPolicy {
                age_ms: RETENTION_MS,
                max_bytes: 1,
                max_line_bytes: MAX_LINE_BYTES,
            },
            now,
        );
        let mut timer = Metrics::default()
            .stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
        timer.bind_trace(702);
        let receipt = sink.bind_for_test(&mut timer);
        timer.finish(DeliveryOutcome::Copied, None);
        receipt.await.unwrap().unwrap();
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(sink.test_registry.clone())
            .await;
        assert_eq!(fs::metadata(sink.path()).unwrap().len(), 0);
    }

    #[test]
    fn terminal_handoff_panic_does_not_duplicate_delivery_metrics() {
        let metrics = Metrics::default();
        let mut timer =
            metrics.stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
        timer.bind_trace(703);
        timer.bind_terminal_handoff(|_, _, _| panic!("injected handoff panic"));
        timer.finish(DeliveryOutcome::PasteUnconfirmed, None);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.groups.len(), 1);
        assert_eq!(
            snapshot.groups[0]
                .stop_to_insert_by_outcome
                .iter()
                .map(|outcome| outcome.latency.sample_count)
                .sum::<usize>(),
            1
        );
    }

    #[tokio::test]
    async fn retained_write_owns_clear_and_terminal_finish_does_not_wait() {
        let _serial = TEST_LOCK.lock().await;
        let root = owned_root();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let sink = DictationTraceSink::new(root.0.clone()).with_io_gate(TestIoGate {
            entered: entered_tx,
            released: std::sync::Mutex::new(false),
            changed: std::sync::Condvar::new(),
        });
        let metrics = Metrics::default();
        let mut completions = Vec::new();
        for generation in 0..3 {
            let mut timer =
                metrics.stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
            timer.bind_trace(9 + generation);
            completions.push(sink.bind_for_test(&mut timer));
            timer.finish(DeliveryOutcome::HistoryOnly, None);
        }
        for _ in 0..3 {
            entered_rx
                .recv()
                .expect("retained worker reached file barrier");
        }
        assert!(crate::store::begin_clear_all_data(&root.0).is_err());
        assert!(!sink.path().exists());
        let mut busy_timer =
            metrics.stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
        busy_timer.bind_trace(99);
        sink.bind(&mut busy_timer);
        busy_timer.finish(DeliveryOutcome::Copied, None);
        {
            let mut released = sink.io_gate.as_ref().unwrap().released.lock().unwrap();
            *released = true;
            sink.io_gate.as_ref().unwrap().changed.notify_all();
        }
        for completion in completions {
            completion.await.unwrap().unwrap();
        }
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(sink.test_registry.clone())
            .await;
        assert!(sink.path().exists());
        let terminal_rows = fs::read_to_string(sink.path()).unwrap();
        assert_eq!(terminal_rows.lines().count(), 3);
        assert!(terminal_rows.lines().all(|line| {
            serde_json::from_str::<PersistedTrace>(line)
                .map(|record| record.session_generation != 99)
                .unwrap_or(false)
        }));
        let guard = crate::store::begin_clear_all_data(&root.0).unwrap();
        clear_all_data_under_guard(&root.0, &guard, &sink).unwrap();
        drop(guard);
        assert!(!sink.path().exists());
    }

    #[tokio::test]
    async fn terminal_handoff_returns_while_root_and_owner_admission_are_contended() {
        let _serial = TEST_LOCK.lock().await;
        let root = owned_root();
        let sink = DictationTraceSink::new(root.0.clone());
        let (entered, release, lock_thread) = crate::store::hold_root_admission_lock_for_test();
        tokio::task::spawn_blocking(move || entered.recv().unwrap())
            .await
            .unwrap();
        let mut root_busy = Metrics::default()
            .stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
        root_busy.bind_trace(720);
        sink.bind(&mut root_busy);
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let finish_thread = std::thread::spawn(move || {
            root_busy.finish(DeliveryOutcome::Copied, None);
            let _ = finished_tx.send(());
        });
        let root_finished = finished_rx.recv_timeout(std::time::Duration::from_secs(1));
        release.send(()).unwrap();
        lock_thread.join().unwrap();
        finish_thread.join().unwrap();
        assert!(
            root_finished.is_ok(),
            "root admission contention cannot block terminal callback"
        );

        let (entered, release, lock_thread) =
            crate::audio::hold_retained_registry_admission_lock_for_test(
                sink.test_registry.clone(),
            );
        tokio::task::spawn_blocking(move || entered.recv().unwrap())
            .await
            .unwrap();
        let mut owner_busy = Metrics::default()
            .stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
        owner_busy.bind_trace(721);
        sink.bind(&mut owner_busy);
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let finish_thread = std::thread::spawn(move || {
            owner_busy.finish(DeliveryOutcome::Copied, None);
            let _ = finished_tx.send(());
        });
        let owner_finished = finished_rx.recv_timeout(std::time::Duration::from_secs(1));
        release.send(()).unwrap();
        lock_thread.join().unwrap();
        finish_thread.join().unwrap();
        assert!(
            owner_finished.is_ok(),
            "retained-owner lock contention cannot block terminal callback"
        );
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(sink.test_registry.clone())
            .await;
        assert!(!sink.path().exists());
    }

    #[tokio::test]
    async fn terminal_handoff_drops_when_fixture_owner_byte_budget_is_full() {
        let _serial = TEST_LOCK.lock().await;
        let root = owned_root();
        let sink = DictationTraceSink::new(root.0.clone());
        let mutation = crate::store::begin_spool_root_mutation(&root.0).unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut reservation = crate::audio::try_enqueue_retained_spool_store_job_with_registry(
            sink.test_registry.clone(),
            "dictation-trace-byte-budget".into(),
            root.0.clone(),
            crate::audio::SPOOL_OWNER_QUEUE_BYTES,
            mutation,
            move |_: &crate::store::SpoolRootMutationLease| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            },
        )
        .unwrap();
        tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
            .await
            .unwrap();
        let mut timer = Metrics::default()
            .stop_to_insert(MetricGroup::unknown("dictation"), CancellationToken::new());
        timer.bind_trace(722);
        sink.bind(&mut timer);
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let finish_thread = std::thread::spawn(move || {
            timer.finish(DeliveryOutcome::Copied, None);
            let _ = finished_tx.send(());
        });
        let finished = finished_rx.recv_timeout(std::time::Duration::from_secs(1));
        release_tx.send(()).unwrap();
        finish_thread.join().unwrap();
        assert!(finished.is_ok(), "byte-budget rejection is nonblocking");
        reservation.wait_until_complete().await.unwrap();
        crate::audio::wait_for_retained_spool_registry_to_exit_for_test(sink.test_registry.clone())
            .await;
        assert!(!sink.path().exists());
    }
}
