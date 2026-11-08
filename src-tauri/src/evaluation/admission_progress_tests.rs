//! Model-free production-boundary fixtures. These do not admit a fixed model/runtime.
use super::super::fixtures::FixtureDirectory;
use super::*;

#[cfg(unix)]
pub(crate) struct Sink {
    receiver: Option<std::os::unix::net::UnixDatagram>,
    pub(crate) owner: FixtureDirectory,
    pub(crate) path: PathBuf,
}
#[cfg(unix)]
impl Sink {
    pub(crate) fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        #[cfg(target_os = "macos")]
        let base = Path::new("/private/tmp");
        #[cfg(not(target_os = "macos"))]
        let base = Path::new("/tmp");
        let owner = FixtureDirectory::new_under(base);
        let path = owner.0.canonicalize().unwrap().join("p");
        let receiver = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        receiver.set_nonblocking(true).unwrap();
        use std::os::fd::AsRawFd;
        let capacity: libc::c_int = i32::from(MAX_RECORDS) * MAX_PACKET as i32;
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    receiver.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_RCVBUF,
                    (&capacity as *const libc::c_int).cast(),
                    std::mem::size_of_val(&capacity) as libc::socklen_t,
                )
            },
            0
        );
        Self {
            receiver: Some(receiver),
            owner,
            path,
        }
    }
    pub(crate) fn progress(&self) -> Progress {
        Progress::connect(Some(&self.path)).unwrap()
    }
    pub(crate) fn drain(&self) -> Vec<Record<String>> {
        let mut records = Vec::new();
        let receiver = self.receiver.as_ref().unwrap();
        for _ in 0..MAX_RECORDS {
            let mut bytes = [0; MAX_PACKET + 1];
            match receiver.recv(&mut bytes) {
                Ok(n) => {
                    records.push(decode(&bytes[..n]).expect("closed diagnostic packet grammar"))
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return records,
                Err(e) => panic!("fixture receiver: {e}"),
            }
        }
        let mut extra = [0; MAX_PACKET + 1];
        assert_eq!(
            receiver.recv(&mut extra).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        records
    }
    pub(crate) fn collect<T>(&self, action: impl FnOnce() -> T) -> (T, Vec<Record<String>>) {
        // Caller-owned bounded collector, not a producer worker. Production has no thread.
        use std::sync::atomic::{AtomicBool, Ordering};
        let stop = AtomicBool::new(false);
        let ready = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                let start = Instant::now();
                let mut records = Vec::new();
                ready.store(true, Ordering::Release);
                while start.elapsed() < std::time::Duration::from_secs(5) {
                    records.extend(self.drain());
                    assert!(records.len() <= usize::from(MAX_RECORDS));
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    std::thread::yield_now();
                }
                records.extend(self.drain());
                assert!(records.len() <= usize::from(MAX_RECORDS));
                records
            });
            let ready_deadline = Instant::now();
            while !ready.load(Ordering::Acquire) {
                assert!(
                    ready_deadline.elapsed() < std::time::Duration::from_secs(1),
                    "bounded fixture collector startup"
                );
                std::thread::yield_now();
            }
            let result = action();
            stop.store(true, Ordering::Release);
            (result, reader.join().unwrap())
        })
    }
    pub(crate) fn close_receiver(&mut self) {
        drop(self.receiver.take());
    }
}
// Drop order closes the receiver before the existing identity-owned directory cleanup.
fn decode(bytes: &[u8]) -> Result<Record<String>, &'static str> {
    if bytes.is_empty()
        || bytes.len() > MAX_PACKET
        || bytes.last() != Some(&b'\n')
        || bytes[..bytes.len() - 1].contains(&b'\n')
    {
        return Err("packet framing");
    }
    let record: Record<String> =
        serde_json::from_slice(&bytes[..bytes.len() - 1]).map_err(|_| "closed JSON schema")?;
    if record.v != 1 || !(1..=MAX_RECORDS).contains(&record.seq) {
        return Err("version or sequence");
    }
    if record
        .candidate_sha256
        .as_ref()
        .is_some_and(|s| !super::super::validation::digest(s))
        || record.input_error_code.as_ref().is_some_and(|s| {
            s.is_empty()
                || s.len() > 64
                || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
    {
        return Err("bounded identity token");
    }
    let global = matches!(
        record.event,
        Event::AdmissionBegin | Event::AdmissionEnd | Event::Truncated
    );
    if (global
        && (record.entries_declared.is_some()
            || record.entries_done.is_some()
            || record.active_entry.is_some()
            || record.declared_file_bytes.is_some()))
        || global != (record.scope == Scope::Admission)
        || (record.scope_state.is_some() != (record.event == Event::ScopeEnd))
        || (record.admission_state.is_some() != (record.event == Event::AdmissionEnd))
        || (record.diag_state.is_some()
            != matches!(record.event, Event::AdmissionEnd | Event::Truncated))
        || (record.reason.is_some() != (record.event == Event::Truncated))
        || (record.event == Event::ScopeSample && !record.scope.samples())
        || (record.input_error_code.is_some()
            != (record.admission_state == Some(AdmissionState::InputError)))
    {
        return Err("event field applicability");
    }
    Ok(record)
}
fn complete_trace(records: &[Record<String>]) -> bool {
    records
        .first()
        .is_some_and(|r| r.event == Event::AdmissionBegin)
        && records.last().is_some_and(|r| {
            r.event == Event::AdmissionEnd
                && r.diag_state == Some(DiagnosticState::ProducerEnqueueComplete)
        })
        && records
            .iter()
            .enumerate()
            .all(|(i, r)| usize::from(r.seq) == i + 1 && r.event != Event::Truncated)
        && records.windows(2).all(|w| w[0].t_ns <= w[1].t_ns)
}
#[test]
fn vf14_no_flag_is_disabled_and_returns_the_same_full_input_error() {
    let progress = Progress::connect(None).unwrap();
    assert!(progress.state.is_none());
    let original = error(
        Path::new("/caller/selected-input"),
        "sha_mismatch",
        "exact original reason",
    );
    let expected = serde_json::to_value(&original).unwrap();
    let actual = progress.finish::<()>(Err(original)).unwrap_err();
    assert_eq!(serde_json::to_value(actual).unwrap(), expected);
}
#[tokio::test]
async fn vf14_progress_cli_legality_precedes_socket_or_input_access() {
    let cases = [
        vec!["--admission-progress-socket"],
        vec!["--admission-progress-socket", "/not-a-socket"],
        vec!["--run", "--admission-progress-socket", "/not-a-socket"],
        vec![
            "--validate",
            "--admission-progress-socket",
            "/a",
            "--admission-progress-socket",
            "/b",
        ],
        vec!["--validate", "--admission-progress"],
    ];
    for args in cases {
        let result = super::super::main(args.into_iter().map(str::to_owned).collect())
            .await
            .unwrap_err();
        assert!(matches!(
            result.code(),
            "missing_argument"
                | "conflicting_arguments"
                | "duplicate_argument"
                | "unknown_argument"
        ));
        assert_eq!(result.provider_attempts(), 0);
        assert_eq!(result.exit_code(), 2);
    }
}
#[cfg(unix)]
#[test]
fn vf14_public_validate_with_no_recipe_has_no_model_or_cleanup_byte_claims() {
    let sink = Sink::new();
    std::fs::write(sink.owner.0.join("a.wav"), super::super::tests::audio()).unwrap();
    let manifest = sink.owner.write("speech.json", &serde_json::json!({"schema_version":1,"provenance":{"kind":"synthetic","note":"silent carrier; no acoustic gold","reviewed":true},"cases":[{"id":"one","audio":"a.wav","reference":"declared","expected_final":"declared","scenario":"structural","family":"general","focus_kind":"unknown","cleanup":"off"}]}));
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (result, records) = sink.collect(|| {
        runtime.block_on(super::super::main(vec![
            "--manifest".into(),
            manifest.display().to_string(),
            "--validate".into(),
            "--json".into(),
            "--admission-progress-socket".into(),
            sink.path.display().to_string(),
        ]))
    });
    assert_eq!(result.unwrap(), 0);
    assert!(complete_trace(&records));
    assert_eq!(
        records.last().unwrap().admission_state,
        Some(AdmissionState::Complete)
    );
    let counters = records.last().unwrap().cleanup.unwrap();
    assert_eq!(counters.read_bytes, 0);
    assert_eq!(counters.verified_files, 0);
    assert!(!records.iter().any(|r| r.scope == Scope::Candidate));
}
#[cfg(unix)]
#[test]
fn vf14_owned_socket_selection_rejects_alias_non_socket_permissions_and_capacity() {
    use std::os::unix::fs::PermissionsExt;
    let sink = Sink::new();
    let alias = sink.owner.0.join("alias");
    std::os::unix::fs::symlink(&sink.path, &alias).unwrap();
    for path in [
        alias,
        sink.owner.0.join("missing"),
        PathBuf::from("relative"),
        PathBuf::from(format!("/{}", "x".repeat(104))),
    ] {
        let error = match Progress::connect(Some(&path)) {
            Ok(_) => panic!("invalid receiver admitted"),
            Err(e) => e,
        };
        assert_eq!(error.code, "admission_diagnostic_unavailable");
    }
    let plain = sink.owner.0.join("regular");
    std::fs::write(&plain, b"not a socket").unwrap();
    assert!(Progress::connect(Some(&plain)).is_err());
    std::fs::set_permissions(&sink.owner.0, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Progress::connect(Some(&sink.path)).is_err());
    std::fs::set_permissions(&sink.owner.0, std::fs::Permissions::from_mode(0o700)).unwrap();
}
#[cfg(unix)]
#[test]
fn vf14_closed_sink_failure_is_secondary_to_the_same_primary_error() {
    let mut sink = Sink::new();
    let mut progress = sink.progress();
    sink.close_receiver();
    progress.scope(Scope::Inputs, |_| Ok(())).unwrap();
    let state = progress.state.as_ref().unwrap();
    assert!(!state.usable);
    let accepted = state.accepted;
    progress.scope(Scope::Preflight, |_| Ok(())).unwrap();
    assert_eq!(progress.state.as_ref().unwrap().accepted, accepted);
    let original = error(
        &sink.owner.0.join("chosen.json"),
        "schema",
        "original caller-specific reason",
    );
    let expected = serde_json::to_value(&original).unwrap();
    let actual = progress.finish::<()>(Err(original)).unwrap_err();
    assert_eq!(serde_json::to_value(actual).unwrap(), expected);
}
#[cfg(unix)]
#[test]
fn vf14_terminal_send_failure_cannot_be_validation_success_or_output_fallback() {
    let mut sink = Sink::new();
    let progress = sink.progress();
    sink.close_receiver();
    let error = progress.finish(Ok(())).unwrap_err();
    assert_eq!(error.code, "admission_diagnostic_failure");
    assert_eq!(error.artifact_path, sink.path.display().to_string());
    assert_eq!(error.provider_attempts, 0);
}
#[cfg(unix)]
#[test]
fn vf14_nonreading_real_receiver_has_bounded_attempts_or_reports_not_established() {
    let sink = Sink::new();
    let mut progress = sink.progress();
    let start = Instant::now();
    // Bound ALL offered packets, not just prefill: admission_begin + two scope
    // begins +16 legitimate samples +two scope ends +two sticky-check scope
    // records +finish <=24 production calls, each <=MAX_PACKET. Raw filler is
    // pressure only; its refusal never establishes an event refusal.
    const PRODUCTION_CALL_BOUND: usize = 24;
    let filler_limit = usize::from(MAX_RECORDS) - PRODUCTION_CALL_BOUND;
    let mut filler = Packet::new();
    filler.bytes.fill(b'x');
    filler.len = MAX_PACKET;
    let mut filler_attempts = 0usize;
    let mut filler_accepted = 0usize;
    let mut filler_refusal = None;
    progress.begin(Scope::LocalCleanupAdmission);
    progress.begin(Scope::ModelFiles);
    progress.declare_files([Some(16)]);
    progress.next_entry();
    if progress.state.as_ref().unwrap().io_failure.is_none() {
        for _ in 0..filler_limit {
            if start.elapsed() >= std::time::Duration::from_secs(1) {
                break;
            }
            filler_attempts += 1;
            match progress.state.as_ref().unwrap().sender.send(&filler) {
                Ok(()) => filler_accepted += 1,
                Err(e) => {
                    filler_refusal = Some(e);
                    break;
                }
            }
        }
    }
    if let Some(cause) = &filler_refusal {
        assert!(
            cause.kind() == io::ErrorKind::WouldBlock
                || cause.raw_os_error() == Some(libc::ENOBUFS),
            "unexpected filler errno: {cause}"
        );
    }
    // Exercise actual State::sample -> State::emit -> Sender::send. Each
    // callback crosses one of the unchanged16 buckets; no retry/cadence/quota
    // override. These are model-free counter inputs, not real file coverage.
    let mut sample_callbacks = 0usize;
    for _ in 0..16 {
        if start.elapsed() >= std::time::Duration::from_secs(1)
            || progress.state.as_ref().unwrap().io_failure.is_some()
        {
            break;
        }
        sample_callbacks += 1;
        progress.read(1);
        progress.file_hash_feed(1);
    }
    let pressure_elapsed = start.elapsed();
    let state = progress.state.as_ref().unwrap();
    let accepted = state.accepted;
    let production_refusal = state
        .io_failure
        .as_ref()
        .map(|e| (e.kind(), e.raw_os_error()));
    assert_eq!(state.normal_limit, NORMAL_RECORDS);
    assert!(accepted <= NORMAL_RECORDS);
    assert!(!state.truncated);
    assert!(sample_callbacks <= 16);
    assert!(filler_attempts + PRODUCTION_CALL_BOUND <= usize::from(MAX_RECORDS));
    assert!((filler_attempts + PRODUCTION_CALL_BOUND) * MAX_PACKET <= 128 * 1024);

    // Drain only AFTER the pressure phase, preserving actual accepted packet
    // lengths without re-creating the producer's serializer. Free queue space
    // makes the subsequent sticky check stronger than probing a still-full sink.
    let receiver = sink.receiver.as_ref().unwrap();
    let mut received_filler = 0usize;
    let mut production_packet_lengths = Vec::new();
    for _ in 0..MAX_RECORDS {
        let mut bytes = [0; MAX_PACKET + 1];
        match receiver.recv(&mut bytes) {
            Ok(n) => {
                assert!(n <= MAX_PACKET);
                if n == MAX_PACKET && bytes[..n].iter().all(|b| *b == b'x') {
                    received_filler += 1;
                } else {
                    decode(&bytes[..n]).expect("actual production packet, not assumed filler size");
                    production_packet_lengths.push(n);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) => panic!("fixture receiver: {e}"),
        }
    }
    let mut extra = [0; MAX_PACKET + 1];
    assert_eq!(
        receiver.recv(&mut extra).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    eprintln!(
        "R02-M05 observation {}",
        serde_json::json!({
            "filler_packet_bytes":filler.len,"filler_attempts":filler_attempts,
            "filler_accepted":filler_accepted,"filler_received":received_filler,
            "filler_refusal_errno":filler_refusal.as_ref().and_then(io::Error::raw_os_error),
            "sample_callbacks":sample_callbacks,"production_accepted_before_sticky":accepted,
            "production_received_packet_bytes":production_packet_lengths,
            "production_refusal_kind":production_refusal.as_ref().map(|(kind,_)|format!("{kind:?}")),
            "production_refusal_errno":production_refusal.as_ref().and_then(|(_,errno)|*errno),
            "rejected_production_packet_bytes":null,"rejected_length":"not exposed by unchanged producer; bounded by MAX_PACKET",
            "offered_packet_bound":filler_attempts+PRODUCTION_CALL_BOUND,
            "offered_byte_bound":(filler_attempts+PRODUCTION_CALL_BOUND)*MAX_PACKET,
            "pressure_phase_elapsed_ns":u64::try_from(pressure_elapsed.as_nanos()).ok()
        })
    );
    let Some((kind, errno)) = production_refusal else {
        let state = progress.state.as_ref().unwrap();
        assert!(state.usable);
        assert!(state.failure.is_none());
        eprintln!("R02-M05 production queue saturation NOT_ESTABLISHED within128KiB/1s; filler refusal is not production evidence");
        return;
    };
    assert!(
        kind == io::ErrorKind::WouldBlock || errno == Some(libc::ENOBUFS),
        "actual production rejection: kind={kind:?}, errno={errno:?}"
    );
    assert!(!progress.state.as_ref().unwrap().usable);
    assert!(matches!(
        progress.state.as_ref().unwrap().failure,
        Some(Failure::Io)
    ));
    // Only unwind/continue after the actual sticky failure is established;
    // those calls cannot send. NOT_ESTABLISHED exits without a terminal trace.
    progress.end(Scope::ModelFiles, true);
    progress.end(Scope::LocalCleanupAdmission, true);
    let mut continued = false;
    progress
        .scope(Scope::OutputResumeAdmission, |_| {
            continued = true;
            Ok(())
        })
        .unwrap();
    assert!(continued); // diagnostics must not suppress later admission work
    assert!(!progress.state.as_ref().unwrap().usable);
    assert_eq!(progress.state.as_ref().unwrap().accepted, accepted);
    assert_eq!(
        progress
            .state
            .as_ref()
            .unwrap()
            .io_failure
            .as_ref()
            .and_then(io::Error::raw_os_error),
        errno
    );
    assert_eq!(
        progress.finish(Ok(())).unwrap_err().code,
        "admission_diagnostic_failure"
    );
    assert_eq!(
        receiver.recv(&mut extra).unwrap_err().kind(),
        io::ErrorKind::WouldBlock,
        "no packets may follow actual sticky failure, even after receiver headroom is restored"
    );
}
#[cfg(unix)]
#[test]
fn vf14_quota_reserves_truncation_and_terminal_and_keeps_primary_input_result() {
    let sink = Sink::new();
    let mut progress = sink.progress();
    progress.state.as_mut().unwrap().normal_limit = 2;
    let mut did_work = false;
    let result = progress.scope(Scope::Inputs, |_| {
        did_work = true;
        Ok(())
    });
    assert!(did_work);
    assert_eq!(
        progress.finish(result).unwrap_err().code,
        "admission_diagnostic_failure"
    );
    let records = sink.drain();
    assert_eq!(records.len(), 4);
    assert_eq!(records[2].event, Event::Truncated);
    assert_eq!(records[2].reason, Some(Reason::Quota));
    assert_eq!(records[3].event, Event::AdmissionEnd);
    assert_eq!(records[3].admission_state, Some(AdmissionState::Complete));
    assert!(!complete_trace(&records));
    assert!(records
        .iter()
        .enumerate()
        .all(|(i, r)| usize::from(r.seq) == i + 1));
}
#[cfg(unix)]
#[test]
fn vf14_zero_unknown_skipped_buckets_and_counter_overflow_are_distinct() {
    let sink = Sink::new();
    let (result, records) = sink.collect(|| {
        let mut progress = sink.progress();
        let result = progress.scope(Scope::LocalCleanupAdmission, |p| {
            p.begin(Scope::FixedPins);
            p.declare_files([Some(0)]);
            p.end(Scope::FixedPins, true);
            p.begin(Scope::ModelFiles);
            p.declare_files([None]);
            p.read(16);
            p.file_hash_feed(16);
            p.end(Scope::ModelFiles, true);
            p.begin(Scope::RuntimeSources);
            p.declare_files([Some(160)]);
            p.read(80);
            p.file_hash_feed(80);
            p.read(80);
            p.file_hash_feed(80);
            p.end(Scope::RuntimeSources, true);
            Ok(())
        });
        progress.finish(result)
    });
    result.unwrap();
    assert!(complete_trace(&records));
    assert_eq!(
        records
            .iter()
            .filter(|r| r.event == Event::ScopeSample)
            .count(),
        2
    );
    assert!(records.iter().any(|r| r.scope == Scope::FixedPins
        && r.event == Event::ScopeEnd
        && r.declared_file_bytes == Some(0)));
    assert!(records.iter().any(|r| r.scope == Scope::ModelFiles
        && r.event == Event::ScopeEnd
        && r.declared_file_bytes.is_none()));
    let c = records.last().unwrap().cleanup.unwrap();
    assert_eq!(c.read_bytes, 176);
    assert_eq!(c.file_hash_feed_bytes, 176);
    assert_eq!(c.verified_file_bytes, 0);
    let sink = Sink::new();
    let mut progress = sink.progress();
    let result = progress.scope(Scope::LocalCleanupAdmission, |p| {
        p.read(u64::MAX);
        p.read(1);
        p.file_hash_feed(123);
        Ok(())
    });
    assert_eq!(
        progress.finish(result).unwrap_err().code,
        "admission_diagnostic_failure"
    );
    let records = sink.drain();
    let c = records.last().unwrap().cleanup.unwrap();
    assert_eq!(c.read_bytes, u64::MAX);
    assert_eq!(c.file_hash_feed_bytes, 0);
    assert_eq!(c.counter_state, CounterState::Invalid);
    assert!(records
        .iter()
        .any(|r| r.reason == Some(Reason::CounterOverflow)));
}
#[cfg(unix)]
#[test]
fn vf14_closed_wire_and_buffer_limits_preserve_partial_unknown_interpretation() {
    let sink = Sink::new();
    let progress = sink.progress();
    progress.finish(Ok(())).unwrap();
    let records = sink.drain();
    assert!(complete_trace(&records));
    assert!(!complete_trace(&records[..1]));
    let bytes = serde_json::to_vec(&records[1]).unwrap();
    let mut line = bytes.clone();
    line.push(b'\n');
    assert!(decode(&line).is_ok());
    assert!(decode(&bytes).is_err());
    let mut trailing = line.clone();
    trailing.extend_from_slice(b"{}\n");
    assert!(decode(&trailing).is_err());
    let json = std::str::from_utf8(&bytes).unwrap();
    for invalid in [
        json.replacen("\"v\":1", "\"v\":2", 1),
        json.replacen("{", "{\"extra\":1,", 1),
        json.replacen("{", "{\"v\":1,", 1),
        json.replace("admission_end", "unknown_event"),
        json.replacen("\"scope_state\":null,", "", 1),
    ] {
        assert!(decode(format!("{invalid}\n").as_bytes()).is_err());
    }
    let mut packet = Packet::new();
    assert!(packet.write(&[0; MAX_PACKET]).is_err());
    assert_eq!(packet.len, 0);
    packet.write_all(&[0; MAX_PACKET - 1]).unwrap();
    assert!(packet.write(b"x").is_err());
}
#[cfg(unix)]
#[test]
fn vf14_owned_sender_does_not_change_receiver_flags_and_drop_releases_its_fd() {
    use std::os::fd::AsRawFd;
    let sink = Sink::new();
    let receiver_fd = sink.receiver.as_ref().unwrap().as_raw_fd();
    let flags = unsafe { libc::fcntl(receiver_fd, libc::F_GETFL) };
    let progress = sink.progress();
    let sender_fd = progress.state.as_ref().unwrap().sender.socket.as_raw_fd();
    assert_ne!(sender_fd, receiver_fd);
    assert_ne!(
        unsafe { libc::fcntl(sender_fd, libc::F_GETFL) } & libc::O_NONBLOCK,
        0
    );
    assert_ne!(
        unsafe { libc::fcntl(sender_fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    assert_eq!(unsafe { libc::fcntl(receiver_fd, libc::F_GETFL) }, flags);
    progress.finish(Ok(())).unwrap();
    // Observe immediate closure where available; integer reuse is explicitly unknown.
    let after = unsafe { libc::fcntl(sender_fd, libc::F_GETFD) };
    if after == -1 {
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EBADF));
    } else {
        // Other parallel Rust tests may reuse the integer. It cannot establish ownership.
        eprintln!("R02-M12 numeric FD closure NOT_ESTABLISHED due possible descriptor reuse");
    }
    assert!(sink.path.exists());
    assert!(complete_trace(&sink.drain()));
}

#[cfg(unix)]
#[test]
fn vf14_fixed_serializer_rejects_oversize_without_partial_send_or_error_recursion() {
    let sink = Sink::new();
    let progress = sink.progress();
    let state = progress.state.as_ref().unwrap();
    let maximal = Record {
        v: 1,
        kind: Kind::AdmissionProgress,
        seq: MAX_RECORDS,
        t_ns: u64::MAX,
        event: Event::AdmissionEnd,
        scope: Scope::Admission,
        scope_state: None,
        entries_declared: None,
        entries_done: None,
        active_entry: None,
        declared_file_bytes: None,
        cleanup: Some(CleanupCounters {
            read_bytes: u64::MAX,
            file_hash_feed_bytes: u64::MAX,
            verified_file_bytes: u64::MAX,
            verified_files: u64::MAX,
            counter_state: CounterState::Invalid,
        }),
        admission_state: Some(AdmissionState::InputError),
        diag_state: Some(DiagnosticState::SerializationFailed),
        reason: None,
        input_error_code: Some("x".repeat(64)),
        candidate_sha256: Some("a".repeat(64)),
    };
    let borrowed = Record {
        input_error_code: maximal.input_error_code.as_deref(),
        candidate_sha256: maximal.candidate_sha256.as_deref(),
        v: maximal.v,
        kind: Kind::AdmissionProgress,
        seq: maximal.seq,
        t_ns: maximal.t_ns,
        event: maximal.event,
        scope: maximal.scope,
        scope_state: maximal.scope_state,
        entries_declared: maximal.entries_declared,
        entries_done: maximal.entries_done,
        active_entry: maximal.active_entry,
        declared_file_bytes: maximal.declared_file_bytes,
        cleanup: maximal.cleanup,
        admission_state: maximal.admission_state,
        diag_state: maximal.diag_state,
        reason: maximal.reason,
    };
    let packet = Packet::encode(&borrowed).unwrap();
    assert!(packet.len <= MAX_PACKET);
    assert!(decode(&packet.bytes[..packet.len]).is_ok());
    let huge = "x".repeat(MAX_PACKET);
    let oversized = Record {
        input_error_code: Some(huge.as_str()),
        ..borrowed
    };
    assert!(Packet::encode(&oversized).is_err());
    assert_eq!(state.accepted, 1);
    // An incompatible future primary code stays primary while the sender refuses it.
    let original = error(
        Path::new("/chosen/source"),
        "abcdefghijklmnopqrstuvwxyz_abcdefghijklmnopqrstuvwxyz_abcdefghijklm",
        "full meaningful original reason",
    );
    let expected = serde_json::to_value(&original).unwrap();
    assert_eq!(
        serde_json::to_value(progress.finish::<()>(Err(original)).unwrap_err()).unwrap(),
        expected
    );
    let records = sink.drain();
    assert_eq!(records.len(), 1); // no partial record/recursive emergency formatter
    assert!(!complete_trace(&records));
}

#[cfg(unix)]
#[test]
fn vf14_caller_delayed_drain_is_separate_from_enqueue_and_trace_completion() {
    let sink = Sink::new();
    let mut progress = sink.progress();
    progress.scope(Scope::Inputs, |_| Ok(())).unwrap();
    // Delay belongs to this explicit fixture collector, never the producer.
    std::thread::sleep(std::time::Duration::from_millis(1));
    let mut records = sink.drain();
    assert!(!complete_trace(&records));
    progress.scope(Scope::Preflight, |_| Ok(())).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1));
    records.extend(sink.drain());
    progress.finish(Ok(())).unwrap();
    records.extend(sink.drain());
    assert!(complete_trace(&records));
}
