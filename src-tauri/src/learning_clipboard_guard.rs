//! Research-only supervisor for clipboard recovery component probes.
//!
//! The supervisor is a separate process from its fixed harness child. The
//! pasteboard snapshot and ownership proof stay in the supervisor; the child
//! receives only an opaque lease ID and fixed operation codes over an
//! inherited anonymous socketpair.

use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PROTOCOL_VERSION: u8 = 1;
const CHILD_CONTROL_FD: RawFd = 3;
const MAX_FRAME_BYTES: usize = 8_192;
const MAX_UNKNOWN_HOLD_COMMAND_BYTES: usize = 128;
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);
const EOF_CHILD_ALIVE_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClipboardSeedFixtureV1 {
    Empty,
    OneItemMultiFlavor,
    MultipleItems,
    Over128MiB,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClipboardWriteFixtureV1 {
    RegisteredTemporaryTextV1,
    ExternalCopyV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RemoteClipboardBoardV1 {
    NamedFixture,
    GeneralClipboard,
}

/// Fixed owner outcomes only. This type never carries clipboard bytes,
/// original-content hashes, or the ownership token.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClipboardGuardReplyV1 {
    CaptureOverBudget,
    CaptureUnknown,
    NotWritten,
    WriteOwned,
    WritePartialUnknown,
    ReadUnknown,
    RestoreRequested,
    Restored,
    OwnershipChanged,
    RestoreUnknown,
    CloseAcknowledged,
    Rejected,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RegisteredNamedRemoteClipboardCaseV1 {
    Positive,
    WriteAckLost,
    PermitAckLost,
    PermitDenied,
    EofChildAlive,
    PrewriteExternalCopy,
    WrongSequence,
    WrongWriteId,
    ReplayWrite,
}

impl RegisteredNamedRemoteClipboardCaseV1 {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "positive" => Some(Self::Positive),
            "write_ack_lost" => Some(Self::WriteAckLost),
            "permit_ack_lost" => Some(Self::PermitAckLost),
            "permit_denied" => Some(Self::PermitDenied),
            "eof_child_alive" => Some(Self::EofChildAlive),
            "prewrite_external_copy" => Some(Self::PrewriteExternalCopy),
            "wrong_sequence" => Some(Self::WrongSequence),
            "wrong_write_id" => Some(Self::WrongWriteId),
            "replay_write" => Some(Self::ReplayWrite),
            _ => None,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::Positive => "positive",
            Self::WriteAckLost => "write_ack_lost",
            Self::PermitAckLost => "permit_ack_lost",
            Self::PermitDenied => "permit_denied",
            Self::EofChildAlive => "eof_child_alive",
            Self::PrewriteExternalCopy => "prewrite_external_copy",
            Self::WrongSequence => "wrong_sequence",
            Self::WrongWriteId => "wrong_write_id",
            Self::ReplayWrite => "replay_write",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClipboardGuardCheckStageV1 {
    MarkerAfterWrite,
    PreBoundary,
    PostCallback,
    PreRestore,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ClipboardGuardOperationV1 {
    CaptureFreshUnchanged,
    CheckUnchangedSinceCapture,
    WriteRegisteredTemporaryText,
    CheckCurrent {
        write_id: u64,
        stage: ClipboardGuardCheckStageV1,
        child_callback_count: u8,
    },
    PermitShortcutPost {
        write_id: u64,
    },
    ApplyDeclaredEdit {
        write_id: u64,
        confirmed_insertion: RegisteredOwnedPasteConfirmationV1,
        baseline: RegisteredOwnedPasteBaselineV1,
    },
    RequestRestoreIfOwned {
        write_id: u64,
    },
    Close {
        child_callback_count: u8,
        shortcut_sent: bool,
        child_restore: RemoteProbeChildRestoreV1,
    },
    CloseRegisteredOwnedPasteSession {
        write_id: Option<u64>,
        shortcut_sent: bool,
        result: RegisteredOwnedPasteChildResultKindV1,
        baseline: RegisteredOwnedPasteBaselineV1,
        final_sample: RegisteredOwnedPasteFinalSampleV1,
        edit_response_received: bool,
        observation_ticket_released: bool,
        owner_joined: bool,
        child_restore: RemoteProbeChildRestoreV1,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ClipboardGuardRpcOutcomeV1 {
    Ready {
        case: RegisteredNamedRemoteClipboardCaseV1,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        board: Option<RemoteClipboardBoardV1>,
    },
    SnapshotUnchanged,
    NotWritten,
    WriteOwned {
        write_id: u64,
    },
    CurrentOwned,
    WritePartialUnknown,
    OwnershipChanged,
    ReadUnknown,
    PostPermitRecordedOnly {
        parent_permit_count: u8,
    },
    PostPermitAllowed {
        parent_permit_count: u8,
    },
    PermitDenied,
    RegisteredOwnedEditObserved {
        result: RegisteredOwnedPasteEditResultV1,
    },
    RestoreDeferred,
    RestoreUnknown,
    CloseAcknowledged,
    RegisteredOwnedPasteSessionCloseAcknowledged,
    Rejected,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ClipboardGuardRpcBodyV1 {
    Request {
        operation: ClipboardGuardOperationV1,
    },
    Reply {
        outcome: ClipboardGuardRpcOutcomeV1,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClipboardGuardRpcFrameV1 {
    pub(crate) protocol_version: u8,
    pub(crate) lease_id: u64,
    pub(crate) sequence: u8,
    pub(crate) body: ClipboardGuardRpcBodyV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClipboardGuardClientErrorV1 {
    Io,
    FrameOverBudget,
    MalformedFrame,
    VersionMismatch,
    LeaseMismatch,
    SequenceMismatch,
    UnexpectedReply,
    AcknowledgementUnavailable,
    Closed,
}

pub(crate) trait ClipboardGuardRpcClientV1: Send + Sync {
    fn request(
        &self,
        lease_id: u64,
        operation: ClipboardGuardOperationV1,
    ) -> Result<ClipboardGuardRpcOutcomeV1, ClipboardGuardClientErrorV1>;

    fn child_restore_state(&self) -> RemoteProbeChildRestoreV1 {
        RemoteProbeChildRestoreV1::Unknown
    }

    fn registered_write_id(&self) -> Option<u64> {
        None
    }
}

pub(crate) struct RemoteProbeChildInputV1 {
    pub(crate) client: Arc<dyn ClipboardGuardRpcClientV1>,
    pub(crate) lease_id: u64,
    pub(crate) case: RegisteredNamedRemoteClipboardCaseV1,
    pub(crate) board: RemoteClipboardBoardV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegisteredOwnedPasteHostIdentityV1 {
    pub(crate) source: String,
    pub(crate) pid: i32,
    pub(crate) parent_pid: i32,
    pub(crate) user_id: u32,
    pub(crate) start_seconds: String,
    pub(crate) start_microseconds: String,
    pub(crate) executable_path: String,
}

/// Sent only in the fixed self-exec child Ready frame. This grants one
/// registered paste/edit/read session; it is not a General/Named recorder
/// permit and contains no clipboard content or ownership proof.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegisteredOwnedPasteSessionAuthorityV1 {
    pub(crate) plan_id: String,
    pub(crate) phase_id: String,
    pub(crate) operation_id: String,
    pub(crate) supervisor_pid: i32,
    pub(crate) helper_pid: i32,
    pub(crate) host_identity: RegisteredOwnedPasteHostIdentityV1,
    pub(crate) binding: crate::learning_research::OwnedPasteSessionBindingV1,
    /// CLOCK_MONOTONIC deadline shared by the supervisor and its self-exec
    /// child. A relative duration would start a fresh lease after handoff.
    pub(crate) expires_at_monotonic_ns: u64,
}

/// Separate sequence-zero handshake for the registered owned-paste child.
/// The established recorder-only frame schema remains byte-for-byte intact.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegisteredOwnedPasteSessionReadyFrameV1 {
    pub(crate) protocol_version: u8,
    pub(crate) lease_id: u64,
    pub(crate) sequence: u8,
    pub(crate) body: RegisteredOwnedPasteSessionReadyBodyV1,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RegisteredOwnedPasteSessionReadyBodyV1 {
    Ready {
        authority: RegisteredOwnedPasteSessionAuthorityV1,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RegisteredOwnedPasteBaselineStateV1 {
    ObservedExpectedPostInsert,
    Unknown,
    Mismatch,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegisteredOwnedPasteBaselineV1 {
    pub(crate) state: RegisteredOwnedPasteBaselineStateV1,
    pub(crate) has_value: bool,
    pub(crate) sha256: [u8; 32],
    pub(crate) byte_length: usize,
    pub(crate) utf16_length: usize,
    pub(crate) selection_start: usize,
    pub(crate) selection_length: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RegisteredOwnedPasteVerificationV1 {
    Verified,
    Unverified,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegisteredOwnedPasteConfirmationV1 {
    pub(crate) verification: RegisteredOwnedPasteVerificationV1,
    pub(crate) target_pid: i32,
    pub(crate) target_bundle_matches: bool,
    pub(crate) source_field_utf16_length: u32,
    pub(crate) inserted_field_utf16_length: u32,
    pub(crate) replaced_range_start: u32,
    pub(crate) replaced_range_length: u32,
    pub(crate) inserted_range_start: u32,
    pub(crate) inserted_range_length: u32,
    pub(crate) inserted_text_utf16_length: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RegisteredOwnedPasteEditStateV1 {
    Observed,
    Changed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RegisteredOwnedPasteComparisonV1 {
    Match,
    Mismatch,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RegisteredOwnedPasteChildResultKindV1 {
    Completed,
    ProtocolRejected,
    PasteRejected,
    BaselineMismatch,
    EditUnknown,
    FinalReadUnknown,
    OwnerUnknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RegisteredOwnedPasteFinalSampleStateV1 {
    MatchesIndependentAxAndExpected,
    MatchesIndependentAxExpectedMismatch,
    ObservedDifferentOrUnbound,
    Unknown,
    NotRun,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegisteredOwnedPasteFinalSampleV1 {
    pub(crate) state: RegisteredOwnedPasteFinalSampleStateV1,
    pub(crate) has_value: bool,
    pub(crate) sha256: [u8; 32],
    pub(crate) byte_length: usize,
    pub(crate) utf16_length: usize,
    pub(crate) focus_matches: bool,
}

/// A hash/length-only projection of the helper's independent AX sample.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegisteredOwnedPasteEditResultV1 {
    pub(crate) state: RegisteredOwnedPasteEditStateV1,
    pub(crate) comparison: RegisteredOwnedPasteComparisonV1,
    pub(crate) source: RegisteredOwnedPasteEditSourceV1,
    pub(crate) has_value: bool,
    pub(crate) sha256: [u8; 32],
    pub(crate) byte_length: usize,
    pub(crate) has_edit_receipt: bool,
    pub(crate) programmatic_edit_receipt_sha256: [u8; 32],
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RegisteredOwnedPasteEditSourceV1 {
    AxTargetReaderOwnedDeclaredTargetV1,
    Unknown,
}

#[derive(Clone)]
pub(crate) struct RegisteredOwnedPasteSessionRemoteContextV1 {
    pub(crate) client: Arc<dyn ClipboardGuardRpcClientV1>,
    pub(crate) lease_id: u64,
    pub(crate) plan_id: String,
    pub(crate) phase_id: String,
    pub(crate) operation_id: String,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
#[derive(Clone)]
pub(crate) struct RegisteredOwnedPasteSessionChildInputV1 {
    pub(crate) remote: RegisteredOwnedPasteSessionRemoteContextV1,
    pub(crate) authority: RegisteredOwnedPasteSessionAuthorityV1,
    pub(crate) host_identity: crate::paste::RegisteredResearchHostIdentityV1,
    pub(crate) expires_at: std::time::Instant,
    pub(crate) cancellation: tokio_util::sync::CancellationToken,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RegisteredOwnedPasteParentCallbackV1 {
    RecheckPostAuthority,
    ApplyDeclaredEdit,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RegisteredOwnedPasteParentCallbackResultV1 {
    HostAndHelperLive,
    HostOrHelperUnavailable,
    Edit(RegisteredOwnedPasteEditResultV1),
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RegisteredOwnedPasteParentStateV1 {
    Complete,
    ChildStartFailed,
    ReadySendFailed,
    PrewriteOwnershipChanged,
    PermitDenied,
    Rejected,
    AcknowledgementUnavailable,
    Unknown,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegisteredOwnedPasteSessionParentReportV1 {
    pub(crate) state: RegisteredOwnedPasteParentStateV1,
    pub(crate) child_started: bool,
    pub(crate) child_exit_confirmed: bool,
    pub(crate) child_exit_status: Option<i32>,
    pub(crate) operation_count: u8,
    pub(crate) parent_write_attempt_count: u8,
    pub(crate) write_proof_saved_before_ack: bool,
    pub(crate) parent_permit_count: u8,
    pub(crate) parent_may_have_posted: bool,
    pub(crate) parent_edit_attempt_count: u8,
    pub(crate) parent_edit_result: RegisteredOwnedPasteEditResultV1,
    pub(crate) write_id: Option<u64>,
    pub(crate) child_baseline: RegisteredOwnedPasteBaselineV1,
    pub(crate) child_final_sample: RegisteredOwnedPasteFinalSampleV1,
    pub(crate) child_edit_response_received: bool,
    pub(crate) child_observation_ticket_released: bool,
    pub(crate) child_owner_joined: bool,
    pub(crate) child_result: RegisteredOwnedPasteChildResultKindV1,
    pub(crate) child_close_received: bool,
    pub(crate) child_shortcut_sent: bool,
    pub(crate) child_restore: RemoteProbeChildRestoreV1,
    pub(crate) parent_restore_deferred_ack_sent: bool,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
pub(crate) fn monotonic_deadline_ns(expires_at: std::time::Instant) -> Option<u64> {
    let now = monotonic_now_ns()?;
    let remaining = expires_at.saturating_duration_since(std::time::Instant::now());
    let remaining_ns = u64::try_from(remaining.as_nanos()).ok()?;
    now.checked_add(remaining_ns)
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn monotonic_now_ns() -> Option<u64> {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is a valid writable timespec for clock_gettime.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) } != 0
        || now.tv_sec < 0
        || now.tv_nsec < 0
    {
        return None;
    }
    u64::try_from(now.tv_sec)
        .ok()?
        .checked_mul(1_000_000_000)?
        .checked_add(u64::try_from(now.tv_nsec).ok()?)
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn instant_from_monotonic_deadline(deadline_ns: u64) -> Option<std::time::Instant> {
    let now = std::time::Instant::now();
    let now_ns = monotonic_now_ns()?;
    let remaining_ns = deadline_ns.checked_sub(now_ns)?;
    now.checked_add(Duration::from_nanos(remaining_ns))
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
impl RegisteredOwnedPasteSessionParentReportV1 {
    fn new(state: RegisteredOwnedPasteParentStateV1) -> Self {
        Self {
            state,
            child_started: false,
            child_exit_confirmed: false,
            child_exit_status: None,
            operation_count: 0,
            parent_write_attempt_count: 0,
            write_proof_saved_before_ack: false,
            parent_permit_count: 0,
            parent_may_have_posted: false,
            parent_edit_attempt_count: 0,
            parent_edit_result: registered_owned_paste_edit_unknown(),
            write_id: None,
            child_baseline: registered_owned_paste_baseline_unknown(),
            child_final_sample: registered_owned_paste_final_sample_unknown(),
            child_edit_response_received: false,
            child_observation_ticket_released: false,
            child_owner_joined: false,
            child_result: RegisteredOwnedPasteChildResultKindV1::OwnerUnknown,
            child_close_received: false,
            child_shortcut_sent: false,
            child_restore: RemoteProbeChildRestoreV1::Unknown,
            parent_restore_deferred_ack_sent: false,
        }
    }
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_owned_paste_edit_unknown() -> RegisteredOwnedPasteEditResultV1 {
    RegisteredOwnedPasteEditResultV1 {
        state: RegisteredOwnedPasteEditStateV1::Unknown,
        comparison: RegisteredOwnedPasteComparisonV1::Unknown,
        source: RegisteredOwnedPasteEditSourceV1::Unknown,
        has_value: false,
        sha256: [0; 32],
        byte_length: 0,
        has_edit_receipt: false,
        programmatic_edit_receipt_sha256: [0; 32],
    }
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_owned_paste_baseline_unknown() -> RegisteredOwnedPasteBaselineV1 {
    RegisteredOwnedPasteBaselineV1 {
        state: RegisteredOwnedPasteBaselineStateV1::Unknown,
        has_value: false,
        sha256: [0; 32],
        byte_length: 0,
        utf16_length: 0,
        selection_start: 0,
        selection_length: 0,
    }
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_owned_paste_final_sample_unknown() -> RegisteredOwnedPasteFinalSampleV1 {
    RegisteredOwnedPasteFinalSampleV1 {
        state: RegisteredOwnedPasteFinalSampleStateV1::Unknown,
        has_value: false,
        sha256: [0; 32],
        byte_length: 0,
        utf16_length: 0,
        focus_matches: false,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RemoteProbeChildRestoreV1 {
    Deferred,
    Unknown,
    NotNeeded,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RemoteProbeChildResultKindV1 {
    Completed,
    PermitDenied,
    PrewriteOwnershipChanged,
    ProtocolRejected,
    AcknowledgementUnavailable,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteProbeChildResultV1 {
    pub(crate) case: RegisteredNamedRemoteClipboardCaseV1,
    pub(crate) result: RemoteProbeChildResultKindV1,
    pub(crate) child_callback_count: u8,
    pub(crate) shortcut_sent: bool,
    pub(crate) target_dispatch_count: u8,
    pub(crate) restore: RemoteProbeChildRestoreV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RemoteParentProtocolOutcomeV1 {
    Complete,
    AcknowledgementUnavailable,
    PrewriteOwnershipChanged,
    Rejected,
    EofChildAlive,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum GeneralRemoteProbeStateV1 {
    ArgumentsRejected,
    CaptureFailed,
    ChildStartFailed,
    ReadySendFailed,
    Completed,
    ProtocolFailed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum GeneralRemoteCaptureStateV1 {
    NotAttempted,
    Captured,
    OverBudget,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum GeneralRemoteProbeCaseV1 {
    NormalRestore,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum GeneralRemotePermitKindV1 {
    RecorderOnlyNoTargetAuthority,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum GeneralRemoteNotProbedV1 {
    #[serde(rename = "not_probed")]
    Unprobed,
    NotRun,
    NotStarted,
    NotOpened,
}

/// Fixed, content-free receipt for the one-case General remote component.
/// Optional values distinguish facts not reached from a negative result.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct GeneralRemoteClipboardProbeReceiptV1 {
    protocol: &'static str,
    state: GeneralRemoteProbeStateV1,
    case: GeneralRemoteProbeCaseV1,
    board: RemoteClipboardBoardV1,
    capture_state: GeneralRemoteCaptureStateV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    capture_result: Option<ClipboardGuardReplyV1>,
    child_started: bool,
    child_exit_confirmed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    child_exit_status: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_protocol_outcome: Option<RemoteParentProtocolOutcomeV1>,
    operation_count: u8,
    parent_write_attempt_count: u8,
    write_proof_saved_before_ack: bool,
    parent_permit_count: u8,
    parent_may_have_posted: bool,
    child_callback_count: u8,
    shortcut_sent: bool,
    target_dispatch_count: u8,
    parent_restore_deferred_ack_sent: bool,
    child_reported_restore: RemoteProbeChildRestoreV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_owner_result: Option<ClipboardGuardReplyV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_cleanup_after_true_child_wait: Option<bool>,
    owner_retained: bool,
    original_contents_readback: GeneralRemoteNotProbedV1,
    permit_kind: GeneralRemotePermitKindV1,
    cmd_v: GeneralRemoteNotProbedV1,
    host: GeneralRemoteNotProbedV1,
    accessibility: GeneralRemoteNotProbedV1,
    sql_learning: GeneralRemoteNotProbedV1,
    settings: GeneralRemoteNotProbedV1,
    general_guard_active_for: bool,
}

impl GeneralRemoteClipboardProbeReceiptV1 {
    fn new(state: GeneralRemoteProbeStateV1) -> Self {
        Self {
            protocol: "voiceflow_general_remote_clipboard_probe_v1",
            state,
            case: GeneralRemoteProbeCaseV1::NormalRestore,
            board: RemoteClipboardBoardV1::GeneralClipboard,
            capture_state: GeneralRemoteCaptureStateV1::NotAttempted,
            capture_result: None,
            child_started: false,
            child_exit_confirmed: false,
            child_exit_status: None,
            parent_protocol_outcome: None,
            operation_count: 0,
            parent_write_attempt_count: 0,
            write_proof_saved_before_ack: false,
            parent_permit_count: 0,
            parent_may_have_posted: false,
            child_callback_count: 0,
            shortcut_sent: false,
            target_dispatch_count: 0,
            parent_restore_deferred_ack_sent: false,
            child_reported_restore: RemoteProbeChildRestoreV1::Unknown,
            parent_owner_result: None,
            parent_cleanup_after_true_child_wait: None,
            owner_retained: false,
            original_contents_readback: GeneralRemoteNotProbedV1::Unprobed,
            permit_kind: GeneralRemotePermitKindV1::RecorderOnlyNoTargetAuthority,
            cmd_v: GeneralRemoteNotProbedV1::NotRun,
            host: GeneralRemoteNotProbedV1::NotStarted,
            accessibility: GeneralRemoteNotProbedV1::Unprobed,
            sql_learning: GeneralRemoteNotProbedV1::NotRun,
            settings: GeneralRemoteNotProbedV1::NotOpened,
            general_guard_active_for: false,
        }
    }
}

struct RemoteParentProtocolReportV1 {
    outcome: RemoteParentProtocolOutcomeV1,
    // Count the owner write call before invoking it, including partial/unknown
    // failures; a saved proof is a separate fact.
    parent_write_attempt_count: u8,
    write_proof_saved_before_ack: bool,
    parent_permit_count: u8,
    parent_may_have_posted: bool,
    child_callback_count: u8,
    shortcut_sent: bool,
    target_dispatch_count: u8,
    operation_count: u8,
    // These are deliberately separate: sending RestoreDeferred is not proof
    // that the child received it. Only a later Close request reports the
    // child's own restore result.
    parent_restore_deferred_ack_sent: bool,
    child_reported_restore: RemoteProbeChildRestoreV1,
}

impl RemoteParentProtocolReportV1 {
    fn new(outcome: RemoteParentProtocolOutcomeV1) -> Self {
        Self {
            outcome,
            parent_write_attempt_count: 0,
            write_proof_saved_before_ack: false,
            parent_permit_count: 0,
            parent_may_have_posted: false,
            child_callback_count: 0,
            shortcut_sent: false,
            target_dispatch_count: 0,
            operation_count: 0,
            parent_restore_deferred_ack_sent: false,
            child_reported_restore: RemoteProbeChildRestoreV1::Unknown,
        }
    }
}

struct ClipboardGuardRpcClientStateV1 {
    stream: UnixStream,
    next_sequence: u8,
    closed: bool,
    registered_write_id: Option<u64>,
    injected_case_fault: bool,
    child_restore_state: RemoteProbeChildRestoreV1,
}

#[derive(Clone, Copy)]
enum SocketClipboardGuardRpcClientModeV1 {
    Component(RegisteredNamedRemoteClipboardCaseV1),
    RegisteredOwnedPasteSession,
}

struct SocketClipboardGuardRpcClientV1 {
    lease_id: u64,
    mode: SocketClipboardGuardRpcClientModeV1,
    max_sequence: u8,
    state: Mutex<ClipboardGuardRpcClientStateV1>,
}

impl SocketClipboardGuardRpcClientV1 {
    fn new(stream: UnixStream, lease_id: u64, case: RegisteredNamedRemoteClipboardCaseV1) -> Self {
        Self {
            lease_id,
            mode: SocketClipboardGuardRpcClientModeV1::Component(case),
            max_sequence: 10,
            state: Mutex::new(ClipboardGuardRpcClientStateV1 {
                stream,
                next_sequence: 1,
                closed: false,
                registered_write_id: None,
                injected_case_fault: false,
                child_restore_state: RemoteProbeChildRestoreV1::Unknown,
            }),
        }
    }

    fn new_registered_owned_session(stream: UnixStream, lease_id: u64) -> Self {
        Self {
            lease_id,
            mode: SocketClipboardGuardRpcClientModeV1::RegisteredOwnedPasteSession,
            max_sequence: 11,
            state: Mutex::new(ClipboardGuardRpcClientStateV1 {
                stream,
                next_sequence: 1,
                closed: false,
                registered_write_id: None,
                injected_case_fault: false,
                child_restore_state: RemoteProbeChildRestoreV1::Unknown,
            }),
        }
    }

    fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            let _ = state.stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

impl ClipboardGuardRpcClientV1 for SocketClipboardGuardRpcClientV1 {
    fn request(
        &self,
        lease_id: u64,
        operation: ClipboardGuardOperationV1,
    ) -> Result<ClipboardGuardRpcOutcomeV1, ClipboardGuardClientErrorV1> {
        if lease_id != self.lease_id {
            return Err(ClipboardGuardClientErrorV1::LeaseMismatch);
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| ClipboardGuardClientErrorV1::Closed)?;
        if state.closed {
            return Err(ClipboardGuardClientErrorV1::Closed);
        }
        if state.next_sequence == 0 || state.next_sequence > self.max_sequence {
            state.closed = true;
            let _ = state.stream.shutdown(std::net::Shutdown::Both);
            return Err(ClipboardGuardClientErrorV1::SequenceMismatch);
        }

        let sequence = state.next_sequence;
        state.next_sequence = sequence
            .checked_add(1)
            .ok_or(ClipboardGuardClientErrorV1::SequenceMismatch)?;
        let wire_sequence = if matches!(
            self.mode,
            SocketClipboardGuardRpcClientModeV1::Component(
                RegisteredNamedRemoteClipboardCaseV1::WrongSequence
            )
        ) && sequence == 1
        {
            sequence
                .checked_add(1)
                .ok_or(ClipboardGuardClientErrorV1::SequenceMismatch)?
        } else {
            sequence
        };
        let mut wire_operation = operation;
        if !state.injected_case_fault
            && matches!(self.mode, SocketClipboardGuardRpcClientModeV1::Component(_))
            && matches!(
                operation,
                ClipboardGuardOperationV1::CheckCurrent {
                    stage: ClipboardGuardCheckStageV1::MarkerAfterWrite,
                    ..
                }
            )
        {
            if let Some(write_id) = state.registered_write_id {
                match self.mode {
                    SocketClipboardGuardRpcClientModeV1::Component(
                        RegisteredNamedRemoteClipboardCaseV1::WrongWriteId,
                    ) => {
                        wire_operation = ClipboardGuardOperationV1::CheckCurrent {
                            write_id: write_id ^ 1,
                            stage: ClipboardGuardCheckStageV1::MarkerAfterWrite,
                            child_callback_count: 0,
                        };
                        state.injected_case_fault = true;
                    }
                    SocketClipboardGuardRpcClientModeV1::Component(
                        RegisteredNamedRemoteClipboardCaseV1::ReplayWrite,
                    ) => {
                        wire_operation = ClipboardGuardOperationV1::WriteRegisteredTemporaryText;
                        state.injected_case_fault = true;
                    }
                    _ => {}
                }
            }
        }
        let request = ClipboardGuardRpcFrameV1 {
            protocol_version: PROTOCOL_VERSION,
            lease_id,
            sequence: wire_sequence,
            body: ClipboardGuardRpcBodyV1::Request {
                operation: wire_operation,
            },
        };
        if let Err(error) = send_frame(&mut state.stream, &request) {
            state.closed = true;
            let _ = state.stream.shutdown(std::net::Shutdown::Both);
            return Err(if error.kind() == std::io::ErrorKind::InvalidData {
                ClipboardGuardClientErrorV1::FrameOverBudget
            } else {
                ClipboardGuardClientErrorV1::Io
            });
        }

        let reply = match read_guard_rpc_frame(&mut state.stream) {
            Ok(Some(reply)) => reply,
            Ok(None) => {
                state.closed = true;
                let _ = state.stream.shutdown(std::net::Shutdown::Both);
                return Err(ClipboardGuardClientErrorV1::AcknowledgementUnavailable);
            }
            Err(error) => {
                state.closed = true;
                let _ = state.stream.shutdown(std::net::Shutdown::Both);
                return Err(error);
            }
        };
        if reply.protocol_version != PROTOCOL_VERSION {
            state.closed = true;
            return Err(ClipboardGuardClientErrorV1::VersionMismatch);
        }
        if reply.lease_id != self.lease_id {
            state.closed = true;
            return Err(ClipboardGuardClientErrorV1::LeaseMismatch);
        }
        if reply.sequence != wire_sequence {
            state.closed = true;
            return Err(ClipboardGuardClientErrorV1::SequenceMismatch);
        }
        match reply.body {
            ClipboardGuardRpcBodyV1::Reply {
                outcome: ClipboardGuardRpcOutcomeV1::Rejected,
            } => {
                state.closed = true;
                let _ = state.stream.shutdown(std::net::Shutdown::Both);
                Err(ClipboardGuardClientErrorV1::UnexpectedReply)
            }
            ClipboardGuardRpcBodyV1::Reply { outcome } => {
                if let ClipboardGuardRpcOutcomeV1::WriteOwned { write_id } = outcome {
                    state.registered_write_id = Some(write_id);
                }
                if matches!(
                    operation,
                    ClipboardGuardOperationV1::RequestRestoreIfOwned { .. }
                ) && outcome == ClipboardGuardRpcOutcomeV1::RestoreDeferred
                {
                    state.child_restore_state = RemoteProbeChildRestoreV1::Deferred;
                }
                Ok(outcome)
            }
            ClipboardGuardRpcBodyV1::Request { .. } => {
                state.closed = true;
                Err(ClipboardGuardClientErrorV1::UnexpectedReply)
            }
        }
    }

    fn child_restore_state(&self) -> RemoteProbeChildRestoreV1 {
        self.state
            .lock()
            .map(|state| state.child_restore_state)
            .unwrap_or(RemoteProbeChildRestoreV1::Unknown)
    }

    fn registered_write_id(&self) -> Option<u64> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.registered_write_id)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ChildRequestV1 {
    protocol_version: u8,
    lease_id: u64,
    sequence: u8,
    operation: ChildOperationV1,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ChildOperationV1 {
    WriteFixture { fixture: ClipboardWriteFixtureV1 },
    WriteRegisteredTemporaryText,
    CheckCurrent,
    RestoreIfOwned,
    Close,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ParentFrameV1 {
    Ready {
        protocol_version: u8,
        lease_id: u64,
    },
    Reply {
        protocol_version: u8,
        lease_id: u64,
        sequence: u8,
        result: ClipboardGuardReplyV1,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChildCaseV1 {
    ExitBeforeWrite,
    ExitBeforeAck,
    ExitAfterAck,
    EofWhileAlive,
    NormalRestore,
    ExternalCopyAfterExit,
}

impl ChildCaseV1 {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "exit_before_write" => Some(Self::ExitBeforeWrite),
            "exit_before_ack" => Some(Self::ExitBeforeAck),
            "exit_after_ack" => Some(Self::ExitAfterAck),
            "eof_while_alive" => Some(Self::EofWhileAlive),
            "normal_restore" => Some(Self::NormalRestore),
            "external_copy_after_exit" => Some(Self::ExternalCopyAfterExit),
            _ => None,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::ExitBeforeWrite => "exit_before_write",
            Self::ExitBeforeAck => "exit_before_ack",
            Self::ExitAfterAck => "exit_after_ack",
            Self::EofWhileAlive => "eof_while_alive",
            Self::NormalRestore => "normal_restore",
            Self::ExternalCopyAfterExit => "external_copy_after_exit",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GeneralChildCaseV1 {
    ExitBeforeWrite,
    ExitBeforeAck,
    ExitAfterAck,
    EofWhileAlive,
    NormalRestore,
}

impl GeneralChildCaseV1 {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "exit_before_write" => Some(Self::ExitBeforeWrite),
            "exit_before_ack" => Some(Self::ExitBeforeAck),
            "exit_after_ack" => Some(Self::ExitAfterAck),
            "eof_while_alive" => Some(Self::EofWhileAlive),
            "normal_restore" => Some(Self::NormalRestore),
            _ => None,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::ExitBeforeWrite => "exit_before_write",
            Self::ExitBeforeAck => "exit_before_ack",
            Self::ExitAfterAck => "exit_after_ack",
            Self::EofWhileAlive => "eof_while_alive",
            Self::NormalRestore => "normal_restore",
        }
    }
}

pub(crate) fn run_component_cli(args: impl IntoIterator<Item = OsString>) -> i32 {
    let args = args.into_iter().collect::<Vec<_>>();
    let (seed, child_case) = match parse_component_arguments(&args) {
        Ok(parsed) => parsed,
        Err(reason) => {
            write_receipt(&serde_json::json!({
                "protocol": "voiceflow_clipboard_guard_component_v1",
                "ok": false,
                "reason": reason,
                "clipboard": "named_fixture_only",
                "general_clipboard": "not_touched",
                "cmd_v": "not_run",
            }));
            return 2;
        }
    };

    let mut owner = match crate::paste::ResearchClipboardGuardOwnerV1::capture_named_fixture(seed) {
        Ok(owner) => owner,
        Err(reason) => {
            write_receipt(&serde_json::json!({
                "protocol": "voiceflow_clipboard_guard_component_v1",
                "ok": false,
                "seed": seed,
                "capture": reason,
                "child_started": false,
                "clipboard": "named_fixture_only",
                "general_clipboard": "not_touched",
                "cmd_v": "not_run",
            }));
            return 1;
        }
    };

    let lease_id = opaque_local_lease_id();
    let (mut child, mut parent_socket) = match spawn_fixed_child(child_case) {
        Ok(spawned) => spawned,
        Err(reason) => {
            let ownership = owner.current_ownership();
            let restore = if ownership == ClipboardGuardReplyV1::WriteOwned {
                owner.restore_if_owned()
            } else {
                ownership
            };
            let restore = close_after_resolved(&mut owner, restore);
            let unknown = owner_requires_retention(restore);
            write_receipt(&serde_json::json!({
                "protocol": "voiceflow_clipboard_guard_component_v1",
                "ok": false,
                "seed": seed,
                "child_started": false,
                "reason": reason,
                "owner_result": restore,
                "owner_retained": unknown,
                "general_clipboard": "not_touched",
                "cmd_v": "not_run",
            }));
            if unknown {
                hold_unknown_owner(&mut owner);
            }
            return 1;
        }
    };

    if send_frame(
        &mut parent_socket,
        &ParentFrameV1::Ready {
            protocol_version: PROTOCOL_VERSION,
            lease_id,
        },
    )
    .is_err()
    {
        // Let the fixed child observe a failed/partial Ready frame as EOF
        // before waiting for its process to exit.
        drop(parent_socket);
        let status = wait_for_child_status(&mut child);
        let ownership = owner.current_ownership();
        let result = close_after_resolved(&mut owner, ownership);
        write_receipt(&serde_json::json!({
            "protocol": "voiceflow_clipboard_guard_component_v1",
            "ok": false,
            "seed": seed,
            "child_exit_confirmed": true,
            "child_exit_status": status.code(),
            "owner_result": result,
            "owner_retained": owner_requires_retention(result),
            "reason": "guard_ready_send_failed",
            "general_clipboard": "not_touched",
            "cmd_v": "not_run",
        }));
        if owner_requires_retention(result) {
            hold_unknown_owner(&mut owner);
        }
        return 1;
    }

    let mut child_marker_check = None;
    let lease = run_child_protocol(
        &mut child,
        &mut parent_socket,
        lease_id,
        &mut owner,
        &mut child_marker_check,
    );
    // On a malformed or failed protocol read, the child may still be waiting
    // for a reply. Closing this endpoint lets it terminate before we wait.
    drop(parent_socket);
    let status = wait_for_child_status(&mut child);
    let mut post_exit_probe = owner.current_ownership();

    if child_case == ChildCaseV1::ExternalCopyAfterExit
        && post_exit_probe == ClipboardGuardReplyV1::WriteOwned
    {
        let _ = owner.write_fixture(ClipboardWriteFixtureV1::ExternalCopyV1);
        post_exit_probe = owner.current_ownership();
    }

    let owner_result = match post_exit_probe {
        ClipboardGuardReplyV1::WriteOwned => owner.restore_if_owned(),
        other => other,
    };
    let owner_result = close_after_resolved(&mut owner, owner_result);
    let child_exit_confirmed = !status.success() || status.code().is_some();
    let owner_retained = owner_requires_retention(owner_result);
    let protocol_unknown = matches!(lease, ProtocolOutcomeV1::Unknown);
    write_receipt(&serde_json::json!({
        "protocol": "voiceflow_clipboard_guard_component_v1",
        "ok": status.success()
            && !owner_retained
            && !protocol_unknown
            && matches!(
                owner_result,
                ClipboardGuardReplyV1::Restored
                    | ClipboardGuardReplyV1::OwnershipChanged
                    | ClipboardGuardReplyV1::NotWritten
            ),
        "seed": seed,
        "child_case": child_case.code(),
        "child_exit_confirmed": child_exit_confirmed,
        "child_exit_status": status.code(),
        "socket_eof_before_child_exit": matches!(
            lease,
            ProtocolOutcomeV1::EofWriteOwned
                | ProtocolOutcomeV1::EofNotWritten
                | ProtocolOutcomeV1::EofOwnershipChanged
        ),
        "child_marker_check": child_marker_check,
        "owner_state_at_eof": lease.eof_owner_state(),
        "owner_result": owner_result,
        "owner_retained": owner_retained,
        "general_clipboard": "not_touched",
        "cmd_v": "not_run",
        "accessibility": "not_used",
    }));
    if owner_retained {
        hold_unknown_owner(&mut owner);
    }
    if status.success() && !owner_retained && !protocol_unknown {
        0
    } else {
        1
    }
}

/// Explicit real-General-board supervisor component. This does not send
/// Cmd+V or start an owned target; the parent alone owns the original snapshot
/// and performs the actual restoration after the child has exited.
pub(crate) fn run_general_component_cli(args: impl IntoIterator<Item = OsString>) -> i32 {
    let args = args.into_iter().collect::<Vec<_>>();
    let child_case = match parse_general_component_arguments(&args) {
        Ok(child_case) => child_case,
        Err(reason) => {
            write_receipt(&serde_json::json!({
                "protocol": "voiceflow_general_clipboard_guard_component_v1",
                "ok": false,
                "reason": reason,
                "general_clipboard": "not_captured",
                "cmd_v": "not_run",
                "accessibility": "not_used",
                "host": "not_started",
            }));
            return 2;
        }
    };

    let mut owner = match crate::paste::ResearchClipboardGuardOwnerV1::capture_general() {
        Ok(owner) => owner,
        Err(capture) => {
            write_receipt(&serde_json::json!({
                "protocol": "voiceflow_general_clipboard_guard_component_v1",
                "ok": false,
                "capture": capture,
                "child_started": false,
                "write_attempted": false,
                "general_clipboard": "capture_failed_or_over_budget",
                "cmd_v": "not_run",
                "accessibility": "not_used",
                "host": "not_started",
            }));
            return 1;
        }
    };

    let lease_id = opaque_local_lease_id();
    let (mut child, mut parent_socket) = match spawn_fixed_general_child(child_case) {
        Ok(spawned) => spawned,
        Err(reason) => {
            let current = owner.current_ownership();
            let result = close_after_general_resolution(&mut owner, current);
            let retained = owner_requires_retention(result);
            write_receipt(&serde_json::json!({
                "protocol": "voiceflow_general_clipboard_guard_component_v1",
                "ok": false,
                "child_started": false,
                "write_attempted": false,
                "owner_result": result,
                "owner_retained": retained,
                "reason": reason,
                "general_clipboard": "captured_no_write",
                "cmd_v": "not_run",
                "accessibility": "not_used",
                "host": "not_started",
            }));
            if retained {
                hold_unknown_general_owner(&mut owner);
            }
            return 1;
        }
    };

    if send_frame(
        &mut parent_socket,
        &ParentFrameV1::Ready {
            protocol_version: PROTOCOL_VERSION,
            lease_id,
        },
    )
    .is_err()
    {
        // Closing the parent endpoint lets a child blocked on a partial or
        // failed Ready frame observe EOF and terminate before we wait for it.
        drop(parent_socket);
        let status = wait_for_child_status(&mut child);
        let owner_state = owner.current_ownership();
        let result = close_after_general_resolution(&mut owner, owner_state);
        let retained = owner_requires_retention(result);
        write_receipt(&serde_json::json!({
            "protocol": "voiceflow_general_clipboard_guard_component_v1",
            "ok": false,
            "child_started": true,
            "child_exit_confirmed": true,
            "child_exit_status": status.code(),
            "write_attempted": false,
            "owner_result": result,
            "owner_retained": retained,
            "reason": "guard_ready_send_failed",
            "general_clipboard": "captured_no_write",
            "cmd_v": "not_run",
            "accessibility": "not_used",
            "host": "not_started",
        }));
        if retained {
            hold_unknown_general_owner(&mut owner);
        }
        return 1;
    }

    let mut child_marker_check = None;
    let lease = run_general_child_protocol(
        &mut child,
        &mut parent_socket,
        lease_id,
        &mut owner,
        &mut child_marker_check,
    );
    // A malformed/failed protocol read may leave the fixed child waiting for
    // its next reply. The child must see EOF before the parent waits for exit.
    drop(parent_socket);
    // EOF only closes the socket. Keep the snapshot owner until the kernel
    // confirms the same fixed child process has exited.
    let status = wait_for_child_status(&mut child);
    let post_exit_state = owner.current_ownership();
    let owner_result = match post_exit_state {
        ClipboardGuardReplyV1::WriteOwned => owner.restore_if_owned(),
        other => other,
    };
    let owner_result = close_after_general_resolution(&mut owner, owner_result);
    let child_exit_confirmed = !status.success() || status.code().is_some();
    let retained = owner_requires_retention(owner_result);
    let protocol_unknown = matches!(lease, ProtocolOutcomeV1::Unknown);
    write_receipt(&serde_json::json!({
        "protocol": "voiceflow_general_clipboard_guard_component_v1",
        "ok": status.success()
            && !retained
            && !protocol_unknown
            && matches!(
                owner_result,
                ClipboardGuardReplyV1::Restored
                    | ClipboardGuardReplyV1::OwnershipChanged
                    | ClipboardGuardReplyV1::NotWritten
            ),
        "child_case": child_case.code(),
        "child_exit_confirmed": child_exit_confirmed,
        "child_exit_status": status.code(),
        "socket_eof_before_child_exit": matches!(
            lease,
            ProtocolOutcomeV1::EofWriteOwned
                | ProtocolOutcomeV1::EofNotWritten
                | ProtocolOutcomeV1::EofOwnershipChanged
        ),
        "child_marker_check": child_marker_check,
        "owner_state_at_eof": lease.eof_owner_state(),
        "owner_result": owner_result,
        "owner_retained": retained,
        "general_clipboard": "explicit_research_component_used",
        "cmd_v": "not_run",
        "accessibility": "not_used",
        "host": "not_started",
    }));
    if retained {
        hold_unknown_general_owner(&mut owner);
    }
    if status.success() && !retained && !protocol_unknown {
        0
    } else {
        1
    }
}

pub(crate) fn run_general_child_cli(args: impl IntoIterator<Item = OsString>) -> i32 {
    let args = args.into_iter().collect::<Vec<_>>();
    if args.len() != 1 {
        return 2;
    }
    let Some(child_case) = args[0].to_str().and_then(GeneralChildCaseV1::parse) else {
        return 2;
    };
    if set_child_fd_cloexec(CHILD_CONTROL_FD).is_err() {
        return 2;
    }
    let mut stream = unsafe { UnixStream::from_raw_fd(CHILD_CONTROL_FD) };
    let lease_id = match read_frame::<ParentFrameV1>(&mut stream) {
        Ok(Some(ParentFrameV1::Ready {
            protocol_version,
            lease_id,
        })) if protocol_version == PROTOCOL_VERSION => lease_id,
        _ => return 2,
    };
    if child_case == GeneralChildCaseV1::ExitBeforeWrite {
        return 0;
    }
    if send_request(
        &mut stream,
        lease_id,
        1,
        ChildOperationV1::WriteRegisteredTemporaryText,
    )
    .is_err()
    {
        return 2;
    }
    if child_case == GeneralChildCaseV1::ExitBeforeAck {
        let _ = stream.shutdown(std::net::Shutdown::Both);
        return 0;
    }
    if read_reply(&mut stream, lease_id, 1) != Some(ClipboardGuardReplyV1::WriteOwned) {
        return 1;
    }
    if check_current(&mut stream, lease_id, 2) != Some(ClipboardGuardReplyV1::WriteOwned) {
        return 1;
    }
    match child_case {
        GeneralChildCaseV1::ExitBeforeWrite | GeneralChildCaseV1::ExitBeforeAck => 0,
        GeneralChildCaseV1::ExitAfterAck => 0,
        GeneralChildCaseV1::EofWhileAlive => {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            std::thread::sleep(EOF_CHILD_ALIVE_DELAY);
            0
        }
        GeneralChildCaseV1::NormalRestore => {
            if send_request(&mut stream, lease_id, 3, ChildOperationV1::RestoreIfOwned).is_err()
                || read_reply(&mut stream, lease_id, 3)
                    != Some(ClipboardGuardReplyV1::RestoreRequested)
                || send_request(&mut stream, lease_id, 4, ChildOperationV1::Close).is_err()
                || read_reply(&mut stream, lease_id, 4)
                    != Some(ClipboardGuardReplyV1::CloseAcknowledged)
            {
                return 2;
            }
            let _ = stream.shutdown(std::net::Shutdown::Both);
            0
        }
    }
}

pub(crate) fn run_child_cli(args: impl IntoIterator<Item = OsString>) -> i32 {
    let args = args.into_iter().collect::<Vec<_>>();
    if args.len() != 1 {
        return 2;
    }
    let Some(child_case) = args[0].to_str().and_then(ChildCaseV1::parse) else {
        return 2;
    };

    if set_child_fd_cloexec(CHILD_CONTROL_FD).is_err() {
        return 2;
    }
    let mut stream = unsafe { UnixStream::from_raw_fd(CHILD_CONTROL_FD) };
    let ready = match read_frame::<ParentFrameV1>(&mut stream) {
        Ok(Some(ParentFrameV1::Ready {
            protocol_version,
            lease_id,
        })) if protocol_version == PROTOCOL_VERSION => lease_id,
        _ => return 2,
    };

    if child_case == ChildCaseV1::ExitBeforeWrite {
        return 0;
    }
    if send_request(
        &mut stream,
        ready,
        1,
        ChildOperationV1::WriteFixture {
            fixture: ClipboardWriteFixtureV1::RegisteredTemporaryTextV1,
        },
    )
    .is_err()
    {
        return 2;
    }

    if child_case == ChildCaseV1::ExitBeforeAck {
        let _ = stream.shutdown(std::net::Shutdown::Both);
        return 0;
    }
    let write_ack = read_reply(&mut stream, ready, 1);
    if child_case == ChildCaseV1::ExitAfterAck {
        if write_ack != Some(ClipboardGuardReplyV1::WriteOwned) {
            return 1;
        }
        return if check_current(&mut stream, ready, 2) == Some(ClipboardGuardReplyV1::WriteOwned) {
            0
        } else {
            1
        };
    }
    if write_ack != Some(ClipboardGuardReplyV1::WriteOwned) {
        return 1;
    }
    if child_case == ChildCaseV1::EofWhileAlive {
        if check_current(&mut stream, ready, 2) != Some(ClipboardGuardReplyV1::WriteOwned) {
            return 1;
        }
        let _ = stream.shutdown(std::net::Shutdown::Both);
        std::thread::sleep(EOF_CHILD_ALIVE_DELAY);
        return 0;
    }

    if check_current(&mut stream, ready, 2) != Some(ClipboardGuardReplyV1::WriteOwned) {
        return 1;
    }
    if child_case == ChildCaseV1::NormalRestore
        && (send_request(&mut stream, ready, 3, ChildOperationV1::RestoreIfOwned).is_err()
            || read_reply(&mut stream, ready, 3) != Some(ClipboardGuardReplyV1::RestoreRequested)
            || send_request(&mut stream, ready, 4, ChildOperationV1::Close).is_err()
            || read_reply(&mut stream, ready, 4) != Some(ClipboardGuardReplyV1::CloseAcknowledged))
    {
        return 2;
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
    0
}

fn parse_component_arguments(
    args: &[OsString],
) -> Result<(ClipboardSeedFixtureV1, ChildCaseV1), &'static str> {
    if args.len() != 4
        || args[0].to_str() != Some("--seed")
        || args[2].to_str() != Some("--child-case")
    {
        return Err("usage: voiceflow-learning-harness --clipboard-guard-component --seed FIXED_SEED --child-case FIXED_CASE");
    }
    let seed = match args[1].to_str() {
        Some("empty") => ClipboardSeedFixtureV1::Empty,
        Some("one_item_multi_flavor") => ClipboardSeedFixtureV1::OneItemMultiFlavor,
        Some("multiple_items") => ClipboardSeedFixtureV1::MultipleItems,
        Some("over_128_mib") => ClipboardSeedFixtureV1::Over128MiB,
        _ => return Err("clipboard_guard_seed_unsupported"),
    };
    let child_case = args[3]
        .to_str()
        .and_then(ChildCaseV1::parse)
        .ok_or("clipboard_guard_child_case_unsupported")?;
    Ok((seed, child_case))
}

fn parse_general_component_arguments(
    args: &[OsString],
) -> Result<GeneralChildCaseV1, &'static str> {
    if args.len() != 2 || args[0].to_str() != Some("--child-case") {
        return Err("usage: voiceflow-learning-harness --clipboard-guard-general-component --child-case FIXED_CASE");
    }
    args[1]
        .to_str()
        .and_then(GeneralChildCaseV1::parse)
        .ok_or("general_clipboard_guard_child_case_unsupported")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProtocolOutcomeV1 {
    EofWriteOwned,
    EofNotWritten,
    EofOwnershipChanged,
    AcknowledgementUnavailable,
    Complete,
    Unknown,
}

impl ProtocolOutcomeV1 {
    fn eof_owner_state(self) -> &'static str {
        match self {
            Self::EofWriteOwned => "write_still_owned",
            Self::EofNotWritten => "not_written",
            Self::EofOwnershipChanged => "ownership_changed",
            Self::AcknowledgementUnavailable => "write_ack_unavailable",
            Self::Complete => "not_applicable",
            Self::Unknown => "unknown",
        }
    }
}

fn run_child_protocol(
    child: &mut Child,
    socket: &mut UnixStream,
    lease_id: u64,
    owner: &mut crate::paste::ResearchClipboardGuardOwnerV1,
    child_marker_check: &mut Option<ClipboardGuardReplyV1>,
) -> ProtocolOutcomeV1 {
    let mut buffer = Vec::new();
    let mut expected_sequence = 1_u8;
    let mut checked_current = false;
    let mut restore_requested = false;
    loop {
        match read_request_nonblocking(socket, &mut buffer, child) {
            Ok(Some(request)) => {
                if request.protocol_version != PROTOCOL_VERSION
                    || request.lease_id != lease_id
                    || request.sequence != expected_sequence
                {
                    let _ = send_reply(
                        socket,
                        lease_id,
                        expected_sequence,
                        ClipboardGuardReplyV1::Rejected,
                    );
                    return ProtocolOutcomeV1::Unknown;
                }
                let is_write_request =
                    matches!(&request.operation, ChildOperationV1::WriteFixture { .. });
                let result = match request.operation {
                    ChildOperationV1::WriteFixture {
                        fixture: ClipboardWriteFixtureV1::RegisteredTemporaryTextV1,
                    } if expected_sequence == 1 && !restore_requested => {
                        owner.write_fixture(ClipboardWriteFixtureV1::RegisteredTemporaryTextV1)
                    }
                    ChildOperationV1::CheckCurrent
                        if expected_sequence == 2 && !checked_current && !restore_requested =>
                    {
                        checked_current = true;
                        let result = owner.current_ownership();
                        *child_marker_check = Some(result);
                        result
                    }
                    ChildOperationV1::RestoreIfOwned
                        if expected_sequence == 3 && checked_current && !restore_requested =>
                    {
                        restore_requested = true;
                        ClipboardGuardReplyV1::RestoreRequested
                    }
                    ChildOperationV1::Close if restore_requested && expected_sequence == 4 => {
                        ClipboardGuardReplyV1::CloseAcknowledged
                    }
                    ChildOperationV1::WriteRegisteredTemporaryText => {
                        ClipboardGuardReplyV1::Rejected
                    }
                    _ => ClipboardGuardReplyV1::Rejected,
                };
                if send_reply(socket, lease_id, expected_sequence, result).is_err() {
                    return if is_write_request {
                        ProtocolOutcomeV1::AcknowledgementUnavailable
                    } else {
                        ProtocolOutcomeV1::Unknown
                    };
                }
                expected_sequence = match expected_sequence.checked_add(1) {
                    Some(next) => next,
                    None => return ProtocolOutcomeV1::Unknown,
                };
            }
            Ok(None) => loop {
                match child.try_wait() {
                    Ok(Some(_)) => return ProtocolOutcomeV1::Complete,
                    Ok(None) => {
                        let outcome = match owner.current_ownership() {
                            ClipboardGuardReplyV1::WriteOwned => ProtocolOutcomeV1::EofWriteOwned,
                            ClipboardGuardReplyV1::NotWritten => ProtocolOutcomeV1::EofNotWritten,
                            ClipboardGuardReplyV1::OwnershipChanged => {
                                ProtocolOutcomeV1::EofOwnershipChanged
                            }
                            _ => ProtocolOutcomeV1::Unknown,
                        };
                        return wait_for_child_after_eof(child, outcome);
                    }
                    Err(_) => match child.wait() {
                        Ok(_) => return ProtocolOutcomeV1::Complete,
                        Err(_) => std::thread::sleep(CHILD_POLL_INTERVAL),
                    },
                }
            },
            Err(_) => return ProtocolOutcomeV1::Unknown,
        }
    }
}

fn run_general_child_protocol(
    child: &mut Child,
    socket: &mut UnixStream,
    lease_id: u64,
    owner: &mut crate::paste::ResearchClipboardGuardOwnerV1,
    child_marker_check: &mut Option<ClipboardGuardReplyV1>,
) -> ProtocolOutcomeV1 {
    let mut buffer = Vec::new();
    let mut expected_sequence = 1_u8;
    let mut checked_current = false;
    let mut restore_requested = false;
    loop {
        match read_request_nonblocking(socket, &mut buffer, child) {
            Ok(Some(request)) => {
                if request.protocol_version != PROTOCOL_VERSION
                    || request.lease_id != lease_id
                    || request.sequence != expected_sequence
                {
                    let _ = send_reply(
                        socket,
                        lease_id,
                        expected_sequence,
                        ClipboardGuardReplyV1::Rejected,
                    );
                    return ProtocolOutcomeV1::Unknown;
                }
                let is_write_request = matches!(
                    &request.operation,
                    ChildOperationV1::WriteRegisteredTemporaryText
                );
                let result = match request.operation {
                    ChildOperationV1::WriteRegisteredTemporaryText
                        if expected_sequence == 1 && !restore_requested =>
                    {
                        owner.write_registered_temporary_text()
                    }
                    ChildOperationV1::CheckCurrent
                        if expected_sequence == 2 && !checked_current && !restore_requested =>
                    {
                        checked_current = true;
                        let result = owner.current_ownership();
                        *child_marker_check = Some(result);
                        result
                    }
                    ChildOperationV1::RestoreIfOwned
                        if expected_sequence == 3 && checked_current && !restore_requested =>
                    {
                        restore_requested = true;
                        ClipboardGuardReplyV1::RestoreRequested
                    }
                    ChildOperationV1::Close if restore_requested && expected_sequence == 4 => {
                        ClipboardGuardReplyV1::CloseAcknowledged
                    }
                    _ => ClipboardGuardReplyV1::Rejected,
                };
                if send_reply(socket, lease_id, expected_sequence, result).is_err() {
                    return if is_write_request {
                        ProtocolOutcomeV1::AcknowledgementUnavailable
                    } else {
                        ProtocolOutcomeV1::Unknown
                    };
                }
                expected_sequence = match expected_sequence.checked_add(1) {
                    Some(next) => next,
                    None => return ProtocolOutcomeV1::Unknown,
                };
            }
            Ok(None) => loop {
                match child.try_wait() {
                    Ok(Some(_)) => return ProtocolOutcomeV1::Complete,
                    Ok(None) => {
                        let outcome = match owner.current_ownership() {
                            ClipboardGuardReplyV1::WriteOwned => ProtocolOutcomeV1::EofWriteOwned,
                            ClipboardGuardReplyV1::NotWritten => ProtocolOutcomeV1::EofNotWritten,
                            ClipboardGuardReplyV1::OwnershipChanged => {
                                ProtocolOutcomeV1::EofOwnershipChanged
                            }
                            _ => ProtocolOutcomeV1::Unknown,
                        };
                        return wait_for_child_after_eof(child, outcome);
                    }
                    Err(_) => match child.wait() {
                        Ok(_) => return ProtocolOutcomeV1::Complete,
                        Err(_) => std::thread::sleep(CHILD_POLL_INTERVAL),
                    },
                }
            },
            Err(_) => return ProtocolOutcomeV1::Unknown,
        }
    }
}

fn wait_for_child_after_eof(child: &mut Child, outcome: ProtocolOutcomeV1) -> ProtocolOutcomeV1 {
    let _ = wait_for_child_status(child);
    outcome
}

fn read_request_nonblocking(
    socket: &mut UnixStream,
    buffer: &mut Vec<u8>,
    child: &mut Child,
) -> Result<Option<ChildRequestV1>, &'static str> {
    let mut chunk = [0_u8; 2_048];
    loop {
        if let Some(frame) = take_frame::<ChildRequestV1>(buffer)? {
            return Ok(Some(frame));
        }
        match socket.read(&mut chunk) {
            Ok(0) => return Ok(None),
            Ok(count) => {
                if buffer
                    .len()
                    .checked_add(count)
                    .is_none_or(|size| size > MAX_FRAME_BYTES + 4)
                {
                    return Err("clipboard_guard_frame_over_budget");
                }
                buffer.extend_from_slice(&chunk[..count]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if child
                    .try_wait()
                    .map_err(|_| "clipboard_guard_child_status_unknown")?
                    .is_some()
                {
                    match socket.read(&mut chunk) {
                        Ok(0) => return Ok(None),
                        Ok(count) => {
                            buffer.extend_from_slice(&chunk[..count]);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            return Err("clipboard_guard_child_exited_before_request");
                        }
                        Err(_) => return Err("clipboard_guard_socket_read_failed"),
                    }
                } else {
                    std::thread::sleep(CHILD_POLL_INTERVAL);
                }
            }
            Err(_) => return Err("clipboard_guard_socket_read_failed"),
        }
    }
}

fn take_frame<T: for<'de> Deserialize<'de>>(
    buffer: &mut Vec<u8>,
) -> Result<Option<T>, &'static str> {
    if buffer.len() < 4 {
        return Ok(None);
    }
    let length = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err("clipboard_guard_frame_over_budget");
    }
    let frame_end = 4_usize
        .checked_add(length)
        .ok_or("clipboard_guard_frame_over_budget")?;
    if buffer.len() < frame_end {
        return Ok(None);
    }
    let value = serde_json::from_slice(&buffer[4..frame_end])
        .map_err(|_| "clipboard_guard_frame_rejected")?;
    buffer.drain(..frame_end);
    if !buffer.is_empty() {
        return Err("clipboard_guard_multiple_requests_in_single_read");
    }
    Ok(Some(value))
}

fn send_request(
    stream: &mut UnixStream,
    lease_id: u64,
    sequence: u8,
    operation: ChildOperationV1,
) -> std::io::Result<()> {
    send_frame(
        stream,
        &ChildRequestV1 {
            protocol_version: PROTOCOL_VERSION,
            lease_id,
            sequence,
            operation,
        },
    )
}

fn check_current(
    stream: &mut UnixStream,
    lease_id: u64,
    sequence: u8,
) -> Option<ClipboardGuardReplyV1> {
    send_request(stream, lease_id, sequence, ChildOperationV1::CheckCurrent).ok()?;
    read_reply(stream, lease_id, sequence)
}

fn send_reply(
    stream: &mut UnixStream,
    lease_id: u64,
    sequence: u8,
    result: ClipboardGuardReplyV1,
) -> std::io::Result<()> {
    send_frame(
        stream,
        &ParentFrameV1::Reply {
            protocol_version: PROTOCOL_VERSION,
            lease_id,
            sequence,
            result,
        },
    )
}

fn read_reply(
    stream: &mut UnixStream,
    lease_id: u64,
    sequence: u8,
) -> Option<ClipboardGuardReplyV1> {
    match read_frame::<ParentFrameV1>(stream).ok().flatten()? {
        ParentFrameV1::Reply {
            protocol_version,
            lease_id: reply_lease,
            sequence: reply_sequence,
            result,
        } if protocol_version == PROTOCOL_VERSION
            && reply_lease == lease_id
            && reply_sequence == sequence =>
        {
            Some(result)
        }
        _ => None,
    }
}

fn send_frame<T: Serialize>(stream: &mut UnixStream, value: &T) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "clipboard guard frame over budget",
        ));
    }
    let length = u32::try_from(bytes.len())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "frame length"))?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(&bytes)?;
    stream.flush()
}

fn read_frame<T: for<'de> Deserialize<'de>>(stream: &mut impl Read) -> std::io::Result<Option<T>> {
    let mut header = [0_u8; 4];
    let mut first = [0_u8; 1];
    match stream.read(&mut first)? {
        0 => return Ok(None),
        1 => header[0] = first[0],
        _ => unreachable!("one-byte buffer read more than one byte"),
    }
    stream.read_exact(&mut header[1..])?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "clipboard guard frame over budget",
        ));
    }
    let mut bytes = vec![0_u8; length];
    stream.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn read_guard_rpc_frame(
    stream: &mut UnixStream,
) -> Result<Option<ClipboardGuardRpcFrameV1>, ClipboardGuardClientErrorV1> {
    let mut header = [0_u8; 4];
    let mut first = [0_u8; 1];
    match stream.read(&mut first) {
        Ok(0) => return Ok(None),
        Ok(_) => header[0] = first[0],
        Err(_) => return Err(ClipboardGuardClientErrorV1::Io),
    }
    stream
        .read_exact(&mut header[1..])
        .map_err(|_| ClipboardGuardClientErrorV1::MalformedFrame)?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(ClipboardGuardClientErrorV1::FrameOverBudget);
    }
    let mut bytes = vec![0_u8; length];
    stream
        .read_exact(&mut bytes)
        .map_err(|_| ClipboardGuardClientErrorV1::MalformedFrame)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| ClipboardGuardClientErrorV1::MalformedFrame)
}

fn spawn_fixed_child(child_case: ChildCaseV1) -> Result<(Child, UnixStream), &'static str> {
    let (parent, child_socket) =
        UnixStream::pair().map_err(|_| "clipboard_guard_socketpair_failed")?;
    parent
        .set_nonblocking(true)
        .map_err(|_| "clipboard_guard_socket_nonblocking_failed")?;
    let inherited_fd = child_socket.as_raw_fd();
    let executable = std::env::current_exe().map_err(|_| "clipboard_guard_executable_unknown")?;
    let mut command = Command::new(executable);
    command
        .arg("--clipboard-guard-child")
        .arg(child_case.code())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(inherited_fd, CHILD_CONTROL_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let flags = libc::fcntl(CHILD_CONTROL_FD, libc::F_GETFD);
            if flags < 0
                || libc::fcntl(CHILD_CONTROL_FD, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command
        .spawn()
        .map_err(|_| "clipboard_guard_child_spawn_failed")?;
    drop(child_socket);
    Ok((child, parent))
}

fn spawn_fixed_general_child(
    child_case: GeneralChildCaseV1,
) -> Result<(Child, UnixStream), &'static str> {
    let (parent, child_socket) =
        UnixStream::pair().map_err(|_| "clipboard_guard_socketpair_failed")?;
    parent
        .set_nonblocking(true)
        .map_err(|_| "clipboard_guard_socket_nonblocking_failed")?;
    let inherited_fd = child_socket.as_raw_fd();
    let executable = std::env::current_exe().map_err(|_| "clipboard_guard_executable_unknown")?;
    let mut command = Command::new(executable);
    command
        .arg("--clipboard-guard-general-child")
        .arg(child_case.code())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(inherited_fd, CHILD_CONTROL_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let flags = libc::fcntl(CHILD_CONTROL_FD, libc::F_GETFD);
            if flags < 0
                || libc::fcntl(CHILD_CONTROL_FD, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command
        .spawn()
        .map_err(|_| "clipboard_guard_child_spawn_failed")?;
    drop(child_socket);
    Ok((child, parent))
}

fn spawn_fixed_remote_probe_child() -> Result<(Child, UnixStream), &'static str> {
    let (parent, child_socket) =
        UnixStream::pair().map_err(|_| "clipboard_guard_socketpair_failed")?;
    parent
        .set_nonblocking(true)
        .map_err(|_| "clipboard_guard_socket_nonblocking_failed")?;
    let inherited_fd = child_socket.as_raw_fd();
    let executable = std::env::current_exe().map_err(|_| "clipboard_guard_executable_unknown")?;
    let mut command = Command::new(executable);
    command
        .arg("--clipboard-guard-remote-probe-child")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(inherited_fd, CHILD_CONTROL_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let flags = libc::fcntl(CHILD_CONTROL_FD, libc::F_GETFD);
            if flags < 0
                || libc::fcntl(CHILD_CONTROL_FD, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command
        .spawn()
        .map_err(|_| "clipboard_guard_child_spawn_failed")?;
    drop(child_socket);
    Ok((child, parent))
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn spawn_fixed_registered_owned_paste_child() -> Result<(Child, UnixStream), &'static str> {
    let (parent, child_socket) =
        UnixStream::pair().map_err(|_| "owned_paste_session_socketpair_failed")?;
    parent
        .set_nonblocking(true)
        .map_err(|_| "owned_paste_session_socket_nonblocking_failed")?;
    let inherited_fd = child_socket.as_raw_fd();
    let executable =
        std::env::current_exe().map_err(|_| "owned_paste_session_executable_unknown")?;
    let mut command = Command::new(executable);
    command
        .arg("--owned-paste-session-child")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The same-user inherited-FD boundary is the private self-exec protocol's
    // trust boundary; the opaque lease identifier is only correlation data.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(inherited_fd, CHILD_CONTROL_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let flags = libc::fcntl(CHILD_CONTROL_FD, libc::F_GETFD);
            if flags < 0
                || libc::fcntl(CHILD_CONTROL_FD, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command
        .spawn()
        .map_err(|_| "owned_paste_session_child_spawn_failed")?;
    drop(child_socket);
    Ok((child, parent))
}

fn wait_for_child_status(child: &mut Child) -> ExitStatus {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status,
            Ok(None) => {}
            Err(_) => {
                if let Ok(status) = child.wait() {
                    return status;
                }
            }
        }
        std::thread::sleep(CHILD_POLL_INTERVAL);
    }
}

fn set_child_fd_cloexec(fd: RawFd) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn opaque_local_lease_id() -> u64 {
    (u64::from(std::process::id()) << 32)
        ^ std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos() as u64)
}

fn is_unknown_owner_state(reply: ClipboardGuardReplyV1) -> bool {
    matches!(
        reply,
        ClipboardGuardReplyV1::CaptureUnknown
            | ClipboardGuardReplyV1::WritePartialUnknown
            | ClipboardGuardReplyV1::ReadUnknown
            | ClipboardGuardReplyV1::RestoreUnknown
    )
}

fn owner_requires_retention(reply: ClipboardGuardReplyV1) -> bool {
    is_unknown_owner_state(reply) || reply == ClipboardGuardReplyV1::WriteOwned
}

fn close_after_resolved(
    owner: &mut crate::paste::ResearchClipboardGuardOwnerV1,
    result: ClipboardGuardReplyV1,
) -> ClipboardGuardReplyV1 {
    if matches!(
        result,
        ClipboardGuardReplyV1::Restored | ClipboardGuardReplyV1::NotWritten
    ) {
        owner.close_after_restore()
    } else {
        result
    }
}

fn close_after_general_resolution(
    owner: &mut crate::paste::ResearchClipboardGuardOwnerV1,
    result: ClipboardGuardReplyV1,
) -> ClipboardGuardReplyV1 {
    if matches!(
        result,
        ClipboardGuardReplyV1::Restored
            | ClipboardGuardReplyV1::NotWritten
            | ClipboardGuardReplyV1::OwnershipChanged
    ) {
        owner.close_after_restore()
    } else {
        result
    }
}

fn parse_remote_probe_arguments(
    args: &[OsString],
) -> Result<RegisteredNamedRemoteClipboardCaseV1, &'static str> {
    if args.len() != 2 || args[0].to_str() != Some("--case") {
        return Err(
            "usage: voiceflow-learning-harness --clipboard-guard-remote-probe --case FIXED_CASE",
        );
    }
    args[1]
        .to_str()
        .and_then(RegisteredNamedRemoteClipboardCaseV1::parse)
        .ok_or("clipboard_guard_remote_case_unsupported")
}

pub(crate) fn run_remote_probe_component_cli(args: impl IntoIterator<Item = OsString>) -> i32 {
    let args = args.into_iter().collect::<Vec<_>>();
    let case = match parse_remote_probe_arguments(&args) {
        Ok(case) => case,
        Err(reason) => {
            write_receipt(&serde_json::json!({
                "protocol": "voiceflow_named_remote_clipboard_probe_v1",
                "ok": false,
                "reason": reason,
                "clipboard": "named_fixture_only",
                "general_clipboard": "not_touched",
                "cmd_v": "not_run",
                "target_dispatch_count": 0,
            }));
            return 2;
        }
    };

    let mut owner = match crate::paste::ResearchClipboardGuardOwnerV1::capture_named_fixture(
        ClipboardSeedFixtureV1::OneItemMultiFlavor,
    ) {
        Ok(owner) => owner,
        Err(reason) => {
            write_receipt(&serde_json::json!({
                "protocol": "voiceflow_named_remote_clipboard_probe_v1",
                "ok": false,
                "case": case.code(),
                "capture": reason,
                "child_started": false,
                "clipboard": "named_fixture_only",
                "general_clipboard": "not_touched",
                "cmd_v": "not_run",
                "target_dispatch_count": 0,
            }));
            return 1;
        }
    };

    if case == RegisteredNamedRemoteClipboardCaseV1::PrewriteExternalCopy {
        if let Err(injected) = owner.inject_named_external_copy_for_probe() {
            let retained = owner_requires_retention(injected);
            write_receipt(&serde_json::json!({
                "protocol": "voiceflow_named_remote_clipboard_probe_v1",
                "ok": false,
                "case": case.code(),
                "prewrite_external_copy": injected,
                "child_started": false,
                "owner_retained": retained,
                "general_clipboard": "not_touched",
                "cmd_v": "not_run",
                "target_dispatch_count": 0,
            }));
            if retained {
                hold_unknown_owner(&mut owner);
            }
            return 1;
        }
    }

    let lease_id = opaque_local_lease_id().max(1);
    let (mut child, mut parent_socket) = match spawn_fixed_remote_probe_child() {
        Ok(spawned) => spawned,
        Err(reason) => {
            let state = owner.current_ownership();
            let result = if state == ClipboardGuardReplyV1::WriteOwned {
                owner.restore_if_owned()
            } else {
                state
            };
            let result = close_after_resolved(&mut owner, result);
            let retained = owner_requires_retention(result);
            write_receipt(&serde_json::json!({
                "protocol": "voiceflow_named_remote_clipboard_probe_v1",
                "ok": false,
                "case": case.code(),
                "child_started": false,
                "reason": reason,
                "owner_result": result,
                "owner_retained": retained,
                "general_clipboard": "not_touched",
                "cmd_v": "not_run",
                "target_dispatch_count": 0,
            }));
            if retained {
                hold_unknown_owner(&mut owner);
            }
            return 1;
        }
    };

    let ready = ClipboardGuardRpcFrameV1 {
        protocol_version: PROTOCOL_VERSION,
        lease_id,
        sequence: 0,
        body: ClipboardGuardRpcBodyV1::Reply {
            outcome: ClipboardGuardRpcOutcomeV1::Ready { case, board: None },
        },
    };
    if send_frame(&mut parent_socket, &ready).is_err() {
        drop(parent_socket);
        let status = wait_for_child_status(&mut child);
        let state = owner.current_ownership();
        let result = close_after_resolved(&mut owner, state);
        let retained = owner_requires_retention(result);
        write_receipt(&serde_json::json!({
            "protocol": "voiceflow_named_remote_clipboard_probe_v1",
            "ok": false,
            "case": case.code(),
            "child_exit_confirmed": true,
            "child_exit_status": status.code(),
            "owner_result": result,
            "owner_retained": retained,
            "reason": "guard_ready_send_failed",
            "general_clipboard": "not_touched",
            "cmd_v": "not_run",
            "target_dispatch_count": 0,
        }));
        if retained {
            hold_unknown_owner(&mut owner);
        }
        return 1;
    }

    let protocol =
        run_remote_probe_child_protocol(&mut child, &mut parent_socket, lease_id, case, &mut owner);
    // The child is never allowed to request an immediate restore. Drop the
    // request channel, confirm that the fixed child exited, then re-check
    // ownership and perform the parent-only restore.
    drop(parent_socket);
    let status = wait_for_child_status(&mut child);
    let owner_state_after_wait = owner.current_ownership();
    let owner_result = match owner_state_after_wait {
        ClipboardGuardReplyV1::WriteOwned => owner.restore_if_owned(),
        state => state,
    };
    let owner_result = close_after_resolved(&mut owner, owner_result);
    let retained = owner_requires_retention(owner_result);
    let child_exit_confirmed = true;
    let successful = remote_probe_case_succeeded(case, &protocol, status.success(), owner_result);

    write_receipt(&serde_json::json!({
        "protocol": "voiceflow_named_remote_clipboard_probe_v1",
        "ok": successful && !retained,
        "case": case.code(),
        "parent_protocol_outcome": remote_parent_outcome_code(protocol.outcome),
        "operation_count": protocol.operation_count,
        "parent_write_attempt_count": protocol.parent_write_attempt_count,
        "write_proof_saved_before_ack": protocol.write_proof_saved_before_ack,
        "parent_permit_count": protocol.parent_permit_count,
        "parent_may_have_posted": protocol.parent_may_have_posted,
        "child_callback_count": protocol.child_callback_count,
        "shortcut_sent": protocol.shortcut_sent,
        "target_dispatch_count": protocol.target_dispatch_count,
        "parent_restore_deferred_ack_sent": protocol.parent_restore_deferred_ack_sent,
        "child_reported_restore": protocol.child_reported_restore,
        "child_exit_confirmed": child_exit_confirmed,
        "child_exit_status": status.code(),
        "eof_observed_while_child_alive": protocol.outcome == RemoteParentProtocolOutcomeV1::EofChildAlive,
        "parent_restore_after_true_child_wait": owner_result == ClipboardGuardReplyV1::Restored,
        "parent_owner_result": owner_result,
        "owner_retained": retained,
        "named_external_copy_preserved": case == RegisteredNamedRemoteClipboardCaseV1::PrewriteExternalCopy
            && owner_result == ClipboardGuardReplyV1::OwnershipChanged,
        "general_clipboard": "not_touched",
        "cmd_v": "not_run",
        "accessibility": "not_used",
        "host": "not_started",
    }));

    if retained {
        hold_unknown_owner(&mut owner);
    }
    if successful && !retained {
        0
    } else {
        1
    }
}

fn parse_general_remote_probe_arguments(args: &[OsString]) -> Result<(), &'static str> {
    if args.is_empty() {
        Ok(())
    } else {
        Err("general_remote_probe_arguments_rejected")
    }
}

pub(crate) fn run_general_remote_probe_component_cli(
    args: impl IntoIterator<Item = OsString>,
) -> i32 {
    let args = args.into_iter().collect::<Vec<_>>();
    if parse_general_remote_probe_arguments(&args).is_err() {
        let receipt =
            GeneralRemoteClipboardProbeReceiptV1::new(GeneralRemoteProbeStateV1::ArgumentsRejected);
        write_receipt(&receipt);
        return 2;
    }

    let mut receipt =
        GeneralRemoteClipboardProbeReceiptV1::new(GeneralRemoteProbeStateV1::CaptureFailed);
    let mut owner = match crate::paste::ResearchClipboardGuardOwnerV1::capture_general() {
        Ok(owner) => {
            receipt.capture_state = GeneralRemoteCaptureStateV1::Captured;
            owner
        }
        Err(reason) => {
            receipt.capture_state = match reason {
                ClipboardGuardReplyV1::CaptureOverBudget => GeneralRemoteCaptureStateV1::OverBudget,
                _ => GeneralRemoteCaptureStateV1::Unknown,
            };
            receipt.capture_result = Some(reason);
            write_receipt(&receipt);
            return 1;
        }
    };

    let case = RegisteredNamedRemoteClipboardCaseV1::Positive;
    let lease_id = opaque_local_lease_id().max(1);
    let (mut child, mut parent_socket) = match spawn_fixed_remote_probe_child() {
        Ok(spawned) => spawned,
        Err(_) => {
            let state = owner.current_ownership();
            let result = if state == ClipboardGuardReplyV1::WriteOwned {
                owner.restore_if_owned()
            } else {
                state
            };
            let result = close_after_general_resolution(&mut owner, result);
            let retained = owner_requires_retention(result);
            receipt.state = GeneralRemoteProbeStateV1::ChildStartFailed;
            receipt.parent_owner_result = Some(result);
            receipt.owner_retained = retained;
            write_receipt(&receipt);
            if retained {
                hold_unknown_general_owner(&mut owner);
            }
            return 1;
        }
    };

    let ready = ClipboardGuardRpcFrameV1 {
        protocol_version: PROTOCOL_VERSION,
        lease_id,
        sequence: 0,
        body: ClipboardGuardRpcBodyV1::Reply {
            outcome: ClipboardGuardRpcOutcomeV1::Ready {
                case,
                board: Some(RemoteClipboardBoardV1::GeneralClipboard),
            },
        },
    };
    if send_frame(&mut parent_socket, &ready).is_err() {
        drop(parent_socket);
        let status = wait_for_child_status(&mut child);
        let state = owner.current_ownership();
        let state = if state == ClipboardGuardReplyV1::WriteOwned {
            owner.restore_if_owned()
        } else {
            state
        };
        let result = close_after_general_resolution(&mut owner, state);
        let retained = owner_requires_retention(result);
        receipt.state = GeneralRemoteProbeStateV1::ReadySendFailed;
        receipt.child_started = true;
        receipt.child_exit_confirmed = true;
        receipt.child_exit_status = status.code();
        receipt.parent_owner_result = Some(result);
        receipt.parent_cleanup_after_true_child_wait = Some(true);
        receipt.owner_retained = retained;
        write_receipt(&receipt);
        if retained {
            hold_unknown_general_owner(&mut owner);
        }
        return 1;
    }

    let protocol =
        run_remote_probe_child_protocol(&mut child, &mut parent_socket, lease_id, case, &mut owner);
    drop(parent_socket);
    let status = wait_for_child_status(&mut child);
    let owner_state_after_wait = owner.current_ownership();
    let attempted_restore = owner_state_after_wait == ClipboardGuardReplyV1::WriteOwned;
    let owner_result = if attempted_restore {
        owner.restore_if_owned()
    } else {
        owner_state_after_wait
    };
    let owner_result = close_after_general_resolution(&mut owner, owner_result);
    let retained = owner_requires_retention(owner_result);
    let successful = remote_probe_case_succeeded(case, &protocol, status.success(), owner_result);
    receipt.state = if successful && !retained {
        GeneralRemoteProbeStateV1::Completed
    } else if retained || protocol.outcome == RemoteParentProtocolOutcomeV1::Unknown {
        GeneralRemoteProbeStateV1::Unknown
    } else {
        GeneralRemoteProbeStateV1::ProtocolFailed
    };
    receipt.child_started = true;
    receipt.child_exit_confirmed = true;
    receipt.child_exit_status = status.code();
    receipt.parent_protocol_outcome = Some(protocol.outcome);
    receipt.operation_count = protocol.operation_count;
    receipt.parent_write_attempt_count = protocol.parent_write_attempt_count;
    receipt.write_proof_saved_before_ack = protocol.write_proof_saved_before_ack;
    receipt.parent_permit_count = protocol.parent_permit_count;
    receipt.parent_may_have_posted = protocol.parent_may_have_posted;
    receipt.child_callback_count = protocol.child_callback_count;
    receipt.shortcut_sent = protocol.shortcut_sent;
    receipt.target_dispatch_count = protocol.target_dispatch_count;
    receipt.parent_restore_deferred_ack_sent = protocol.parent_restore_deferred_ack_sent;
    receipt.child_reported_restore = protocol.child_reported_restore;
    receipt.parent_owner_result = Some(owner_result);
    receipt.parent_cleanup_after_true_child_wait = Some(true);
    receipt.owner_retained = retained;
    write_receipt(&receipt);
    if retained {
        hold_unknown_general_owner(&mut owner);
    }
    if successful && !retained {
        0
    } else {
        1
    }
}

pub(crate) fn run_remote_probe_child_cli(args: impl IntoIterator<Item = OsString>) -> i32 {
    if args.into_iter().next().is_some() || set_child_fd_cloexec(CHILD_CONTROL_FD).is_err() {
        return 2;
    }
    let mut stream = unsafe { UnixStream::from_raw_fd(CHILD_CONTROL_FD) };
    let (lease_id, case, board) = match read_guard_rpc_frame(&mut stream) {
        Ok(Some(ClipboardGuardRpcFrameV1 {
            protocol_version: PROTOCOL_VERSION,
            lease_id,
            sequence: 0,
            body:
                ClipboardGuardRpcBodyV1::Reply {
                    outcome: ClipboardGuardRpcOutcomeV1::Ready { case, board },
                },
        })) if lease_id != 0 => (
            lease_id,
            case,
            board.unwrap_or(RemoteClipboardBoardV1::NamedFixture),
        ),
        _ => return 2,
    };
    if board == RemoteClipboardBoardV1::GeneralClipboard
        && case != RegisteredNamedRemoteClipboardCaseV1::Positive
    {
        return 2;
    }
    let client = Arc::new(SocketClipboardGuardRpcClientV1::new(stream, lease_id, case));
    let child_input = RemoteProbeChildInputV1 {
        client: Arc::clone(&client) as Arc<dyn ClipboardGuardRpcClientV1>,
        lease_id,
        case,
        board,
    };
    let result = match board {
        RemoteClipboardBoardV1::NamedFixture => {
            crate::paste::run_research_named_remote_clipboard_probe(child_input)
        }
        RemoteClipboardBoardV1::GeneralClipboard => {
            crate::paste::run_research_remote_clipboard_probe(child_input)
        }
    };
    // This synthetic EOF is used to prove the parent waits for true child exit
    // before restoring the board; it never exercises the system pasteboard.
    if case == RegisteredNamedRemoteClipboardCaseV1::EofChildAlive {
        client.close();
        std::thread::sleep(EOF_CHILD_ALIVE_DELAY);
    }
    let expected_result = match case {
        RegisteredNamedRemoteClipboardCaseV1::Positive
        | RegisteredNamedRemoteClipboardCaseV1::EofChildAlive => {
            result.result == RemoteProbeChildResultKindV1::Completed
                && result.child_callback_count == 1
                && result.restore == RemoteProbeChildRestoreV1::Deferred
        }
        RegisteredNamedRemoteClipboardCaseV1::WriteAckLost
        | RegisteredNamedRemoteClipboardCaseV1::PermitAckLost => {
            result.result == RemoteProbeChildResultKindV1::AcknowledgementUnavailable
                && result.child_callback_count == 0
                && result.restore == RemoteProbeChildRestoreV1::Unknown
        }
        RegisteredNamedRemoteClipboardCaseV1::PermitDenied => {
            result.result == RemoteProbeChildResultKindV1::PermitDenied
                && result.child_callback_count == 0
                && result.restore == RemoteProbeChildRestoreV1::Deferred
        }
        RegisteredNamedRemoteClipboardCaseV1::PrewriteExternalCopy => {
            result.result == RemoteProbeChildResultKindV1::PrewriteOwnershipChanged
                && result.child_callback_count == 0
                && result.restore == RemoteProbeChildRestoreV1::NotNeeded
        }
        RegisteredNamedRemoteClipboardCaseV1::WrongSequence
        | RegisteredNamedRemoteClipboardCaseV1::WrongWriteId
        | RegisteredNamedRemoteClipboardCaseV1::ReplayWrite => {
            result.result == RemoteProbeChildResultKindV1::ProtocolRejected
                && result.child_callback_count == 0
                && result.restore == RemoteProbeChildRestoreV1::Unknown
        }
    };
    if result.shortcut_sent || result.target_dispatch_count != 0 || !expected_result {
        return 1;
    }
    0
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
pub(crate) fn run_registered_owned_paste_session_child_cli(
    args: impl IntoIterator<Item = OsString>,
) -> i32 {
    if args.into_iter().next().is_some() || set_child_fd_cloexec(CHILD_CONTROL_FD).is_err() {
        return 2;
    }
    let mut stream = unsafe { UnixStream::from_raw_fd(CHILD_CONTROL_FD) };
    let (lease_id, authority) =
        match read_frame::<RegisteredOwnedPasteSessionReadyFrameV1>(&mut stream) {
            Ok(Some(RegisteredOwnedPasteSessionReadyFrameV1 {
                protocol_version: PROTOCOL_VERSION,
                lease_id,
                sequence: 0,
                body: RegisteredOwnedPasteSessionReadyBodyV1::Ready { authority },
            })) if lease_id != 0 => (lease_id, authority),
            _ => return 2,
        };
    // The inherited socket is accepted only from this binary's actual parent.
    // This is a same-user harness boundary, not cryptographic authentication;
    // PID/Host checks and the parent's one-use permit still gate IO.
    // SAFETY: these calls only query process metadata.
    let actual_parent_pid = unsafe { libc::getppid() };
    // SAFETY: getuid only returns the calling process' real uid.
    let actual_user_id = unsafe { libc::getuid() };
    // SAFETY: getpid only returns the calling process' PID.
    let actual_child_pid = unsafe { libc::getpid() };
    if !registered_owned_paste_child_parent_is_allowed(
        &authority,
        actual_child_pid,
        actual_parent_pid,
        actual_user_id,
        registered_owned_paste_parent_is_current_binary(actual_parent_pid),
    ) || !registered_owned_paste_authority_contract_is_valid(&authority)
        || registered_owned_paste_host_identity(&authority.host_identity, authority.helper_pid)
            .is_none()
        || crate::learning_research::validate_registered_owned_paste_binding_fields(
            &authority.binding,
        )
        .is_err()
    {
        return 2;
    }
    let Some(expires_at) = instant_from_monotonic_deadline(authority.expires_at_monotonic_ns)
    else {
        return 2;
    };
    let Some(host_identity) =
        registered_owned_paste_host_identity(&authority.host_identity, authority.helper_pid)
    else {
        return 2;
    };
    if !crate::paste::registered_research_host_matches(&host_identity) {
        return 2;
    }

    let client =
        Arc::new(SocketClipboardGuardRpcClientV1::new_registered_owned_session(stream, lease_id));
    let result = crate::learning_research::execute_registered_owned_paste_session_child(
        RegisteredOwnedPasteSessionChildInputV1 {
            remote: RegisteredOwnedPasteSessionRemoteContextV1 {
                client: Arc::clone(&client) as Arc<dyn ClipboardGuardRpcClientV1>,
                lease_id,
                plan_id: authority.plan_id.clone(),
                phase_id: authority.phase_id.clone(),
                operation_id: authority.operation_id.clone(),
            },
            authority,
            host_identity,
            expires_at,
            cancellation: tokio_util::sync::CancellationToken::new(),
        },
    );
    client.close();
    if result == RegisteredOwnedPasteChildResultKindV1::Completed {
        0
    } else {
        1
    }
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_owned_paste_supervisor_matches(
    authority: &RegisteredOwnedPasteSessionAuthorityV1,
    actual_parent_pid: i32,
    actual_user_id: u32,
) -> bool {
    authority.supervisor_pid > 0
        && authority.supervisor_pid == actual_parent_pid
        && authority.host_identity.user_id == actual_user_id
}

fn run_remote_probe_child_protocol(
    child: &mut Child,
    socket: &mut UnixStream,
    lease_id: u64,
    case: RegisteredNamedRemoteClipboardCaseV1,
    owner: &mut crate::paste::ResearchClipboardGuardOwnerV1,
) -> RemoteParentProtocolReportV1 {
    let mut report = RemoteParentProtocolReportV1::new(RemoteParentProtocolOutcomeV1::Unknown);
    let mut buffer = Vec::new();
    let mut expected_sequence = 1_u8;
    let mut capture_checked = false;
    let mut prewrite_checked = false;
    let mut write_id = None;
    let mut write_attempted = false;
    let mut marker_check_seen = false;
    let mut pre_boundary_check_seen = false;
    let mut permit_attempted = false;
    let mut permit_granted = false;
    let mut post_callback_check_seen = false;
    let mut pre_restore_check_seen = false;
    let mut last_child_callback_count = 0_u8;
    let mut restore_requested = false;

    loop {
        let frame = match read_remote_request_nonblocking(socket, &mut buffer, child) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                report.outcome = match child.try_wait() {
                    Ok(None) if case == RegisteredNamedRemoteClipboardCaseV1::EofChildAlive => {
                        RemoteParentProtocolOutcomeV1::EofChildAlive
                    }
                    _ => RemoteParentProtocolOutcomeV1::Unknown,
                };
                return report;
            }
            Err(_) => {
                report.outcome = RemoteParentProtocolOutcomeV1::Unknown;
                return report;
            }
        };
        let ClipboardGuardRpcBodyV1::Request { operation } = frame.body else {
            report.outcome = RemoteParentProtocolOutcomeV1::Rejected;
            return report;
        };
        report.operation_count = report.operation_count.saturating_add(1);

        if frame.protocol_version != PROTOCOL_VERSION || frame.lease_id != lease_id {
            let _ = send_remote_reply(
                socket,
                lease_id,
                frame.sequence,
                ClipboardGuardRpcOutcomeV1::Rejected,
            );
            report.outcome = RemoteParentProtocolOutcomeV1::Rejected;
            return report;
        }
        if frame.sequence != expected_sequence || frame.sequence > 10 {
            let _ = send_remote_reply(
                socket,
                lease_id,
                frame.sequence,
                ClipboardGuardRpcOutcomeV1::Rejected,
            );
            report.outcome = RemoteParentProtocolOutcomeV1::Rejected;
            return report;
        }

        let sequence = expected_sequence;
        expected_sequence = match expected_sequence.checked_add(1) {
            Some(next) if next <= 11 => next,
            _ => {
                report.outcome = RemoteParentProtocolOutcomeV1::Rejected;
                return report;
            }
        };

        let (outcome, finish, suppress_ack) = match operation {
            ClipboardGuardOperationV1::CaptureFreshUnchanged
                if !write_attempted && !restore_requested =>
            {
                match owner.fresh_unchanged_since_capture() {
                    Ok(true) => {
                        capture_checked = true;
                        (ClipboardGuardRpcOutcomeV1::SnapshotUnchanged, None, false)
                    }
                    Ok(false) => (
                        ClipboardGuardRpcOutcomeV1::OwnershipChanged,
                        Some(RemoteParentProtocolOutcomeV1::PrewriteOwnershipChanged),
                        false,
                    ),
                    Err(reason) => (
                        remote_rpc_outcome_for_owner(reason),
                        Some(RemoteParentProtocolOutcomeV1::Unknown),
                        false,
                    ),
                }
            }
            ClipboardGuardOperationV1::CheckUnchangedSinceCapture
                if !write_attempted && !restore_requested =>
            {
                match owner.fresh_unchanged_since_capture() {
                    Ok(true) => {
                        prewrite_checked = true;
                        (ClipboardGuardRpcOutcomeV1::SnapshotUnchanged, None, false)
                    }
                    Ok(false) => (
                        ClipboardGuardRpcOutcomeV1::OwnershipChanged,
                        Some(RemoteParentProtocolOutcomeV1::PrewriteOwnershipChanged),
                        false,
                    ),
                    Err(reason) => (
                        remote_rpc_outcome_for_owner(reason),
                        Some(RemoteParentProtocolOutcomeV1::Unknown),
                        false,
                    ),
                }
            }
            ClipboardGuardOperationV1::WriteRegisteredTemporaryText
                if !write_attempted
                    && !restore_requested
                    && capture_checked
                    && prewrite_checked =>
            {
                write_attempted = true;
                report.parent_write_attempt_count =
                    report.parent_write_attempt_count.saturating_add(1);
                let result =
                    owner.write_fixture(ClipboardWriteFixtureV1::RegisteredTemporaryTextV1);
                if result == ClipboardGuardReplyV1::WriteOwned {
                    report.write_proof_saved_before_ack = true;
                    let generated = opaque_local_lease_id().max(1);
                    write_id = Some(generated);
                    (
                        ClipboardGuardRpcOutcomeV1::WriteOwned {
                            write_id: generated,
                        },
                        None,
                        case == RegisteredNamedRemoteClipboardCaseV1::WriteAckLost,
                    )
                } else {
                    (
                        remote_rpc_outcome_for_owner(result),
                        Some(RemoteParentProtocolOutcomeV1::Unknown),
                        false,
                    )
                }
            }
            ClipboardGuardOperationV1::CheckCurrent {
                write_id: requested_write_id,
                stage,
                child_callback_count,
            } if write_attempted
                && (!restore_requested || report.parent_restore_deferred_ack_sent) =>
            {
                let stage_valid = match stage {
                    ClipboardGuardCheckStageV1::MarkerAfterWrite => {
                        !permit_attempted && child_callback_count == 0
                    }
                    ClipboardGuardCheckStageV1::PreBoundary => {
                        !permit_attempted && marker_check_seen && child_callback_count == 0
                    }
                    ClipboardGuardCheckStageV1::PostCallback => {
                        permit_attempted
                            && !pre_restore_check_seen
                            && (permit_granted || child_callback_count == 0)
                            && child_callback_count <= 1
                            && child_callback_count >= last_child_callback_count
                    }
                    ClipboardGuardCheckStageV1::PreRestore => {
                        (!permit_attempted || post_callback_check_seen)
                            && child_callback_count == last_child_callback_count
                    }
                };
                if write_id != Some(requested_write_id) || !stage_valid {
                    (
                        ClipboardGuardRpcOutcomeV1::Rejected,
                        Some(RemoteParentProtocolOutcomeV1::Rejected),
                        false,
                    )
                } else {
                    let current = owner.current_ownership();
                    let current_outcome = remote_rpc_outcome_for_owner(current);
                    if current == ClipboardGuardReplyV1::WriteOwned {
                        report.child_callback_count = child_callback_count;
                        last_child_callback_count = child_callback_count;
                        match stage {
                            ClipboardGuardCheckStageV1::MarkerAfterWrite => {
                                marker_check_seen = true;
                            }
                            ClipboardGuardCheckStageV1::PreBoundary => {
                                pre_boundary_check_seen = true;
                            }
                            ClipboardGuardCheckStageV1::PostCallback => {
                                post_callback_check_seen = true;
                            }
                            ClipboardGuardCheckStageV1::PreRestore => {
                                pre_restore_check_seen = true;
                            }
                        }
                    }
                    (current_outcome, None, false)
                }
            }
            ClipboardGuardOperationV1::PermitShortcutPost {
                write_id: requested,
            } if write_id == Some(requested)
                && write_attempted
                && pre_boundary_check_seen
                && !permit_attempted
                && !restore_requested =>
            {
                permit_attempted = true;
                let current = owner.current_ownership();
                if current != ClipboardGuardReplyV1::WriteOwned {
                    (
                        remote_rpc_outcome_for_owner(current),
                        Some(RemoteParentProtocolOutcomeV1::Unknown),
                        false,
                    )
                } else if case == RegisteredNamedRemoteClipboardCaseV1::PermitDenied {
                    (ClipboardGuardRpcOutcomeV1::PermitDenied, None, false)
                } else {
                    // The supervisor records authorization before the ACK.
                    // This count is a parent-side permit, never a claim that
                    // a shortcut or target dispatch happened.
                    permit_granted = true;
                    report.parent_permit_count = 1;
                    report.parent_may_have_posted = true;
                    (
                        ClipboardGuardRpcOutcomeV1::PostPermitRecordedOnly {
                            parent_permit_count: 1,
                        },
                        None,
                        case == RegisteredNamedRemoteClipboardCaseV1::PermitAckLost,
                    )
                }
            }
            ClipboardGuardOperationV1::RequestRestoreIfOwned {
                write_id: requested,
            } if write_id == Some(requested)
                && write_attempted
                && pre_restore_check_seen
                && !restore_requested =>
            {
                restore_requested = true;
                (ClipboardGuardRpcOutcomeV1::RestoreDeferred, None, false)
            }
            ClipboardGuardOperationV1::Close {
                child_callback_count,
                shortcut_sent,
                child_restore,
            } if restore_requested
                && report.parent_restore_deferred_ack_sent
                && child_callback_count == report.child_callback_count
                && child_restore == RemoteProbeChildRestoreV1::Deferred
                && !shortcut_sent =>
            {
                report.shortcut_sent = shortcut_sent;
                report.target_dispatch_count = 0;
                report.child_reported_restore = child_restore;
                (
                    ClipboardGuardRpcOutcomeV1::CloseAcknowledged,
                    Some(RemoteParentProtocolOutcomeV1::Complete),
                    false,
                )
            }
            _ => (
                ClipboardGuardRpcOutcomeV1::Rejected,
                Some(RemoteParentProtocolOutcomeV1::Rejected),
                false,
            ),
        };

        if suppress_ack {
            let _ = socket.shutdown(std::net::Shutdown::Both);
            report.outcome = RemoteParentProtocolOutcomeV1::AcknowledgementUnavailable;
            return report;
        }
        if send_remote_reply(socket, lease_id, sequence, outcome).is_err() {
            report.outcome = RemoteParentProtocolOutcomeV1::AcknowledgementUnavailable;
            return report;
        }
        if outcome == ClipboardGuardRpcOutcomeV1::RestoreDeferred {
            report.parent_restore_deferred_ack_sent = true;
        }
        if let Some(finish) = finish {
            report.outcome = finish;
            return report;
        }
    }
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
pub(crate) fn run_registered_owned_paste_session_parent(
    authority: RegisteredOwnedPasteSessionAuthorityV1,
    owner: &mut crate::paste::ResearchClipboardGuardOwnerV1,
    expires_at: std::time::Instant,
    cancellation: &tokio_util::sync::CancellationToken,
    mut callback: impl FnMut(
        RegisteredOwnedPasteParentCallbackV1,
    ) -> RegisteredOwnedPasteParentCallbackResultV1,
) -> RegisteredOwnedPasteSessionParentReportV1 {
    if !registered_owned_paste_authority_is_valid(&authority)
        || expires_at <= std::time::Instant::now()
        || cancellation.is_cancelled()
    {
        return RegisteredOwnedPasteSessionParentReportV1::new(
            RegisteredOwnedPasteParentStateV1::ChildStartFailed,
        );
    }

    let (mut child, mut socket) = match spawn_fixed_registered_owned_paste_child() {
        Ok(pair) => pair,
        Err(_) => {
            return RegisteredOwnedPasteSessionParentReportV1::new(
                RegisteredOwnedPasteParentStateV1::ChildStartFailed,
            );
        }
    };
    let lease_id = opaque_local_lease_id().max(1);
    let mut report =
        RegisteredOwnedPasteSessionParentReportV1::new(RegisteredOwnedPasteParentStateV1::Unknown);
    report.child_started = true;
    let ready = RegisteredOwnedPasteSessionReadyFrameV1 {
        protocol_version: PROTOCOL_VERSION,
        lease_id,
        sequence: 0,
        body: RegisteredOwnedPasteSessionReadyBodyV1::Ready {
            authority: authority.clone(),
        },
    };
    if send_frame(&mut socket, &ready).is_err() {
        drop(socket);
        let status = wait_for_child_status(&mut child);
        report.state = RegisteredOwnedPasteParentStateV1::ReadySendFailed;
        report.child_exit_confirmed = true;
        report.child_exit_status = status.code();
        return report;
    }

    registered_owned_paste_session_protocol(
        RegisteredOwnedPasteSessionProtocolContextV1 {
            child: &mut child,
            socket: &mut socket,
            lease_id,
            authority,
            expires_at,
            cancellation,
        },
        owner,
        &mut callback,
        &mut report,
    );
    drop(socket);
    let status = wait_for_child_status(&mut child);
    report.child_exit_confirmed = true;
    report.child_exit_status = status.code();
    if report.state == RegisteredOwnedPasteParentStateV1::Complete && !status.success() {
        report.state = RegisteredOwnedPasteParentStateV1::Unknown;
    }
    report
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_owned_paste_authority_is_valid(
    authority: &RegisteredOwnedPasteSessionAuthorityV1,
) -> bool {
    registered_owned_paste_authority_matches_pid(authority, std::process::id() as i32)
        && registered_owned_paste_authority_contract_is_valid(authority)
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_owned_paste_authority_matches_pid(
    authority: &RegisteredOwnedPasteSessionAuthorityV1,
    actual_supervisor_pid: i32,
) -> bool {
    actual_supervisor_pid > 0 && authority.supervisor_pid == actual_supervisor_pid
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_owned_paste_authority_contract_is_valid(
    authority: &RegisteredOwnedPasteSessionAuthorityV1,
) -> bool {
    let Some(now_ns) = monotonic_now_ns() else {
        return false;
    };
    registered_owned_paste_authority_contract_is_valid_at(authority, now_ns)
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_owned_paste_authority_contract_is_valid_at(
    authority: &RegisteredOwnedPasteSessionAuthorityV1,
    now_ns: u64,
) -> bool {
    if !registered_owned_paste_deadline_is_valid(now_ns, authority.expires_at_monotonic_ns) {
        return false;
    }
    crate::onboarding::validate_id(&authority.plan_id).is_ok()
        && crate::onboarding::validate_id(&authority.phase_id).is_ok()
        && crate::onboarding::validate_id(&authority.operation_id).is_ok()
        && authority.supervisor_pid > 0
        && registered_owned_paste_host_identity(&authority.host_identity, authority.helper_pid)
            .is_some()
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_owned_paste_deadline_is_valid(now_ns: u64, deadline_ns: u64) -> bool {
    matches!(
        deadline_ns.checked_sub(now_ns),
        Some(remaining) if remaining > 0 && remaining <= 30_000_000_000
    )
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_owned_paste_child_parent_is_allowed(
    authority: &RegisteredOwnedPasteSessionAuthorityV1,
    actual_child_pid: i32,
    actual_parent_pid: i32,
    actual_user_id: u32,
    parent_uses_current_binary: bool,
) -> bool {
    actual_child_pid > 0
        && actual_child_pid != actual_parent_pid
        && parent_uses_current_binary
        && registered_owned_paste_supervisor_matches(authority, actual_parent_pid, actual_user_id)
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_owned_paste_parent_is_current_binary(parent_pid: i32) -> bool {
    use std::ffi::CStr;

    if parent_pid <= 0 {
        return false;
    }
    let Ok(expected_path) = std::env::current_exe() else {
        return false;
    };
    let Ok(expected_path) = expected_path.canonicalize() else {
        return false;
    };
    let mut path_bytes = [0_i8; 4096];
    // SAFETY: `path_bytes` is a writable buffer for the current parent PID.
    let path_len = unsafe {
        libc::proc_pidpath(
            parent_pid,
            path_bytes.as_mut_ptr() as *mut libc::c_void,
            path_bytes.len() as u32,
        )
    };
    if path_len <= 0 || path_len as usize >= path_bytes.len() {
        return false;
    }
    let Ok(actual_path) = unsafe { CStr::from_ptr(path_bytes.as_ptr()) }.to_str() else {
        return false;
    };
    let Ok(actual_path) = std::path::Path::new(actual_path).canonicalize() else {
        return false;
    };
    actual_path == expected_path
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_owned_paste_host_identity(
    identity: &RegisteredOwnedPasteHostIdentityV1,
    helper_pid: i32,
) -> Option<crate::paste::RegisteredResearchHostIdentityV1> {
    if identity.source != "darwin_proc_pidinfo_bsdinfo_proc_pidpath"
        || identity.pid <= 0
        || identity.pid == helper_pid
        || identity.parent_pid != helper_pid
        || identity.parent_pid == identity.pid
        || identity.user_id == 0
        || !std::path::Path::new(&identity.executable_path).is_absolute()
    {
        return None;
    }
    let start_seconds = identity
        .start_seconds
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0 && value.to_string() == identity.start_seconds)?;
    let start_microseconds = identity
        .start_microseconds
        .parse::<u32>()
        .ok()
        .filter(|value| *value < 1_000_000 && value.to_string() == identity.start_microseconds)?;
    Some(crate::paste::RegisteredResearchHostIdentityV1 {
        pid: identity.pid,
        parent_pid: identity.parent_pid,
        user_id: identity.user_id,
        start_seconds,
        start_microseconds,
        executable_path: std::path::PathBuf::from(&identity.executable_path),
    })
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_owned_paste_confirmation_matches(
    authority: &RegisteredOwnedPasteSessionAuthorityV1,
    confirmation: RegisteredOwnedPasteConfirmationV1,
    baseline: RegisteredOwnedPasteBaselineV1,
) -> bool {
    let Ok(expected_hash) = hex::decode(&authority.binding.expected_post_insert_sha256) else {
        return false;
    };
    let Ok(expected_hash) = <[u8; 32]>::try_from(expected_hash.as_slice()) else {
        return false;
    };
    confirmation.verification == RegisteredOwnedPasteVerificationV1::Verified
        && confirmation.target_pid == authority.host_identity.pid
        && confirmation.target_bundle_matches
        && confirmation.source_field_utf16_length == 6
        && confirmation.inserted_field_utf16_length == 14
        && confirmation.replaced_range_start == 6
        && confirmation.replaced_range_length == 0
        && confirmation.inserted_range_start == 6
        && confirmation.inserted_range_length == 8
        && confirmation.inserted_text_utf16_length == 8
        && baseline.state == RegisteredOwnedPasteBaselineStateV1::ObservedExpectedPostInsert
        && baseline.has_value
        && baseline.sha256 == expected_hash
        && baseline.byte_length == authority.binding.expected_post_insert_byte_length
        && baseline.utf16_length == 14
        && baseline.selection_start == 14
        && baseline.selection_length == 0
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_owned_paste_final_matches(
    authority: &RegisteredOwnedPasteSessionAuthorityV1,
    result: RegisteredOwnedPasteEditResultV1,
    sample: RegisteredOwnedPasteFinalSampleV1,
) -> bool {
    let Ok(expected_hash) = hex::decode(&authority.binding.expected_settled_sha256) else {
        return false;
    };
    let Ok(expected_hash) = <[u8; 32]>::try_from(expected_hash.as_slice()) else {
        return false;
    };
    result.state == RegisteredOwnedPasteEditStateV1::Observed
        && result.comparison == RegisteredOwnedPasteComparisonV1::Match
        && result.source == RegisteredOwnedPasteEditSourceV1::AxTargetReaderOwnedDeclaredTargetV1
        && result.has_value
        && result.sha256 == expected_hash
        && result.byte_length == authority.binding.expected_settled_byte_length
        && result.has_edit_receipt
        && sample.state == RegisteredOwnedPasteFinalSampleStateV1::MatchesIndependentAxAndExpected
        && sample.has_value
        && sample.sha256 == expected_hash
        && sample.byte_length == authority.binding.expected_settled_byte_length
        && sample.utf16_length == 15
        && sample.focus_matches
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
struct RegisteredOwnedPasteSessionProtocolContextV1<'a> {
    child: &'a mut Child,
    socket: &'a mut UnixStream,
    lease_id: u64,
    authority: RegisteredOwnedPasteSessionAuthorityV1,
    expires_at: std::time::Instant,
    cancellation: &'a tokio_util::sync::CancellationToken,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_owned_paste_session_protocol(
    context: RegisteredOwnedPasteSessionProtocolContextV1<'_>,
    owner: &mut crate::paste::ResearchClipboardGuardOwnerV1,
    callback: &mut impl FnMut(
        RegisteredOwnedPasteParentCallbackV1,
    ) -> RegisteredOwnedPasteParentCallbackResultV1,
    report: &mut RegisteredOwnedPasteSessionParentReportV1,
) {
    let RegisteredOwnedPasteSessionProtocolContextV1 {
        child,
        socket,
        lease_id,
        authority,
        expires_at,
        cancellation,
    } = context;
    let mut buffer = Vec::new();
    let mut expected_sequence = 1_u8;
    let mut capture_checked = false;
    let mut prewrite_checked = false;
    let mut write_attempted = false;
    let mut write_id = None;
    let mut marker_check_seen = false;
    let mut pre_boundary_check_seen = false;
    let mut permit_attempted = false;
    let mut permit_granted = false;
    let mut post_callback_check_seen = false;
    let mut edit_attempted = false;
    let mut pre_restore_check_seen = false;
    let mut restore_requested = false;
    let mut last_child_callback_count = 0_u8;
    loop {
        let frame = match read_remote_request_nonblocking(socket, &mut buffer, child) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                report.state = RegisteredOwnedPasteParentStateV1::Unknown;
                return;
            }
            Err(_) => {
                report.state = RegisteredOwnedPasteParentStateV1::Unknown;
                return;
            }
        };
        let ClipboardGuardRpcBodyV1::Request { operation } = frame.body else {
            report.state = RegisteredOwnedPasteParentStateV1::Rejected;
            return;
        };
        report.operation_count = report.operation_count.saturating_add(1);
        if frame.protocol_version != PROTOCOL_VERSION
            || frame.lease_id != lease_id
            || frame.sequence != expected_sequence
            || frame.sequence > 11
        {
            let _ = send_remote_reply(
                socket,
                lease_id,
                frame.sequence,
                ClipboardGuardRpcOutcomeV1::Rejected,
            );
            report.state = RegisteredOwnedPasteParentStateV1::Rejected;
            return;
        }
        expected_sequence = match expected_sequence.checked_add(1) {
            Some(next) if next <= 12 => next,
            _ => {
                report.state = RegisteredOwnedPasteParentStateV1::Rejected;
                return;
            }
        };

        let phase_active = std::time::Instant::now() < expires_at && !cancellation.is_cancelled();
        let mut finish = false;
        let outcome = match operation {
            ClipboardGuardOperationV1::CaptureFreshUnchanged
                if !write_attempted && !restore_requested && phase_active =>
            {
                match owner.fresh_unchanged_since_capture() {
                    Ok(true) => {
                        capture_checked = true;
                        ClipboardGuardRpcOutcomeV1::SnapshotUnchanged
                    }
                    Ok(false) => {
                        report.state = RegisteredOwnedPasteParentStateV1::PrewriteOwnershipChanged;
                        ClipboardGuardRpcOutcomeV1::OwnershipChanged
                    }
                    Err(_) => {
                        report.state = RegisteredOwnedPasteParentStateV1::Unknown;
                        ClipboardGuardRpcOutcomeV1::ReadUnknown
                    }
                }
            }
            ClipboardGuardOperationV1::CheckUnchangedSinceCapture
                if !write_attempted && !restore_requested && phase_active =>
            {
                match owner.fresh_unchanged_since_capture() {
                    Ok(true) => {
                        prewrite_checked = true;
                        ClipboardGuardRpcOutcomeV1::SnapshotUnchanged
                    }
                    Ok(false) => {
                        report.state = RegisteredOwnedPasteParentStateV1::PrewriteOwnershipChanged;
                        ClipboardGuardRpcOutcomeV1::OwnershipChanged
                    }
                    Err(_) => {
                        report.state = RegisteredOwnedPasteParentStateV1::Unknown;
                        ClipboardGuardRpcOutcomeV1::ReadUnknown
                    }
                }
            }
            ClipboardGuardOperationV1::WriteRegisteredTemporaryText
                if !write_attempted
                    && !restore_requested
                    && capture_checked
                    && prewrite_checked
                    && phase_active =>
            {
                write_attempted = true;
                report.parent_write_attempt_count = 1;
                match owner.write_registered_temporary_text() {
                    ClipboardGuardReplyV1::WriteOwned => {
                        let generated = opaque_local_lease_id().max(1);
                        write_id = Some(generated);
                        report.write_id = Some(generated);
                        report.write_proof_saved_before_ack = true;
                        ClipboardGuardRpcOutcomeV1::WriteOwned {
                            write_id: generated,
                        }
                    }
                    reply => {
                        report.state = RegisteredOwnedPasteParentStateV1::Unknown;
                        finish = true;
                        remote_rpc_outcome_for_owner(reply)
                    }
                }
            }
            ClipboardGuardOperationV1::CheckCurrent {
                write_id: requested_write_id,
                stage,
                child_callback_count,
            } if write_attempted
                && write_id == Some(requested_write_id)
                && (!restore_requested || report.parent_restore_deferred_ack_sent)
                && child_callback_count <= 1
                && child_callback_count >= last_child_callback_count =>
            {
                let stage_valid = match stage {
                    ClipboardGuardCheckStageV1::MarkerAfterWrite => {
                        !permit_attempted && child_callback_count == 0
                    }
                    ClipboardGuardCheckStageV1::PreBoundary => {
                        !permit_attempted && marker_check_seen && child_callback_count == 0
                    }
                    ClipboardGuardCheckStageV1::PostCallback => {
                        permit_attempted
                            && (permit_granted && child_callback_count == 1
                                || !permit_granted && child_callback_count == 0)
                    }
                    ClipboardGuardCheckStageV1::PreRestore => {
                        post_callback_check_seen
                            && child_callback_count == last_child_callback_count
                    }
                };
                if !stage_valid {
                    report.state = RegisteredOwnedPasteParentStateV1::Rejected;
                    finish = true;
                    ClipboardGuardRpcOutcomeV1::Rejected
                } else {
                    let current = owner.current_ownership();
                    let current_outcome = remote_rpc_outcome_for_owner(current);
                    if current == ClipboardGuardReplyV1::WriteOwned {
                        last_child_callback_count = child_callback_count;
                        match stage {
                            ClipboardGuardCheckStageV1::MarkerAfterWrite => {
                                marker_check_seen = true;
                            }
                            ClipboardGuardCheckStageV1::PreBoundary => {
                                pre_boundary_check_seen = true;
                            }
                            ClipboardGuardCheckStageV1::PostCallback => {
                                post_callback_check_seen = true;
                            }
                            ClipboardGuardCheckStageV1::PreRestore => {
                                pre_restore_check_seen = true;
                            }
                        }
                    } else if matches!(
                        current,
                        ClipboardGuardReplyV1::OwnershipChanged
                            | ClipboardGuardReplyV1::ReadUnknown
                    ) {
                        report.state = RegisteredOwnedPasteParentStateV1::Unknown;
                        finish = true;
                    }
                    current_outcome
                }
            }
            ClipboardGuardOperationV1::PermitShortcutPost {
                write_id: requested,
            } if write_id == Some(requested)
                && write_attempted
                && pre_boundary_check_seen
                && !permit_attempted
                && !restore_requested
                && phase_active =>
            {
                permit_attempted = true;
                if owner.current_ownership() != ClipboardGuardReplyV1::WriteOwned {
                    report.state = RegisteredOwnedPasteParentStateV1::Unknown;
                    finish = true;
                    ClipboardGuardRpcOutcomeV1::PermitDenied
                } else {
                    match callback(RegisteredOwnedPasteParentCallbackV1::RecheckPostAuthority) {
                        RegisteredOwnedPasteParentCallbackResultV1::HostAndHelperLive
                            if std::time::Instant::now() < expires_at
                                && !cancellation.is_cancelled() =>
                        {
                            permit_granted = true;
                            report.parent_permit_count = 1;
                            // This is a possible-post fact set before its ACK;
                            // it does not claim that the child actually posted.
                            report.parent_may_have_posted = true;
                            ClipboardGuardRpcOutcomeV1::PostPermitAllowed {
                                parent_permit_count: 1,
                            }
                        }
                        _ => {
                            report.state = RegisteredOwnedPasteParentStateV1::PermitDenied;
                            ClipboardGuardRpcOutcomeV1::PermitDenied
                        }
                    }
                }
            }
            ClipboardGuardOperationV1::ApplyDeclaredEdit {
                write_id: requested,
                confirmed_insertion,
                baseline,
            } if write_id == Some(requested)
                && permit_granted
                && post_callback_check_seen
                && !edit_attempted
                && restore_requested
                && report.parent_restore_deferred_ack_sent
                && report.parent_may_have_posted
                && phase_active
                && registered_owned_paste_confirmation_matches(
                    &authority,
                    confirmed_insertion,
                    baseline,
                ) =>
            {
                edit_attempted = true;
                report.parent_edit_attempt_count = 1;
                report.child_baseline = baseline;
                match callback(RegisteredOwnedPasteParentCallbackV1::ApplyDeclaredEdit) {
                    RegisteredOwnedPasteParentCallbackResultV1::Edit(result) => {
                        report.parent_edit_result = result;
                        ClipboardGuardRpcOutcomeV1::RegisteredOwnedEditObserved { result }
                    }
                    RegisteredOwnedPasteParentCallbackResultV1::HostAndHelperLive
                    | RegisteredOwnedPasteParentCallbackResultV1::HostOrHelperUnavailable => {
                        let result = registered_owned_paste_edit_unknown();
                        report.parent_edit_result = result;
                        report.state = RegisteredOwnedPasteParentStateV1::Unknown;
                        ClipboardGuardRpcOutcomeV1::RegisteredOwnedEditObserved { result }
                    }
                }
            }
            ClipboardGuardOperationV1::RequestRestoreIfOwned {
                write_id: requested,
            } if write_id == Some(requested) && pre_restore_check_seen && !restore_requested => {
                restore_requested = true;
                if owner.current_ownership() == ClipboardGuardReplyV1::WriteOwned {
                    ClipboardGuardRpcOutcomeV1::RestoreDeferred
                } else {
                    report.state = RegisteredOwnedPasteParentStateV1::Unknown;
                    finish = true;
                    ClipboardGuardRpcOutcomeV1::RestoreUnknown
                }
            }
            ClipboardGuardOperationV1::CloseRegisteredOwnedPasteSession {
                write_id: child_write_id,
                shortcut_sent,
                result,
                baseline,
                final_sample,
                edit_response_received,
                observation_ticket_released,
                owner_joined,
                child_restore,
            } if restore_requested
                && report.parent_restore_deferred_ack_sent
                && child_write_id == write_id
                && child_restore == RemoteProbeChildRestoreV1::Deferred
                && (!edit_attempted || baseline == report.child_baseline) =>
            {
                report.child_result = result;
                report.child_close_received = true;
                report.child_shortcut_sent = shortcut_sent;
                report.child_baseline = baseline;
                report.child_final_sample = final_sample;
                report.child_edit_response_received = edit_response_received;
                report.child_observation_ticket_released = observation_ticket_released;
                report.child_owner_joined = owner_joined;
                report.child_restore = child_restore;
                report.state = if result == RegisteredOwnedPasteChildResultKindV1::Completed
                    && edit_response_received
                    && report.parent_edit_attempt_count == 1
                    && report.parent_edit_result.state == RegisteredOwnedPasteEditStateV1::Observed
                    && observation_ticket_released
                    && owner_joined
                    && shortcut_sent
                    && registered_owned_paste_final_matches(
                        &authority,
                        report.parent_edit_result,
                        final_sample,
                    ) {
                    RegisteredOwnedPasteParentStateV1::Complete
                } else {
                    RegisteredOwnedPasteParentStateV1::Unknown
                };
                finish = true;
                ClipboardGuardRpcOutcomeV1::RegisteredOwnedPasteSessionCloseAcknowledged
            }
            _ => {
                report.state = RegisteredOwnedPasteParentStateV1::Rejected;
                finish = true;
                ClipboardGuardRpcOutcomeV1::Rejected
            }
        };

        if send_remote_reply(socket, lease_id, frame.sequence, outcome).is_err() {
            report.state = RegisteredOwnedPasteParentStateV1::AcknowledgementUnavailable;
            return;
        }
        if outcome == ClipboardGuardRpcOutcomeV1::RestoreDeferred {
            report.parent_restore_deferred_ack_sent = true;
        }
        if let ClipboardGuardRpcOutcomeV1::RegisteredOwnedEditObserved { result } = outcome {
            report.parent_edit_result = result;
        }
        if finish {
            return;
        }
    }
}

fn read_remote_request_nonblocking(
    socket: &mut UnixStream,
    buffer: &mut Vec<u8>,
    child: &mut Child,
) -> Result<Option<ClipboardGuardRpcFrameV1>, &'static str> {
    let mut chunk = [0_u8; 2_048];
    loop {
        if let Some(frame) = take_frame::<ClipboardGuardRpcFrameV1>(buffer)? {
            return Ok(Some(frame));
        }
        match socket.read(&mut chunk) {
            Ok(0) => return Ok(None),
            Ok(count) => {
                if buffer
                    .len()
                    .checked_add(count)
                    .is_none_or(|size| size > MAX_FRAME_BYTES + 4)
                {
                    return Err("clipboard_guard_frame_over_budget");
                }
                buffer.extend_from_slice(&chunk[..count]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if child
                    .try_wait()
                    .map_err(|_| "clipboard_guard_child_status_unknown")?
                    .is_some()
                {
                    match socket.read(&mut chunk) {
                        Ok(0) => return Ok(None),
                        Ok(count) => {
                            buffer.extend_from_slice(&chunk[..count]);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            return Err("clipboard_guard_child_exited_before_request");
                        }
                        Err(_) => return Err("clipboard_guard_socket_read_failed"),
                    }
                } else {
                    std::thread::sleep(CHILD_POLL_INTERVAL);
                }
            }
            Err(_) => return Err("clipboard_guard_socket_read_failed"),
        }
    }
}

fn send_remote_reply(
    socket: &mut UnixStream,
    lease_id: u64,
    sequence: u8,
    outcome: ClipboardGuardRpcOutcomeV1,
) -> std::io::Result<()> {
    send_frame(
        socket,
        &ClipboardGuardRpcFrameV1 {
            protocol_version: PROTOCOL_VERSION,
            lease_id,
            sequence,
            body: ClipboardGuardRpcBodyV1::Reply { outcome },
        },
    )
}

fn remote_rpc_outcome_for_owner(reply: ClipboardGuardReplyV1) -> ClipboardGuardRpcOutcomeV1 {
    match reply {
        ClipboardGuardReplyV1::NotWritten => ClipboardGuardRpcOutcomeV1::NotWritten,
        ClipboardGuardReplyV1::WriteOwned => ClipboardGuardRpcOutcomeV1::CurrentOwned,
        ClipboardGuardReplyV1::WritePartialUnknown => {
            ClipboardGuardRpcOutcomeV1::WritePartialUnknown
        }
        ClipboardGuardReplyV1::OwnershipChanged => ClipboardGuardRpcOutcomeV1::OwnershipChanged,
        ClipboardGuardReplyV1::ReadUnknown
        | ClipboardGuardReplyV1::CaptureUnknown
        | ClipboardGuardReplyV1::RestoreUnknown => ClipboardGuardRpcOutcomeV1::ReadUnknown,
        ClipboardGuardReplyV1::CaptureOverBudget
        | ClipboardGuardReplyV1::RestoreRequested
        | ClipboardGuardReplyV1::Restored
        | ClipboardGuardReplyV1::CloseAcknowledged
        | ClipboardGuardReplyV1::Rejected => ClipboardGuardRpcOutcomeV1::Rejected,
    }
}

fn remote_parent_outcome_code(outcome: RemoteParentProtocolOutcomeV1) -> &'static str {
    match outcome {
        RemoteParentProtocolOutcomeV1::Complete => "complete",
        RemoteParentProtocolOutcomeV1::AcknowledgementUnavailable => "acknowledgement_unavailable",
        RemoteParentProtocolOutcomeV1::PrewriteOwnershipChanged => "prewrite_ownership_changed",
        RemoteParentProtocolOutcomeV1::Rejected => "rejected",
        RemoteParentProtocolOutcomeV1::EofChildAlive => "eof_child_alive",
        RemoteParentProtocolOutcomeV1::Unknown => "unknown",
    }
}

fn remote_probe_case_succeeded(
    case: RegisteredNamedRemoteClipboardCaseV1,
    protocol: &RemoteParentProtocolReportV1,
    child_success: bool,
    owner_result: ClipboardGuardReplyV1,
) -> bool {
    if !child_success
        || protocol.shortcut_sent
        || protocol.target_dispatch_count != 0
        || protocol.parent_write_attempt_count > 1
        || (protocol.parent_write_attempt_count == 1 && !protocol.write_proof_saved_before_ack)
    {
        return false;
    }
    match case {
        RegisteredNamedRemoteClipboardCaseV1::Positive => {
            protocol.outcome == RemoteParentProtocolOutcomeV1::Complete
                && protocol.parent_write_attempt_count == 1
                && protocol.parent_permit_count == 1
                && protocol.parent_may_have_posted
                && protocol.child_callback_count == 1
                && protocol.parent_restore_deferred_ack_sent
                && protocol.child_reported_restore == RemoteProbeChildRestoreV1::Deferred
                && owner_result == ClipboardGuardReplyV1::Restored
        }
        RegisteredNamedRemoteClipboardCaseV1::EofChildAlive => {
            protocol.outcome == RemoteParentProtocolOutcomeV1::EofChildAlive
                && protocol.parent_write_attempt_count == 1
                && protocol.parent_permit_count == 1
                && protocol.parent_may_have_posted
                && protocol.child_callback_count == 1
                && protocol.parent_restore_deferred_ack_sent
                && protocol.child_reported_restore == RemoteProbeChildRestoreV1::Unknown
                && owner_result == ClipboardGuardReplyV1::Restored
        }
        RegisteredNamedRemoteClipboardCaseV1::WriteAckLost => {
            protocol.outcome == RemoteParentProtocolOutcomeV1::AcknowledgementUnavailable
                && protocol.parent_write_attempt_count == 1
                && protocol.parent_permit_count == 0
                && protocol.child_callback_count == 0
                && !protocol.parent_restore_deferred_ack_sent
                && protocol.child_reported_restore == RemoteProbeChildRestoreV1::Unknown
                && owner_result == ClipboardGuardReplyV1::Restored
        }
        RegisteredNamedRemoteClipboardCaseV1::PermitAckLost => {
            protocol.outcome == RemoteParentProtocolOutcomeV1::AcknowledgementUnavailable
                && protocol.parent_write_attempt_count == 1
                && protocol.parent_permit_count == 1
                && protocol.parent_may_have_posted
                && protocol.child_callback_count == 0
                && !protocol.parent_restore_deferred_ack_sent
                && protocol.child_reported_restore == RemoteProbeChildRestoreV1::Unknown
                && owner_result == ClipboardGuardReplyV1::Restored
        }
        RegisteredNamedRemoteClipboardCaseV1::PermitDenied => {
            protocol.outcome == RemoteParentProtocolOutcomeV1::Complete
                && protocol.parent_write_attempt_count == 1
                && protocol.parent_permit_count == 0
                && !protocol.parent_may_have_posted
                && protocol.child_callback_count == 0
                && protocol.parent_restore_deferred_ack_sent
                && protocol.child_reported_restore == RemoteProbeChildRestoreV1::Deferred
                && owner_result == ClipboardGuardReplyV1::Restored
        }
        RegisteredNamedRemoteClipboardCaseV1::PrewriteExternalCopy => {
            protocol.outcome == RemoteParentProtocolOutcomeV1::PrewriteOwnershipChanged
                && protocol.parent_write_attempt_count == 0
                && protocol.parent_permit_count == 0
                && protocol.child_callback_count == 0
                && owner_result == ClipboardGuardReplyV1::OwnershipChanged
        }
        RegisteredNamedRemoteClipboardCaseV1::WrongSequence => {
            protocol.outcome == RemoteParentProtocolOutcomeV1::Rejected
                && protocol.parent_write_attempt_count == 0
                && protocol.parent_permit_count == 0
                && owner_result == ClipboardGuardReplyV1::NotWritten
        }
        RegisteredNamedRemoteClipboardCaseV1::WrongWriteId
        | RegisteredNamedRemoteClipboardCaseV1::ReplayWrite => {
            protocol.outcome == RemoteParentProtocolOutcomeV1::Rejected
                && protocol.parent_write_attempt_count == 1
                && protocol.parent_permit_count == 0
                && protocol.child_callback_count == 0
                && owner_result == ClipboardGuardReplyV1::Restored
        }
    }
}

fn write_receipt<T: Serialize>(value: &T) {
    let mut output = std::io::stdout().lock();
    if let Ok(mut encoded) = serde_json::to_vec(value) {
        encoded.push(b'\n');
        let _ = output.write_all(&encoded).and_then(|_| output.flush());
    }
}

fn hold_unknown_owner(owner: &mut crate::paste::ResearchClipboardGuardOwnerV1) {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    loop {
        let mut line = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            match input.read(&mut byte) {
                Ok(0) => std::thread::sleep(CHILD_POLL_INTERVAL),
                Ok(_) if byte[0] == b'\n' => break,
                Ok(_) if line.len() >= MAX_UNKNOWN_HOLD_COMMAND_BYTES => {
                    line.clear();
                    break;
                }
                Ok(_) => line.push(byte[0]),
                Err(_) => std::thread::sleep(CHILD_POLL_INTERVAL),
            }
        }
        if line.as_slice() == b"release_named_fixture_owner" {
            return;
        }
        if line.as_slice() == b"check_current" {
            write_receipt(&serde_json::json!({
                "protocol": "voiceflow_clipboard_guard_component_v1",
                "owner_result": owner.current_ownership(),
                "owner_retained": true,
            }));
        } else if line.as_slice() == b"restore_if_owned" {
            let restored = owner.restore_if_owned();
            let result = close_after_resolved(owner, restored);
            write_receipt(&serde_json::json!({
                "protocol": "voiceflow_clipboard_guard_component_v1",
                "owner_result": result,
                "owner_retained": owner_requires_retention(result),
            }));
            if !owner_requires_retention(result) {
                return;
            }
        } else {
            write_receipt(&serde_json::json!({
                "protocol": "voiceflow_clipboard_guard_component_v1",
                "ok": false,
                "reason": "unknown_owner_hold_command_rejected",
                "owner_retained": true,
            }));
        }
    }
}

fn hold_unknown_general_owner(owner: &mut crate::paste::ResearchClipboardGuardOwnerV1) {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    loop {
        let mut line = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            match input.read(&mut byte) {
                Ok(0) => std::thread::sleep(CHILD_POLL_INTERVAL),
                Ok(_) if byte[0] == b'\n' => break,
                Ok(_) if line.len() >= MAX_UNKNOWN_HOLD_COMMAND_BYTES => {
                    line.clear();
                    break;
                }
                Ok(_) => line.push(byte[0]),
                Err(_) => std::thread::sleep(CHILD_POLL_INTERVAL),
            }
        }
        if line.as_slice() == b"check_current" {
            let result = owner.current_ownership();
            let terminal = matches!(
                result,
                ClipboardGuardReplyV1::Restored
                    | ClipboardGuardReplyV1::NotWritten
                    | ClipboardGuardReplyV1::OwnershipChanged
            );
            let result = if terminal {
                close_after_general_resolution(owner, result)
            } else {
                result
            };
            let retained = owner_requires_retention(result);
            write_receipt(&serde_json::json!({
                "protocol": "voiceflow_general_clipboard_guard_component_v1",
                "owner_result": result,
                "owner_retained": retained,
            }));
            if !retained {
                return;
            }
        } else if line.as_slice() == b"restore_if_owned" {
            let restore = owner.restore_if_owned();
            let result = close_after_general_resolution(owner, restore);
            let retained = owner_requires_retention(result);
            write_receipt(&serde_json::json!({
                "protocol": "voiceflow_general_clipboard_guard_component_v1",
                "owner_result": result,
                "owner_retained": retained,
            }));
            if !retained {
                return;
            }
        } else {
            // There is deliberately no force-release command on the General
            // board; the original snapshot remains owned while status is
            // unresolved.
            write_receipt(&serde_json::json!({
                "protocol": "voiceflow_general_clipboard_guard_component_v1",
                "ok": false,
                "reason": "general_guard_owner_command_rejected",
                "owner_retained": true,
            }));
        }
    }
}

#[cfg(test)]
mod general_remote_probe_receipt_tests {
    use super::*;

    #[test]
    fn general_remote_probe_arguments_are_exactly_empty() {
        assert_eq!(parse_general_remote_probe_arguments(&[]), Ok(()));
        assert_eq!(
            parse_general_remote_probe_arguments(&[OsString::from("--case")]),
            Err("general_remote_probe_arguments_rejected")
        );
    }

    #[test]
    fn general_ready_is_typed_and_named_ready_keeps_its_old_shape() {
        let named = ClipboardGuardRpcOutcomeV1::Ready {
            case: RegisteredNamedRemoteClipboardCaseV1::Positive,
            board: None,
        };
        let named = serde_json::to_value(named).unwrap();
        assert_eq!(named["kind"], "ready");
        assert_eq!(named["case"], "positive");
        assert!(named.get("board").is_none());

        let general = ClipboardGuardRpcOutcomeV1::Ready {
            case: RegisteredNamedRemoteClipboardCaseV1::Positive,
            board: Some(RemoteClipboardBoardV1::GeneralClipboard),
        };
        let general = serde_json::to_value(general).unwrap();
        assert_eq!(general["kind"], "ready");
        assert_eq!(general["board"], "general_clipboard");
        assert!(
            serde_json::from_value::<ClipboardGuardRpcOutcomeV1>(serde_json::json!({
                "kind": "ready",
                "case": "positive",
                "board": "arbitrary"
            }))
            .is_err()
        );
    }

    #[test]
    fn general_receipt_distinguishes_unreached_cleanup_from_confirmed_cleanup() {
        let mut receipt =
            GeneralRemoteClipboardProbeReceiptV1::new(GeneralRemoteProbeStateV1::ArgumentsRejected);
        let before_wait = serde_json::to_value(&receipt).unwrap();
        assert_eq!(
            before_wait["protocol"],
            "voiceflow_general_remote_clipboard_probe_v1"
        );
        assert_eq!(before_wait["board"], "general_clipboard");
        assert_eq!(
            before_wait["permit_kind"],
            "recorder_only_no_target_authority"
        );
        assert_eq!(before_wait["general_guard_active_for"], false);
        assert_eq!(before_wait["original_contents_readback"], "not_probed");
        assert_eq!(before_wait["accessibility"], "not_probed");
        assert!(before_wait
            .get("parent_cleanup_after_true_child_wait")
            .is_none());
        for forbidden in [
            "original_text",
            "original_hash",
            "ownership_token",
            "flavors",
        ] {
            assert!(before_wait.get(forbidden).is_none());
        }

        receipt.state = GeneralRemoteProbeStateV1::Completed;
        receipt.parent_cleanup_after_true_child_wait = Some(true);
        let after_wait = serde_json::to_value(receipt).unwrap();
        assert_eq!(after_wait["parent_cleanup_after_true_child_wait"], true);
    }
}

#[cfg(all(test, target_os = "macos", feature = "research-harness"))]
mod registered_owned_paste_authority_tests {
    use super::*;

    fn authority() -> RegisteredOwnedPasteSessionAuthorityV1 {
        RegisteredOwnedPasteSessionAuthorityV1 {
            plan_id: "90000000-0000-4000-8000-000000000021".into(),
            phase_id: "90000000-0000-4000-8000-000000000022".into(),
            operation_id: "90000000-0000-4000-8000-000000000023".into(),
            supervisor_pid: 41,
            helper_pid: 42,
            host_identity: RegisteredOwnedPasteHostIdentityV1 {
                source: "darwin_proc_pidinfo_bsdinfo_proc_pidpath".into(),
                pid: 43,
                parent_pid: 42,
                user_id: 501,
                start_seconds: "1".into(),
                start_microseconds: "0".into(),
                executable_path: "/tmp/FixtureHost.app/Contents/MacOS/FixtureHost".into(),
            },
            binding: crate::learning_research::OwnedPasteSessionBindingV1 {
                fixture_id: String::new(),
                source_manifest_sha256: String::new(),
                helper_executable_sha256: String::new(),
                host_executable_sha256: String::new(),
                host_bundle_identifier: String::new(),
                host_info_plist_sha256: String::new(),
                recipe_sha256: String::new(),
                window_title: String::new(),
                control_identifier: String::new(),
                inserted_text_sha256: String::new(),
                inserted_text_byte_length: 0,
                expected_post_insert_sha256: String::new(),
                expected_post_insert_byte_length: 0,
                expected_settled_sha256: String::new(),
                expected_settled_byte_length: 0,
            },
            expires_at_monotonic_ns: monotonic_now_ns().unwrap() + 1_000_000_000,
        }
    }

    fn authority_at(now_ns: u64) -> RegisteredOwnedPasteSessionAuthorityV1 {
        let mut authority = authority();
        authority.expires_at_monotonic_ns = now_ns + 1_000_000_000;
        authority
    }

    #[test]
    fn registered_ready_requires_the_inherited_supervisor_and_user() {
        let authority = authority();
        assert!(registered_owned_paste_supervisor_matches(
            &authority, 41, 501
        ));
        assert!(!registered_owned_paste_supervisor_matches(
            &authority, 99, 501
        ));
        assert!(!registered_owned_paste_supervisor_matches(
            &authority, 41, 502
        ));
        assert!(registered_owned_paste_child_parent_is_allowed(
            &authority, 42, 41, 501, true,
        ));
        assert!(!registered_owned_paste_child_parent_is_allowed(
            &authority, 42, 99, 501, true,
        ));
        assert!(!registered_owned_paste_child_parent_is_allowed(
            &authority, 42, 41, 501, false,
        ));
        assert!(registered_owned_paste_authority_matches_pid(&authority, 41));
        assert!(!registered_owned_paste_authority_matches_pid(
            &authority, 42
        ));
    }

    #[test]
    fn registered_parent_identity_uses_the_running_harness_executable() {
        assert!(registered_owned_paste_parent_is_current_binary(
            std::process::id() as i32,
        ));
        assert!(!registered_owned_paste_parent_is_current_binary(0));
    }

    #[test]
    fn registered_authority_rejects_expired_overlong_and_wrong_host_bindings() {
        let now_ns = 1_000_000_000;
        let mut valid = authority_at(now_ns);
        assert!(registered_owned_paste_authority_contract_is_valid_at(
            &valid, now_ns
        ));
        assert!(registered_owned_paste_deadline_is_valid(
            now_ns,
            now_ns + 30_000_000_000,
        ));
        assert!(!registered_owned_paste_deadline_is_valid(
            now_ns,
            now_ns + 30_000_000_001,
        ));
        assert!(!registered_owned_paste_deadline_is_valid(now_ns, now_ns));

        valid.expires_at_monotonic_ns = now_ns;
        assert!(!registered_owned_paste_authority_contract_is_valid_at(
            &valid, now_ns
        ));

        let mut too_long = authority_at(now_ns);
        too_long.expires_at_monotonic_ns = now_ns + 30_000_000_001;
        assert!(!registered_owned_paste_authority_contract_is_valid_at(
            &too_long, now_ns,
        ));

        let mut wrong_parent = authority_at(now_ns);
        wrong_parent.host_identity.parent_pid += 1;
        assert!(!registered_owned_paste_authority_contract_is_valid_at(
            &wrong_parent,
            now_ns,
        ));

        let mut helper_as_host = authority_at(now_ns);
        helper_as_host.host_identity.pid = helper_as_host.helper_pid;
        assert!(!registered_owned_paste_authority_contract_is_valid_at(
            &helper_as_host,
            now_ns,
        ));

        let mut noncanonical_start = authority_at(now_ns);
        noncanonical_start.host_identity.start_seconds = "01".into();
        assert!(!registered_owned_paste_authority_contract_is_valid_at(
            &noncanonical_start,
            now_ns,
        ));
    }
}
