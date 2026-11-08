//! One-shot, installable-Test-only Groq credential import over stdin.
//! The request is handled before Tauri starts so the final signed executable
//! itself is the Keychain item creator.

use crate::cli::TestGroqKeyImportInvocation;
#[cfg(all(target_os = "macos", not(debug_assertions)))]
use crate::keychain::TestGroqKeyStatusResult;
use crate::keychain::{
    TestCredentialAttemptDiagnostic, TestCredentialReadStatus, TestGroqKeyImportDiagnostic,
    TestGroqKeyStatus,
};
#[cfg(test)]
use crate::keychain::{TestCredentialPhase, TestCredentialWaitOutcome};
use std::io::Write;

#[cfg(any(test, all(target_os = "macos", not(debug_assertions))))]
const MAX_KEY_BYTES: usize = 512;
#[cfg(all(target_os = "macos", not(debug_assertions)))]
const STDIN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImportStatus {
    Imported,
    AlreadyConfigured,
    InvalidInvocation,
    WrongBundle,
    Busy,
    InvalidStdin,
    StdinTimeout,
    Conflict,
    KeychainUnavailable,
    VerificationUnknown,
    Unsupported,
}

impl ImportStatus {
    fn code(self) -> &'static str {
        match self {
            Self::Imported => "stored",
            Self::AlreadyConfigured => "already_configured",
            Self::InvalidInvocation => "invalid_invocation",
            Self::WrongBundle => "wrong_bundle",
            Self::Busy => "busy",
            Self::InvalidStdin => "invalid_stdin",
            Self::StdinTimeout => "stdin_timeout",
            Self::Conflict => "credential_conflict",
            Self::KeychainUnavailable => "keychain_unavailable",
            Self::VerificationUnknown => "verification_unknown",
            Self::Unsupported => "unsupported_build",
        }
    }

    fn exit_code(self) -> i32 {
        match self {
            Self::Imported | Self::AlreadyConfigured => 0,
            _ => 1,
        }
    }
}

pub(crate) fn run_if_requested(invocation: TestGroqKeyImportInvocation, bundle_identifier: &str) {
    if invocation == TestGroqKeyImportInvocation::NotRequested {
        return;
    }
    let (status, diagnostic) =
        if bundle_identifier != crate::keychain::INSTALLABLE_TEST_BUNDLE_IDENTIFIER {
            (ImportStatus::WrongBundle, None)
        } else {
            match invocation {
                TestGroqKeyImportInvocation::NotRequested => return,
                TestGroqKeyImportInvocation::InvalidInvocation => {
                    (ImportStatus::InvalidInvocation, None)
                }
                TestGroqKeyImportInvocation::Import => run_import(bundle_identifier),
            }
        };
    finish_import(status, diagnostic.as_ref())
}

pub(crate) fn run_status_if_requested(
    invocation: crate::cli::TestGroqKeyStatusInvocation,
    bundle_identifier: &str,
) {
    if invocation == crate::cli::TestGroqKeyStatusInvocation::NotRequested {
        return;
    }
    if invocation == crate::cli::TestGroqKeyStatusInvocation::InvalidInvocation {
        finish_status("invalid_invocation", None, false)
    }
    if bundle_identifier != crate::keychain::INSTALLABLE_TEST_BUNDLE_IDENTIFIER {
        finish_status("wrong_bundle", None, false)
    }
    #[cfg(not(all(target_os = "macos", not(debug_assertions))))]
    finish_status("unsupported_build", None, false);

    #[cfg(all(target_os = "macos", not(debug_assertions)))]
    {
        let _instance_lock = match crate::instance::acquire() {
            Ok(lock) => lock,
            Err(_) => finish_status("busy", None, false),
        };
        if crate::keychain::configure_runtime_bundle(bundle_identifier).is_err() {
            finish_status("wrong_namespace", None, false)
        }
        let status = crate::keychain::inspect_test_groq_key_status();
        finish_key_status(status)
    }
}

