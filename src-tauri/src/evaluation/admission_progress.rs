//! Optional, bounded admission observations. No model/runtime or input authority.
//! Kernel enqueue is distinct from receiver observation and legacy stream delivery.
use super::validation::{error, InputError};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub(super) const MAX_RECORDS: u16 = 128;
pub(super) const MAX_PACKET: usize = 1024;
const NORMAL_RECORDS: u16 = MAX_RECORDS - 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Scope {
    Admission,
    Inputs,
    Preflight,
    LocalAsrAdmission,
    LocalCleanupAdmission,
    OutputResumeAdmission,
    RepositorySource,
    Lockfiles,
    Executable,
    Platform,
    Candidate,
    FixedPins,
    ModelFiles,
    RuntimeSources,
    VersionShim,
    ExactInventories,
    WorkspaceParent,
}
impl Scope {
    fn parent(self) -> Self {
        match self {
            Self::RepositorySource | Self::Lockfiles | Self::Executable | Self::Platform => {
                Self::Preflight
            }
            Self::Candidate
            | Self::FixedPins
            | Self::ModelFiles
            | Self::RuntimeSources
            | Self::VersionShim
            | Self::ExactInventories
            | Self::WorkspaceParent => Self::LocalCleanupAdmission,
            _ => Self::Admission,
        }
    }
    fn samples(self) -> bool {
        matches!(
            self,
            Self::FixedPins | Self::ModelFiles | Self::RuntimeSources
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Event {
    AdmissionBegin,
    ScopeBegin,
    ScopeSample,
    ScopeEnd,
    Truncated,
    AdmissionEnd,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ScopeState {
    Ok,
    Error,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum AdmissionState {
    Complete,
    InputError,
    NotStarted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DiagnosticState {
    ProducerEnqueueComplete,
    QuotaTruncated,
    CounterInvalid,
    IoFailed,
    SerializationFailed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Reason {
    Quota,
    CounterOverflow,
    StateError,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CounterState {
    Valid,
    Invalid,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CleanupCounters {
    pub read_bytes: u64,
    pub file_hash_feed_bytes: u64,
    pub verified_file_bytes: u64,
    pub verified_files: u64,
    pub counter_state: CounterState,
}
impl Default for CleanupCounters {
    fn default() -> Self {
        Self {
            read_bytes: 0,
            file_hash_feed_bytes: 0,
            verified_file_bytes: 0,
            verified_files: 0,
            counter_state: CounterState::Valid,
        }
    }
}
#[derive(Serialize, Deserialize)]
pub(super) enum Kind {
    #[serde(rename = "vf14_admission_progress")]
    AdmissionProgress,
}
fn required_option<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(d)
}
/// One closed wire representation; the sender borrows strings and fixtures decode owned strings.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, bound(deserialize = "S: Deserialize<'de>"))]
pub(super) struct Record<S> {
    pub v: u8,
    pub kind: Kind,
    pub seq: u16,
    pub t_ns: u64,
    pub event: Event,
    pub scope: Scope,
    #[serde(deserialize_with = "required_option")]
    pub scope_state: Option<ScopeState>,
    #[serde(deserialize_with = "required_option")]
    pub entries_declared: Option<u64>,
    #[serde(deserialize_with = "required_option")]
    pub entries_done: Option<u64>,
    #[serde(deserialize_with = "required_option")]
    pub active_entry: Option<u64>,
    #[serde(deserialize_with = "required_option")]
    pub declared_file_bytes: Option<u64>,
    #[serde(deserialize_with = "required_option")]
    pub cleanup: Option<CleanupCounters>,
    #[serde(deserialize_with = "required_option")]
    pub admission_state: Option<AdmissionState>,
    #[serde(deserialize_with = "required_option")]
    pub diag_state: Option<DiagnosticState>,
    #[serde(deserialize_with = "required_option")]
    pub reason: Option<Reason>,
    #[serde(deserialize_with = "required_option")]
    pub input_error_code: Option<S>,
    #[serde(deserialize_with = "required_option")]
    pub candidate_sha256: Option<S>,
}
struct Packet {
    bytes: [u8; MAX_PACKET],
    len: usize,
}
impl Packet {
    fn new() -> Self {
        Self {
            bytes: [0; MAX_PACKET],
            len: 0,
        }
    }
    fn encode(record: &Record<&str>) -> io::Result<Self> {
        let mut packet = Self::new();
        serde_json::to_writer(&mut packet, record).map_err(io::Error::other)?;
        packet.bytes[packet.len] = b'\n';
        packet.len += 1;
        Ok(packet)
    }
}
impl Write for Packet {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > (MAX_PACKET - 1).saturating_sub(self.len) {
            return Err(io::Error::other("diagnostic record exceeds fixed buffer"));
        }
        self.bytes[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[derive(Clone, Copy)]
struct Group {
    scope: Scope,
    entries_declared: Option<u64>,
    entries_done: Option<u64>,
    active_entry: Option<u64>,
    declared_file_bytes: Option<u64>,
    read_start: u64,
    bucket: u8,
}
struct Sender {
    #[cfg(unix)]
    socket: std::os::unix::net::UnixDatagram,
}
impl Sender {
    #[cfg(unix)]
    fn connect(path: &Path) -> Result<Self, InputError> {
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
        let fail = |detail: String| error(path, "admission_diagnostic_unavailable", detail);
        let raw = path.as_os_str().as_bytes();
        // Use the actual platform structure, rather than a portable guessed path cap.
        let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        if !path.is_absolute()
            || path.to_str().is_none()
            || raw.contains(&0)
            || raw.len() >= address.sun_path.len()
        {
            return Err(fail(
                "select an absolute socket path fitting sockaddr_un including NUL".into(),
            ));
        }
        let canonical = super::validation::external(path)
            .map_err(|e| fail(format!("receiver selection: {e}")))?;
        let meta =
            std::fs::symlink_metadata(path).map_err(|e| fail(format!("receiver metadata: {e}")))?;
        let parent = path
            .parent()
            .ok_or_else(|| fail("receiver parent required".into()))?;
        let parent_meta =
            std::fs::symlink_metadata(parent).map_err(|e| fail(format!("receiver parent: {e}")))?;
        if canonical != path
            || !meta.file_type().is_socket()
            || !parent_meta.is_dir()
            || parent_meta.permissions().mode() & 0o777 != 0o700
            || parent_meta.uid() != unsafe { libc::geteuid() }
            || meta.uid() != parent_meta.uid()
        {
            return Err(fail(
                "select a canonical owned socket under a caller-owned0700 directory".into(),
            ));
        }
        let socket = std::os::unix::net::UnixDatagram::unbound()
            .map_err(|e| fail(format!("sender creation: {e}")))?;
        socket
            .set_nonblocking(true)
            .map_err(|e| fail(format!("sender nonblocking: {e}")))?;
        let fd = socket.as_raw_fd();
        // Only the fresh sender's flags/options are touched. Never mutate inherited FDs.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(fail(format!(
                "sender close-on-exec: {}",
                io::Error::last_os_error()
            )));
        }
        #[cfg(target_os = "macos")]
        {
            let one: libc::c_int = 1;
            if unsafe {
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_NOSIGPIPE,
                    (&one as *const libc::c_int).cast(),
                    std::mem::size_of_val(&one) as libc::socklen_t,
                )
            } == -1
            {
                return Err(fail(format!(
                    "sender no-SIGPIPE: {}",
                    io::Error::last_os_error()
                )));
            }
        }
        socket
            .connect(path)
            .map_err(|e| fail(format!("nonblocking receiver connect: {e}")))?;
        Ok(Self { socket })
    }
    #[cfg(not(unix))]
    fn connect(path: &Path) -> Result<Self, InputError> {
        Err(error(
            path,
            "admission_diagnostic_unavailable",
            "Unix datagram diagnostics unsupported on this platform",
        ))
    }
    fn send(&self, packet: &Packet) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // One syscall, including EINTR/EAGAIN: do not use write_all or retry.
            let n = unsafe {
                libc::send(
                    self.socket.as_raw_fd(),
                    packet.bytes.as_ptr().cast(),
                    packet.len,
                    libc::MSG_DONTWAIT,
                )
            };
            if n == -1 {
                return Err(io::Error::last_os_error());
            }
            if n as usize != packet.len {
                return Err(io::Error::other("short diagnostic datagram send"));
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = packet;
            Err(io::Error::other("unsupported diagnostic sender"))
        }
    }
}
#[derive(Clone, Copy)]
enum Failure {
    Quota,
    Counter,
    State,
    Serialization,
    Io,
}
struct State {
    sender: Sender,
    path: PathBuf,
    started: Instant,
    accepted: u16,
    normal_limit: u16,
    failure: Option<Failure>,
    io_failure: Option<io::Error>,
    usable: bool,
    truncated: bool,
    stack: [Option<Group>; 2],
    depth: usize,
    seen: u32,
    cleanup: Option<CleanupCounters>,
    candidate: Option<[u8; 64]>,
}
impl State {
    fn current(&self) -> Option<Group> {
        self.depth.checked_sub(1).and_then(|i| self.stack[i])
    }
    fn status(&self) -> DiagnosticState {
        if self.io_failure.is_some() {
            return DiagnosticState::IoFailed;
        }
        match self.failure {
            None => DiagnosticState::ProducerEnqueueComplete,
            Some(Failure::Quota) => DiagnosticState::QuotaTruncated,
            Some(Failure::Counter | Failure::State) => DiagnosticState::CounterInvalid,
            Some(Failure::Serialization) => DiagnosticState::SerializationFailed,
            Some(Failure::Io) => DiagnosticState::IoFailed,
        }
    }
    fn emit(
        &mut self,
        event: Event,
        scope_state: Option<ScopeState>,
        terminal: Option<(AdmissionState, Option<&'static str>)>,
        reason: Option<Reason>,
    ) {
        if !self.usable {
            return;
        }
        let reserved = matches!(event, Event::Truncated | Event::AdmissionEnd);
        if !reserved && self.failure.is_some() {
            return;
        }
        if !reserved && self.accepted >= self.normal_limit {
            self.truncate(Reason::Quota);
            return;
        }
        let Some(seq) = self.accepted.checked_add(1).filter(|s| *s <= MAX_RECORDS) else {
            self.failure.get_or_insert(Failure::State);
            self.usable = false;
            return;
        };
        let Ok(t_ns) = u64::try_from(self.started.elapsed().as_nanos()) else {
            self.failure.get_or_insert(Failure::Counter);
            self.usable = false;
            return;
        };
        let group = if matches!(
            event,
            Event::AdmissionBegin | Event::AdmissionEnd | Event::Truncated
        ) {
            None
        } else {
            self.current()
        };
        let input_error_code = terminal.and_then(|(_, code)| code);
        if input_error_code.is_some_and(|s| {
            s.is_empty()
                || s.len() > 64
                || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        }) {
            self.failure.get_or_insert(Failure::Serialization);
            self.usable = false;
            return;
        }
        let record = Record {
            v: 1,
            kind: Kind::AdmissionProgress,
            seq,
            t_ns,
            event,
            scope: group.map_or(Scope::Admission, |g| g.scope),
            scope_state,
            entries_declared: group.and_then(|g| g.entries_declared),
            entries_done: group.and_then(|g| g.entries_done),
            active_entry: group.and_then(|g| g.active_entry),
            declared_file_bytes: group.and_then(|g| g.declared_file_bytes),
            cleanup: self.cleanup,
            admission_state: terminal.map(|(s, _)| s),
            diag_state: (terminal.is_some() || event == Event::Truncated).then(|| self.status()),
            reason,
            input_error_code,
            candidate_sha256: self
                .candidate
                .as_ref()
                .and_then(|s| std::str::from_utf8(s).ok()),
        };
        let packet = match Packet::encode(&record) {
            Ok(packet) => packet,
            Err(_) => {
                self.failure.get_or_insert(Failure::Serialization);
                self.usable = false;
                return;
            }
        };
        match self.sender.send(&packet) {
            Ok(()) => self.accepted = seq,
            Err(e) => {
                self.failure.get_or_insert(Failure::Io);
                self.io_failure = Some(e);
                self.usable = false;
            }
        }
    }
    fn truncate(&mut self, reason: Reason) {
        self.failure.get_or_insert(match reason {
            Reason::Quota => Failure::Quota,
            Reason::CounterOverflow => Failure::Counter,
            Reason::StateError => Failure::State,
        });
        if reason != Reason::Quota {
            if let Some(c) = &mut self.cleanup {
                c.counter_state = CounterState::Invalid;
            }
        }
        if !self.truncated {
            self.truncated = true;
            self.emit(Event::Truncated, None, None, Some(reason));
        }
    }
    fn begin(&mut self, scope: Scope) {
        let parent = scope.parent();
        let peers = match parent {
            Scope::Preflight => ((1u32 << 10) - 1) ^ ((1u32 << 6) - 1),
            Scope::LocalCleanupAdmission => ((1u32 << 17) - 1) ^ ((1u32 << 10) - 1),
            _ => (1u32 << 6) - 2,
        };
        if scope == Scope::Admission
            || self.depth >= self.stack.len()
            || (self.seen & peers) >= (1u32 << scope as u32)
            || self.current().map_or(Scope::Admission, |g| g.scope) != scope.parent()
            || self.seen & (1 << scope as u32) != 0
        {
            self.truncate(Reason::StateError);
            return;
        }
        self.seen |= 1 << scope as u32;
        if scope == Scope::LocalCleanupAdmission {
            self.cleanup = Some(CleanupCounters::default());
        }
        self.stack[self.depth] = Some(Group {
            scope,
            entries_declared: None,
            entries_done: None,
            active_entry: None,
            declared_file_bytes: None,
            read_start: self.cleanup.map_or(0, |c| c.read_bytes),
            bucket: 0,
        });
        self.depth += 1;
        self.emit(Event::ScopeBegin, None, None, None);
    }
    fn end(&mut self, scope: Scope, ok: bool) {
        if self.current().is_none_or(|g| g.scope != scope) {
            self.truncate(Reason::StateError);
            return;
        }
        self.emit(
            Event::ScopeEnd,
            Some(if ok {
                ScopeState::Ok
            } else {
                ScopeState::Error
            }),
            None,
            None,
        );
        self.depth -= 1;
        self.stack[self.depth] = None;
    }
    fn add(&mut self, update: impl FnOnce(&mut CleanupCounters) -> Option<()>) {
        if let Some(mut next) = self.cleanup {
            if next.counter_state == CounterState::Invalid {
                return;
            }
            if update(&mut next).is_none() {
                self.truncate(Reason::CounterOverflow);
            } else {
                self.cleanup = Some(next);
            }
        }
    }
    fn sample(&mut self) {
        let Some(group) = self.current().filter(|g| g.scope.samples()) else {
            return;
        };
        let Some(total) = group.declared_file_bytes.filter(|n| *n != 0) else {
            return;
        };
        let Some(c) = self
            .cleanup
            .filter(|c| c.counter_state == CounterState::Valid)
        else {
            return;
        };
        let Some(read) = c.read_bytes.checked_sub(group.read_start) else {
            self.truncate(Reason::CounterOverflow);
            return;
        };
        let bucket = ((u128::from(read) * 16 / u128::from(total)).min(16)) as u8;
        if bucket > group.bucket {
            if let Some(g) = &mut self.stack[self.depth - 1] {
                g.bucket = bucket;
            }
            self.emit(Event::ScopeSample, None, None, None);
        }
    }
    fn error(&self) -> InputError {
        let label = match self.failure {
            Some(Failure::Quota) => "quota_truncated",
            Some(Failure::Counter) => "counter_overflow",
            Some(Failure::State) => "state_error",
            Some(Failure::Serialization) => "serialization_failed",
            Some(Failure::Io) => "io_failed",
            None => "unknown",
        };
        let detail = self.io_failure.as_ref().map_or_else(
            || label.to_owned(),
            |e| format!("{label}; nonblocking diagnostic send: {e}"),
        );
        error(&self.path, "admission_diagnostic_failure", detail)
    }
}
/// Disabled observations allocate no sender/clock/buffer. No diagnostic state controls admission.
#[derive(Default)]
pub(super) struct Progress {
    state: Option<State>,
}
impl Progress {
    pub fn disabled() -> Self {
        Self::default()
    }
    pub fn connect(path: Option<&Path>) -> Result<Self, InputError> {
        let Some(path) = path else {
            return Ok(Self::disabled());
        };
        let sender = Sender::connect(path)?;
        let mut progress = Self {
            state: Some(State {
                sender,
                path: path.to_owned(),
                started: Instant::now(),
                accepted: 0,
                normal_limit: NORMAL_RECORDS,
                failure: None,
                io_failure: None,
                usable: true,
                truncated: false,
                stack: [None; 2],
                depth: 0,
                seen: 0,
                cleanup: None,
                candidate: None,
            }),
        };
        if let Some(s) = &mut progress.state {
            s.emit(Event::AdmissionBegin, None, None, None);
        }
        Ok(progress)
    }
    pub fn begin(&mut self, scope: Scope) {
        if let Some(s) = &mut self.state {
            s.begin(scope);
        }
    }
    pub fn end(&mut self, scope: Scope, ok: bool) {
        if let Some(s) = &mut self.state {
            s.end(scope, ok);
        }
    }
    pub fn scope<T>(
        &mut self,
        scope: Scope,
        action: impl FnOnce(&mut Self) -> Result<T, InputError>,
    ) -> Result<T, InputError> {
        let depth = self.state.as_ref().map_or(0, |s| s.depth);
        self.begin(scope);
        let result = action(self);
        if let Some(s) = &mut self.state {
            while s.depth > depth {
                let current = s.current().expect("owned open diagnostic scope").scope;
                s.end(current, result.is_ok());
            }
        }
        result
    }
    pub fn declare_files(&mut self, lengths: impl IntoIterator<Item = Option<u64>>) {
        let Some(s) = &mut self.state else {
            return;
        };
        let mut count = 0u64;
        let mut bytes = Some(0u64);
        for length in lengths {
            let Some(next) = count.checked_add(1) else {
                s.truncate(Reason::CounterOverflow);
                return;
            };
            count = next;
            bytes = match (bytes, length) {
                (Some(a), Some(b)) => match a.checked_add(b) {
                    Some(n) => Some(n),
                    None => {
                        s.truncate(Reason::CounterOverflow);
                        return;
                    }
                },
                _ => None,
            };
        }
        if let Some(g) = s.depth.checked_sub(1).and_then(|i| s.stack[i].as_mut()) {
            g.entries_declared = Some(count);
            g.entries_done = Some(0);
            g.declared_file_bytes = bytes;
        }
    }
    pub fn next_entry(&mut self) {
        if let Some(s) = &mut self.state {
            if let Some(g) = s.depth.checked_sub(1).and_then(|i| s.stack[i].as_mut()) {
                match g.active_entry.unwrap_or(0).checked_add(1) {
                    Some(n) => g.active_entry = Some(n),
                    None => s.truncate(Reason::CounterOverflow),
                }
            }
        }
    }
    pub fn entry_done(&mut self) {
        if let Some(s) = &mut self.state {
            if let Some(g) = s.depth.checked_sub(1).and_then(|i| s.stack[i].as_mut()) {
                match g.entries_done.unwrap_or(0).checked_add(1) {
                    Some(n) => g.entries_done = Some(n),
                    None => s.truncate(Reason::CounterOverflow),
                }
            }
        }
    }
    pub fn read(&mut self, bytes: u64) {
        if let Some(s) = &mut self.state {
            s.add(|c| {
                c.read_bytes = c.read_bytes.checked_add(bytes)?;
                Some(())
            });
        }
    }
    pub fn file_hash_feed(&mut self, bytes: u64) {
        if let Some(s) = &mut self.state {
            s.add(|c| {
                c.file_hash_feed_bytes = c.file_hash_feed_bytes.checked_add(bytes)?;
                Some(())
            });
            s.sample();
        }
    }
    pub fn verified_file(&mut self, bytes: u64) {
        if let Some(s) = &mut self.state {
            s.add(|c| {
                c.verified_file_bytes = c.verified_file_bytes.checked_add(bytes)?;
                c.verified_files = c.verified_files.checked_add(1)?;
                Some(())
            });
        }
        self.entry_done();
    }
    pub fn candidate(&mut self, hash: &str) {
        if let Some(s) = &mut self.state {
            if super::validation::digest(hash) {
                let mut value = [0; 64];
                value.copy_from_slice(hash.as_bytes());
                s.candidate = Some(value);
            } else {
                s.truncate(Reason::StateError);
            }
        }
    }
    pub fn finish<T>(mut self, result: Result<T, InputError>) -> Result<T, InputError> {
        if let Some(s) = &mut self.state {
            if s.depth != 0 {
                s.truncate(Reason::StateError);
            }
            let outcome = if result.is_ok() {
                AdmissionState::Complete
            } else {
                AdmissionState::InputError
            };
            let code = result.as_ref().err().map(|e| e.code);
            s.emit(Event::AdmissionEnd, None, Some((outcome, code)), None);
            // Never replace an obtained InputError with optional-observer failure.
            if result.is_ok() && s.failure.is_some() {
                return Err(s.error());
            }
        }
        result
    }
}

#[cfg(test)]
#[path = "admission_progress_tests.rs"]
pub(super) mod tests;