fn run_import(bundle_identifier: &str) -> (ImportStatus, Option<TestGroqKeyImportDiagnostic>) {
    #[cfg(not(all(target_os = "macos", not(debug_assertions))))]
    let _ = bundle_identifier;

    #[cfg(not(target_os = "macos"))]
    {
        (ImportStatus::Unsupported, None)
    }
    #[cfg(all(target_os = "macos", debug_assertions))]
    {
        (ImportStatus::Unsupported, None)
    }
    #[cfg(all(target_os = "macos", not(debug_assertions)))]
    {
        let _instance_lock = match crate::instance::acquire() {
            Ok(lock) => lock,
            Err(_) => return (ImportStatus::Busy, None),
        };
        if crate::keychain::configure_runtime_bundle(bundle_identifier).is_err() {
            return (ImportStatus::WrongBundle, None);
        }

        let frame = match read_stdin_frame() {
            Ok(frame) => frame,
            Err(StdinFailure::Timeout) => return (ImportStatus::StdinTimeout, None),
            Err(StdinFailure::Invalid) => return (ImportStatus::InvalidStdin, None),
        };
        let key_len = match validate_stdin_frame(&frame.0) {
            Ok(length) => length,
            Err(_) => return (ImportStatus::InvalidStdin, None),
        };
        let outcome = crate::keychain::import_test_groq_key_once_detailed(&frame.0[..key_len]);
        let status = match outcome.result {
            crate::keychain::TestGroqKeyImportResult::Stored => ImportStatus::Imported,
            crate::keychain::TestGroqKeyImportResult::AlreadyConfigured => {
                ImportStatus::AlreadyConfigured
            }
            crate::keychain::TestGroqKeyImportResult::Conflict => ImportStatus::Conflict,
            crate::keychain::TestGroqKeyImportResult::TimedOut
            | crate::keychain::TestGroqKeyImportResult::VerificationUnknown => {
                ImportStatus::VerificationUnknown
            }
            crate::keychain::TestGroqKeyImportResult::Unavailable
            | crate::keychain::TestGroqKeyImportResult::Disabled
            | crate::keychain::TestGroqKeyImportResult::WrongNamespace
            | crate::keychain::TestGroqKeyImportResult::InvalidKey => {
                ImportStatus::KeychainUnavailable
            }
            crate::keychain::TestGroqKeyImportResult::Unsupported => ImportStatus::Unsupported,
        };
        (status, Some(outcome.diagnostic))
    }
}

fn option_status(value: Option<i32>) -> String {
    value.map_or_else(|| "null".to_owned(), |status| status.to_string())
}

fn attempt_json(attempt: TestCredentialAttemptDiagnostic) -> String {
    format!(
        "{{\"add\":\"{}\",\"read\":\"{}\",\"interaction_os_status\":{},\"operation_os_status\":{}}}",
        attempt.add.code(),
        attempt.read.code(),
        option_status(attempt.interaction_os_status),
        option_status(attempt.operation_os_status)
    )
}

fn import_diagnostic_line(diagnostic: TestGroqKeyImportDiagnostic) -> String {
    format!(
        "voiceflow_test_groq_key_import_diagnostic={{\"schema\":1,\"phase\":\"{}\",\"location\":\"{}\",\"primary\":{},\"fallback\":{},\"wait\":\"{}\",\"elapsed_ms\":{}}}",
        diagnostic.phase.code(),
        diagnostic.location,
        attempt_json(diagnostic.primary),
        attempt_json(diagnostic.fallback),
        diagnostic.wait.code(),
        diagnostic.elapsed_ms
    )
}

fn finish_import(status: ImportStatus, diagnostic: Option<&TestGroqKeyImportDiagnostic>) -> ! {
    if let Some(diagnostic) = diagnostic {
        let _ = writeln!(
            std::io::stderr().lock(),
            "{}",
            import_diagnostic_line(*diagnostic)
        );
    }
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let _ = writeln!(output, "voiceflow_test_groq_key_import={}", status.code());
    let _ = output.flush();
    std::process::exit(status.exit_code());
}

fn read_status_json(status: TestCredentialReadStatus) -> String {
    format!(
        "{{\"read\":\"{}\",\"phase\":\"{}\",\"os_status\":{}}}",
        status.outcome.code(),
        status.phase.code(),
        option_status(status.os_status)
    )
}

fn status_line(state: &str, report: Option<TestGroqKeyStatus>) -> String {
    match report {
        Some(report) => format!(
            "{{\"schema\":\"voiceflow_test_groq_key_status_v1\",\"namespace\":\"installable_test\",\"state\":\"{}\",\"prior_add\":\"unknown\",\"primary\":{},\"fallback\":{},\"wait\":\"{}\",\"elapsed_ms\":{}}}",
            state,
            read_status_json(report.primary),
            read_status_json(report.fallback),
            report.wait.code(),
            report.elapsed_ms
        ),
        None => format!(
            "{{\"schema\":\"voiceflow_test_groq_key_status_v1\",\"namespace\":\"installable_test\",\"state\":\"{}\",\"prior_add\":\"unknown\"}}",
            state
        ),
    }
}

fn finish_status(state: &str, report: Option<TestGroqKeyStatus>, success: bool) -> ! {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let _ = writeln!(output, "{}", status_line(state, report));
    let _ = output.flush();
    std::process::exit(if success { 0 } else { 1 });
}

#[cfg(all(target_os = "macos", not(debug_assertions)))]
fn finish_key_status(status: TestGroqKeyStatusResult) -> ! {
    match status {
        TestGroqKeyStatusResult::Completed(report) => {
            finish_status("completed", Some(report), true)
        }
        TestGroqKeyStatusResult::WrongNamespace => finish_status("wrong_namespace", None, false),
        TestGroqKeyStatusResult::Disabled => finish_status("disabled", None, false),
        TestGroqKeyStatusResult::TimedOut => finish_status("timeout", None, false),
        TestGroqKeyStatusResult::Disconnected => finish_status("disconnected", None, false),
    }
}

#[cfg(any(test, all(target_os = "macos", not(debug_assertions))))]
#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
enum StdinFailure {
    Invalid,
    Timeout,
}

#[cfg(all(target_os = "macos", not(debug_assertions)))]
struct SecretFrame(Vec<u8>);

#[cfg(all(target_os = "macos", not(debug_assertions)))]
impl Drop for SecretFrame {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

#[cfg(any(test, all(target_os = "macos", not(debug_assertions))))]
fn validate_stdin_frame(frame: &[u8]) -> Result<usize, StdinFailure> {
    if frame.len() > MAX_KEY_BYTES + 1 || frame.last() != Some(&b'\n') {
        return Err(StdinFailure::Invalid);
    }
    let key = &frame[..frame.len() - 1];
    if key.is_empty() || !key.iter().all(u8::is_ascii_graphic) {
        return Err(StdinFailure::Invalid);
    }
    Ok(key.len())
}

#[cfg(all(target_os = "macos", not(debug_assertions)))]
fn read_stdin_frame() -> Result<SecretFrame, StdinFailure> {
    use std::time::Instant;

    if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
        return Err(StdinFailure::Invalid);
    }
    let started = Instant::now();
    let mut frame = SecretFrame(Vec::with_capacity(MAX_KEY_BYTES + 1));
    loop {
        let remaining = STDIN_DEADLINE
            .checked_sub(started.elapsed())
            .ok_or(StdinFailure::Timeout)?;
        let timeout_ms = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        let mut descriptor = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if ready == 0 {
            return Err(StdinFailure::Timeout);
        }
        if ready < 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(StdinFailure::Invalid);
        }
        if descriptor.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) == 0 {
            return Err(StdinFailure::Invalid);
        }
        let mut byte = [0_u8; 1];
        let read = unsafe { libc::read(libc::STDIN_FILENO, byte.as_mut_ptr().cast(), byte.len()) };
        if read == 0 {
            return Ok(frame);
        }
        if read < 0 {
            if matches!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EINTR) | Some(libc::EAGAIN)
            ) {
                continue;
            }
            return Err(StdinFailure::Invalid);
        }
        if frame.0.last() == Some(&b'\n') || frame.0.len() > MAX_KEY_BYTES {
            return Err(StdinFailure::Invalid);
        }
        frame.0.push(byte[0]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdin_frame_accepts_one_bounded_visible_ascii_key() {
        let key = vec![b'k'; MAX_KEY_BYTES];
        let mut frame = key;
        frame.push(b'\n');
        assert_eq!(validate_stdin_frame(&frame), Ok(MAX_KEY_BYTES));
    }

    #[test]
    fn stdin_frame_rejects_empty_unterminated_control_and_extra_input() {
        for frame in [
            b"".as_slice(),
            b"\n",
            b"secret",
            b"secret\r\n",
            b"secret\0\n",
            b"secret\nnext\n",
            b"secret\nextra",
        ] {
            assert!(validate_stdin_frame(frame).is_err());
        }
        let mut too_long = vec![b'x'; MAX_KEY_BYTES + 1];
        too_long.push(b'\n');
        assert!(validate_stdin_frame(&too_long).is_err());
    }

    #[test]
    fn import_status_is_fixed_and_contains_no_credential_data() {
        assert_eq!(ImportStatus::Imported.code(), "stored");
        assert_eq!(ImportStatus::Conflict.code(), "credential_conflict");
        assert_eq!(ImportStatus::VerificationUnknown.exit_code(), 1);
    }

    #[test]
    fn status_output_reports_only_fixed_locations_and_read_states() {
        let report = TestGroqKeyStatus {
            primary: TestCredentialReadStatus {
                outcome: crate::keychain::TestCredentialReadOutcome::ReadableNonempty,
                phase: TestCredentialPhase::Readback,
                os_status: None,
            },
            fallback: TestCredentialReadStatus {
                outcome: crate::keychain::TestCredentialReadOutcome::Unavailable,
                phase: TestCredentialPhase::InteractionPolicy,
                os_status: Some(-25293),
            },
            wait: TestCredentialWaitOutcome::Completed,
            elapsed_ms: 18,
        };
        let line = status_line("completed", Some(report));
        assert!(line.contains("voiceflow_test_groq_key_status_v1"));
        assert!(line.contains("\"prior_add\":\"unknown\""));
        assert!(line.contains("readable_nonempty"));
        assert!(line.contains("\"os_status\":-25293"));
        for forbidden in [
            "gsk-secret-sentinel",
            "secret-length",
            "secret-hash",
            "hint",
            "service",
            "account",
            "com.voiceflow.installable-test.groq-credentials.v7",
        ] {
            assert!(!line.contains(forbidden));
        }
    }

    #[test]
    fn import_diagnostic_is_fixed_and_redacts_all_keychain_identifiers() {
        let diagnostic = TestGroqKeyImportDiagnostic {
            phase: TestCredentialPhase::Readback,
            location: "primary",
            primary: TestCredentialAttemptDiagnostic {
                add: crate::keychain::TestCredentialAddOutcome::Added,
                read: crate::keychain::TestCredentialReadOutcome::Error,
                interaction_os_status: None,
                operation_os_status: Some(-25293),
            },
            fallback: TestCredentialAttemptDiagnostic::default(),
            wait: TestCredentialWaitOutcome::Completed,
            elapsed_ms: 37,
        };
        let line = import_diagnostic_line(diagnostic);
        assert!(line.contains("\"schema\":1"));
        assert!(line.contains("\"add\":\"added\""));
        assert!(line.contains("\"read\":\"error\""));
        assert!(line.contains("\"operation_os_status\":-25293"));
        for forbidden in [
            "gsk-secret-sentinel",
            "secret-length",
            "secret-hash",
            "service",
            "account",
        ] {
            assert!(!line.contains(forbidden));
        }
    }
}
