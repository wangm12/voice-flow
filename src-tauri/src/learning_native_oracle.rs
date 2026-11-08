#![cfg(all(feature = "research-harness", target_os = "macos"))]

//! Research-only bridge to the source-pinned owned FixtureHost AX reader.
//! This module reports a component sample; it does not prove product delivery.

use super::learning_phase_v3::LearningTargetBindingV1;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const SOURCE_MANIFEST_SHA256: &str =
    "4e143f250d877feebe36098020df5545cf200d6273e95a84eb4af16656d6668f";
const HELPER_SOURCE_SHA256: &str =
    "7fa2fa8d7ae40b40a2e23c361964fe1b05be1cef9e96c64660860509107b4265";
const BUILD_SCRIPT_SHA256: &str =
    "0a5f93d74f2e490c07bdd8e0151ae5d9d57655f86b1ab555070bcfac83b01461";
const RECIPE_SHA256: &str = "1d2f3f722c0232bc78e5896fe80c4a328c800e6813e34ead983167ab0a27ce15";
const INFO_PLIST_SHA256: &str = "8b7bee050e73472f890e0767f9e036c023d712fb9239ff7d41ca6b18613b74d5";
const EXPECTED_VALUE_SHA256: &str =
    "e7b17776dd843cfaa300566ee48aaa3a260364bb8edaebe7401ae749c1e9bfe1";
const EXPECTED_VALUE_BYTE_LENGTH: usize = 29;
const BUNDLE_IDENTIFIER: &str = "local.voiceflow.learning-oracle-fixture.v1";
const FIXTURE_ID: &str = "90000000-0000-4000-8000-000000000002";
const WINDOW_TITLE: &str = "Voice harness owned fixture 90000000-0000-4000-8000-000000000002";
const CONTROL_IDENTIFIER: &str = "vf-fixture-90000000-0000-4000-8000-000000000002-target";
const HELPER_STDOUT_MAX_BYTES: usize = 524_288;
const HELPER_STDERR_MAX_BYTES: usize = 8_192;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct PreparedManifest {
    artifact_type: String,
    schema_version: u8,
    cache_id: String,
    source_manifest_sha256: String,
    helper_source_sha256: String,
    build_script_sha256: String,
    helper_executable_sha256: String,
    host_executable_sha256: String,
    recipe_sha256: String,
    host_info_plist_sha256: String,
    expected_value_sha256: String,
    expected_value_byte_length: usize,
    host_bundle_identifier: String,
    helper_path: String,
    host_app_path: String,
    host_executable_path: String,
    recipe_path: String,
    source_manifest_path: String,
    info_plist_path: String,
}

#[derive(Clone)]
pub(crate) struct PreparedOwnedTargetOracleV1 {
    manifest_path: PathBuf,
    cache_root: PathBuf,
    manifest: PreparedManifest,
}

impl PreparedOwnedTargetOracleV1 {
    pub(crate) fn load(manifest_path: &Path) -> Result<Self, String> {
        let bytes = read_regular_file(manifest_path, 32_768)?;
        let manifest: PreparedManifest = serde_json::from_slice(&bytes)
            .map_err(|_| "native_oracle_prepared_manifest_invalid".to_owned())?;
        let cache_root = manifest_path
            .parent()
            .ok_or_else(|| "native_oracle_prepared_manifest_path_invalid".to_owned())?
            .to_path_buf();
        let prepared = Self {
            manifest_path: manifest_path.to_path_buf(),
            cache_root,
            manifest,
        };
        prepared.validate_files()?;
        Ok(prepared)
    }

    pub(crate) fn validate_binding(&self, binding: &LearningTargetBindingV1) -> Result<(), String> {
        let manifest = &self.manifest;
        if binding.fixture_id != FIXTURE_ID
            || binding.source_manifest_sha256 != manifest.source_manifest_sha256
            || binding.helper_executable_sha256 != manifest.helper_executable_sha256
            || binding.host_executable_sha256 != manifest.host_executable_sha256
            || binding.host_bundle_identifier != manifest.host_bundle_identifier
            || binding.host_info_plist_sha256 != manifest.host_info_plist_sha256
            || binding.recipe_sha256 != manifest.recipe_sha256
            || binding.window_title != WINDOW_TITLE
            || binding.control_identifier != CONTROL_IDENTIFIER
            || binding.expected_value_sha256 != manifest.expected_value_sha256
            || binding.expected_value_byte_length != manifest.expected_value_byte_length
        {
            return Err("native_oracle_target_binding_not_prepared".into());
        }
        Ok(())
    }

    fn validate_files(&self) -> Result<(), String> {
        let manifest = &self.manifest;
        if manifest.artifact_type != "voice_flow_owned_target_oracle_prepared"
            || manifest.schema_version != 1
            || manifest.source_manifest_sha256 != SOURCE_MANIFEST_SHA256
            || manifest.helper_source_sha256 != HELPER_SOURCE_SHA256
            || manifest.build_script_sha256 != BUILD_SCRIPT_SHA256
            || manifest.recipe_sha256 != RECIPE_SHA256
            || manifest.host_info_plist_sha256 != INFO_PLIST_SHA256
            || manifest.expected_value_sha256 != EXPECTED_VALUE_SHA256
            || manifest.expected_value_byte_length != EXPECTED_VALUE_BYTE_LENGTH
            || manifest.host_bundle_identifier != BUNDLE_IDENTIFIER
        {
            return Err("native_oracle_prepared_registry_mismatch".into());
        }
        for digest in [
            manifest.source_manifest_sha256.as_str(),
            manifest.helper_source_sha256.as_str(),
            manifest.build_script_sha256.as_str(),
            manifest.helper_executable_sha256.as_str(),
            manifest.host_executable_sha256.as_str(),
            manifest.recipe_sha256.as_str(),
            manifest.host_info_plist_sha256.as_str(),
        ] {
            if !is_sha256(digest) {
                return Err("native_oracle_prepared_hash_invalid".into());
            }
        }
        let cache_identity = format!(
            "{}\n{}\n{}\n{}\n{}",
            manifest.source_manifest_sha256,
            manifest.helper_source_sha256,
            manifest.build_script_sha256,
            manifest.recipe_sha256,
            manifest.host_info_plist_sha256
        );
        let expected_cache_id = format!("v1-{}", sha256(cache_identity.as_bytes()));
        if manifest.cache_id != expected_cache_id {
            return Err("native_oracle_prepared_cache_identity_invalid".into());
        }

        let target_root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("target/research/learning-native-oracle");
        let expected_root = target_root.join(&manifest.cache_id);
        if self.cache_root != expected_root
            || self.manifest_path != expected_root.join("prepared.json")
        {
            return Err("native_oracle_prepared_path_not_registered".into());
        }

        let expected_helper = expected_root.join("bin/OwnedTargetOracle");
        let expected_app = expected_root.join("FixtureHost.app");
        let expected_host = expected_app.join("Contents/MacOS/FixtureHost");
        let expected_recipe = expected_root.join("Recipe.json");
        let expected_manifest = expected_root.join("source-manifest.txt");
        let expected_info = expected_app.join("Contents/Info.plist");
        if Path::new(&manifest.helper_path) != expected_helper
            || Path::new(&manifest.host_app_path) != expected_app
            || Path::new(&manifest.host_executable_path) != expected_host
            || Path::new(&manifest.recipe_path) != expected_recipe
            || Path::new(&manifest.source_manifest_path) != expected_manifest
            || Path::new(&manifest.info_plist_path) != expected_info
        {
            return Err("native_oracle_prepared_path_invalid".into());
        }
        validate_prepared_directories(&expected_root)?;

        verify_file_hash(
            &expected_helper,
            67_108_864,
            &manifest.helper_executable_sha256,
        )?;
        verify_file_hash(&expected_host, 67_108_864, &manifest.host_executable_sha256)?;
        verify_file_hash(&expected_recipe, 1_048_576, &manifest.recipe_sha256)?;
        verify_file_hash(&expected_manifest, 16_384, &manifest.source_manifest_sha256)?;
        verify_file_hash(&expected_info, 16_384, &manifest.host_info_plist_sha256)?;
        for executable in [&expected_helper, &expected_host] {
            let metadata = fs::symlink_metadata(executable)
                .map_err(|_| "native_oracle_prepared_executable_unavailable".to_owned())?;
            if metadata.permissions().mode() & 0o111 == 0 {
                return Err("native_oracle_prepared_executable_not_executable".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub(crate) struct NativeTargetOracleOutcomeV1 {
    pub artifact_type: String,
    pub schema_version: u8,
    pub plan_id: String,
    pub phase_id: String,
    pub operation_id: String,
    pub session_id: String,
    pub helper_pid: i32,
    pub overall_state: String,
    pub source_manifest_sha256: String,
    pub helper_executable_sha256: String,
    pub host_executable_sha256: String,
    pub host_bundle_identifier: String,
    pub recipe_sha256: String,
    pub setup: NativeTargetSetupV1,
    pub actual_sample: NativeTargetActualSampleV1,
    pub cleanup: NativeTargetCleanupV1,
    pub expected_comparison: String,
    pub target_terminal: String,
    pub unknown_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub(crate) struct NativeTargetSetupV1 {
    pub state: String,
    pub receipt_sha256: Option<String>,
    pub receipt: Option<HostReceiptV1>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub(crate) struct NativeTargetActualSampleV1 {
    pub state: String,
    pub source: String,
    pub sampled_uptime_ns: Option<String>,
    pub value: Option<String>,
    pub value_sha256: Option<String>,
    pub value_byte_length: Option<usize>,
    pub ax_readback: Option<AxReadbackV1>,
    pub process_identity_before: Option<ProcessIdentityObservationV1>,
    pub process_identity_after: Option<ProcessIdentityObservationV1>,
    pub binding_mode: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub(crate) struct NativeTargetCleanupV1 {
    pub state: String,
    pub child_pid: Option<i32>,
    pub termination_status: Option<i32>,
    pub exit_confirmed: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub(crate) struct HostReceiptV1 {
    pub artifact_type: String,
    pub schema_version: u8,
    pub run_id: String,
    pub case_id: String,
    pub fixture_id: String,
    pub process_identity: ProcessIdentityObservationV1,
    pub recipe_sha256: String,
    pub title: String,
    pub control: String,
    pub other_control: String,
    pub ready_uptime_ns: String,
    pub setup_observation: Option<HostSetupObservationV1>,
    pub native_application: Option<NativeApplicationObservationV1>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub(crate) struct HostSetupObservationV1 {
    pub source: String,
    pub local_state: String,
    pub independent_ax_readiness: String,
    pub window_visible: bool,
    pub target_attached: bool,
    pub other_attached: bool,
    pub declared_text_and_range: bool,
    pub target_first_responder: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub(crate) struct NativeApplicationObservationV1 {
    pub source: String,
    pub state: String,
    pub bundle_path: Option<String>,
    pub bundle_identifier: Option<String>,
    pub executable_path: Option<String>,
    pub info_plist_sha256: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub(crate) struct ProcessIdentityObservationV1 {
    pub requested_pid: i32,
    pub kernel_state: String,
    pub kernel: Option<KernelProcessIdentityV1>,
    pub kernel_error: Option<String>,
    pub cocoa_launch_date_source: String,
    pub cocoa_launch_date_state: String,
    pub cocoa_launch_date: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub(crate) struct KernelProcessIdentityV1 {
    pub source: String,
    pub pid: i32,
    pub parent_pid: i32,
    pub user_id: u32,
    pub start_seconds: String,
    pub start_microseconds: String,
    pub executable_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub(crate) struct AxReadbackV1 {
    pub source: String,
    pub applicability: String,
    pub reason: String,
    pub startup_retryable: bool,
    pub process_identity_before: String,
    pub process_identity_after: String,
    pub process_identity_error: Option<String>,
    pub binding: AxBindingFactsV1,
    pub attributes: Vec<AxAttributeFactV1>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub(crate) struct AxBindingFactsV1 {
    pub coverage: String,
    pub window_count: Option<usize>,
    pub window_titles_examined: Option<usize>,
    pub matching_windows_observed: Option<usize>,
    pub elements_examined: Option<usize>,
    pub pending_elements: Option<usize>,
    pub matching_controls_observed: Option<usize>,
    pub matching_other_controls_observed: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub(crate) struct AxAttributeFactV1 {
    pub element_path: String,
    pub attribute: String,
    pub operation: String,
    pub error_code: Option<i32>,
    pub error_name: Option<String>,
    pub state: String,
    pub returned_count: Option<usize>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeTargetExpectedComparisonV1 {
    Matched,
    Mismatched,
    Unknown,
}

pub(crate) fn compare_registered_expected_value(
    binding: &LearningTargetBindingV1,
    outcome: &NativeTargetOracleOutcomeV1,
) -> NativeTargetExpectedComparisonV1 {
    if outcome.overall_state != "observed_sample"
        || outcome.actual_sample.state != "observed"
        || !outcome.cleanup.exit_confirmed
    {
        return NativeTargetExpectedComparisonV1::Unknown;
    }
    let Some(value) = outcome.actual_sample.value.as_ref() else {
        return NativeTargetExpectedComparisonV1::Unknown;
    };
    let actual_sha256 = sha256(value.as_bytes());
    if outcome.actual_sample.value_sha256.as_deref() != Some(actual_sha256.as_str())
        || outcome.actual_sample.value_byte_length != Some(value.len())
    {
        return NativeTargetExpectedComparisonV1::Unknown;
    }
    if actual_sha256 == binding.expected_value_sha256
        && value.len() == binding.expected_value_byte_length
    {
        NativeTargetExpectedComparisonV1::Matched
    } else {
        NativeTargetExpectedComparisonV1::Mismatched
    }
}

struct CancelOwnerOnDrop(Option<CancellationToken>);

impl Drop for CancelOwnerOnDrop {
    fn drop(&mut self) {
        if let Some(token) = self.0.take() {
            token.cancel();
        }
    }
}

pub(crate) async fn run_native_oracle_phase(
    prepared: &PreparedOwnedTargetOracleV1,
    plan_id: &str,
    phase_id: &str,
    operation_id: &str,
    binding: &LearningTargetBindingV1,
    expires_at: Instant,
    cancellation: &CancellationToken,
) -> Result<NativeTargetOracleOutcomeV1, String> {
    validate_uuid(plan_id)?;
    validate_uuid(phase_id)?;
    validate_uuid(operation_id)?;
    prepared.validate_binding(binding)?;
    if expires_at <= Instant::now() {
        return Err("native_oracle_phase_expired_before_launch".into());
    }
    if cancellation.is_cancelled() {
        return Err("native_oracle_phase_cancelled_before_launch".into());
    }

    let owner_token = CancellationToken::new();
    let mut cancel_on_drop = CancelOwnerOnDrop(Some(owner_token.clone()));
    let monitor_token = owner_token.clone();
    let external_cancel = cancellation.clone();
    let deadline = tokio::time::Instant::from_std(expires_at);
    let monitor = tokio::spawn(async move {
        tokio::select! {
            _ = external_cancel.cancelled() => monitor_token.cancel(),
            _ = tokio::time::sleep_until(deadline) => monitor_token.cancel(),
            _ = monitor_token.cancelled() => {},
        }
    });

    let worker_prepared = prepared.clone();
    let worker_plan_id = plan_id.to_owned();
    let worker_phase_id = phase_id.to_owned();
    let worker_operation_id = operation_id.to_owned();
    let binding = binding.clone();
    let worker_token = owner_token.clone();
    let worker = tokio::task::spawn_blocking(move || {
        run_native_oracle_blocking(
            &worker_prepared,
            &worker_plan_id,
            &worker_phase_id,
            &worker_operation_id,
            &binding,
            expires_at,
            &worker_token,
        )
    });
    let result = worker
        .await
        .map_err(|_| "native_oracle_process_owner_failed".to_owned())?;
    monitor.abort();
    cancel_on_drop.0 = None;

    if cancellation.is_cancelled() || Instant::now() >= expires_at {
        return result.map(|outcome| discard_late_sample(outcome, cancellation.is_cancelled()));
    }
    let outcome = result?;
    validate_outcome(&outcome, prepared, plan_id, phase_id, operation_id, None)?;
    if cancellation.is_cancelled() || Instant::now() >= expires_at {
        return Ok(discard_late_sample(outcome, cancellation.is_cancelled()));
    }
    Ok(outcome)
}

fn run_native_oracle_blocking(
    prepared: &PreparedOwnedTargetOracleV1,
    plan_id: &str,
    phase_id: &str,
    operation_id: &str,
    binding: &LearningTargetBindingV1,
    expires_at: Instant,
    cancellation: &CancellationToken,
) -> Result<NativeTargetOracleOutcomeV1, String> {
    if cancellation.is_cancelled() || Instant::now() >= expires_at {
        return Err("native_oracle_phase_cancelled_before_helper_spawn".into());
    }
    prepared.validate_files()?;
    prepared.validate_binding(binding)?;
    let remaining_ms = expires_at
        .saturating_duration_since(Instant::now())
        .as_millis();
    if remaining_ms == 0 || remaining_ms > 1_200_000 {
        return Err("native_oracle_phase_deadline_invalid".into());
    }
    let remaining_ms = u64::try_from(remaining_ms)
        .map_err(|_| "native_oracle_phase_deadline_invalid".to_owned())?;
    let session_id = format!("{plan_id}-{phase_id}-{operation_id}");
    let sessions_root = prepared.cache_root.join("sessions");
    ensure_private_directory(&sessions_root)?;
    let session_path = sessions_root.join(&session_id);
    fs::create_dir(&session_path).map_err(|_| "native_oracle_session_already_used".to_owned())?;
    fs::set_permissions(&session_path, fs::Permissions::from_mode(0o700))
        .map_err(|_| "native_oracle_session_permissions_failed".to_owned())?;

    let stdout_path = session_path.join("helper.stdout.json");
    let stderr_path = session_path.join("helper.stderr.log");
    let stdout = create_new_file(&stdout_path)?;
    let stderr = create_new_file(&stderr_path)?;
    let mut command = Command::new(&prepared.manifest.helper_path);
    command
        .arg("--plan-id")
        .arg(plan_id)
        .arg("--phase-id")
        .arg(phase_id)
        .arg("--operation-id")
        .arg(operation_id)
        .arg("--session-id")
        .arg(&session_id)
        .arg("--remaining-ms")
        .arg(remaining_ms.to_string())
        .current_dir(&prepared.cache_root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let mut child = command
        .spawn()
        .map_err(|_| "native_oracle_helper_spawn_failed".to_owned())?;
    let helper_pid =
        i32::try_from(child.id()).map_err(|_| "native_oracle_helper_pid_invalid".to_owned())?;

    let cancel_path = session_path.join("cancel.json");
    let mut cancellation_requested = false;
    let status = loop {
        if (cancellation.is_cancelled() || Instant::now() >= expires_at) && !cancellation_requested
        {
            // Continue owning the helper if the marker cannot be created; its
            // own phase deadline still closes the Host through its retained
            // process handle. Returning early would detach that child chain.
            let _ = write_cancel_marker(&cancel_path);
            cancellation_requested = true;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(_) => match child.wait() {
                Ok(status) => break status,
                Err(_) => {
                    // Keep this blocking owner and Child handle alive. Without
                    // an observed exit status, the helper may still own Host.
                }
            },
        }
        thread::sleep(Duration::from_millis(10));
    };

    let stdout = read_regular_file(&stdout_path, HELPER_STDOUT_MAX_BYTES)?;
    let stderr = read_regular_file(&stderr_path, HELPER_STDERR_MAX_BYTES)?;
    if !status.success() {
        let detail = String::from_utf8_lossy(&stderr).trim().to_owned();
        return Err(if detail.is_empty() {
            format!(
                "native_oracle_helper_exit_status_{}",
                status.code().unwrap_or(-1)
            )
        } else {
            format!(
                "native_oracle_helper_exit_status_{}: {}",
                status.code().unwrap_or(-1),
                detail
            )
        });
    }
    let outcome: NativeTargetOracleOutcomeV1 = serde_json::from_slice(&stdout)
        .map_err(|_| "native_oracle_helper_receipt_invalid".to_owned())?;
    validate_setup_receipt_file(&outcome, &session_path)?;
    validate_outcome(
        &outcome,
        prepared,
        plan_id,
        phase_id,
        operation_id,
        Some(helper_pid),
    )?;

    if cancellation_requested || cancellation.is_cancelled() || Instant::now() >= expires_at {
        if !outcome.cleanup.exit_confirmed {
            return Err("native_oracle_cancelled_host_exit_unconfirmed".into());
        }
        return Ok(discard_late_sample(outcome, cancellation.is_cancelled()));
    }
    Ok(outcome)
}

fn validate_outcome(
    outcome: &NativeTargetOracleOutcomeV1,
    prepared: &PreparedOwnedTargetOracleV1,
    plan_id: &str,
    phase_id: &str,
    operation_id: &str,
    expected_helper_pid: Option<i32>,
) -> Result<(), String> {
    let manifest = &prepared.manifest;
    let expected_session = format!("{plan_id}-{phase_id}-{operation_id}");
    if outcome.artifact_type != "voice_flow_owned_native_target_oracle"
        || outcome.schema_version != 1
        || outcome.plan_id != plan_id
        || outcome.phase_id != phase_id
        || outcome.operation_id != operation_id
        || outcome.session_id != expected_session
        || outcome.helper_pid <= 0
        || expected_helper_pid.is_some_and(|expected| outcome.helper_pid != expected)
        || outcome.source_manifest_sha256 != manifest.source_manifest_sha256
        || outcome.helper_executable_sha256 != manifest.helper_executable_sha256
        || outcome.host_executable_sha256 != manifest.host_executable_sha256
        || outcome.host_bundle_identifier != manifest.host_bundle_identifier
        || outcome.recipe_sha256 != manifest.recipe_sha256
        || outcome.expected_comparison != "receiver_independent_not_performed_by_helper"
        || outcome.target_terminal != "not_probed_component_sample_only"
    {
        return Err("native_oracle_receipt_identity_mismatch".into());
    }
    if outcome.overall_state == "observed_sample" {
        if outcome.cleanup.state != "owned_child_exit_confirmed"
            || !outcome.cleanup.exit_confirmed
            || outcome.cleanup.termination_status != Some(0)
        {
            return Err("native_oracle_host_terminal_unconfirmed".into());
        }
        if outcome.setup.state != "host_setup_received_only"
            || outcome
                .setup
                .receipt_sha256
                .as_deref()
                .is_none_or(|digest| !is_sha256(digest))
        {
            return Err("native_oracle_setup_receipt_hash_missing".into());
        }
        validate_setup(outcome.setup.receipt.as_ref(), prepared)?;
        validate_actual_sample(
            &outcome.actual_sample,
            &manifest.host_executable_path,
            outcome.helper_pid,
        )?;
        let setup_identity = outcome
            .setup
            .receipt
            .as_ref()
            .and_then(|receipt| receipt.process_identity.kernel.as_ref())
            .ok_or_else(|| "native_oracle_setup_process_identity_missing".to_owned())?;
        let sample_identity = outcome
            .actual_sample
            .process_identity_before
            .as_ref()
            .and_then(|observation| observation.kernel.as_ref())
            .ok_or_else(|| "native_oracle_sample_process_identity_missing".to_owned())?;
        if setup_identity != sample_identity
            || outcome.cleanup.child_pid != Some(setup_identity.pid)
        {
            return Err("native_oracle_cleanup_child_identity_mismatch".into());
        }
    } else if outcome.overall_state == "unknown" {
        if outcome.cleanup.state == "owned_child_exit_confirmed" {
            if !outcome.cleanup.exit_confirmed
                || outcome.cleanup.child_pid.is_none()
                || outcome.cleanup.termination_status.is_none()
            {
                return Err("native_oracle_host_terminal_unconfirmed".into());
            }
        } else if outcome.cleanup.state != "not_started"
            || !outcome.cleanup.exit_confirmed
            || outcome.cleanup.child_pid.is_some()
            || outcome.cleanup.termination_status.is_some()
        {
            return Err("native_oracle_host_terminal_unconfirmed".into());
        }
    } else {
        return Err("native_oracle_receipt_state_invalid".into());
    }
    Ok(())
}

fn validate_setup(
    receipt: Option<&HostReceiptV1>,
    prepared: &PreparedOwnedTargetOracleV1,
) -> Result<(), String> {
    let receipt = receipt.ok_or_else(|| "native_oracle_setup_receipt_missing".to_owned())?;
    let native = receipt
        .native_application
        .as_ref()
        .ok_or_else(|| "native_oracle_host_bundle_identity_missing".to_owned())?;
    let setup = receipt
        .setup_observation
        .as_ref()
        .ok_or_else(|| "native_oracle_host_setup_missing".to_owned())?;
    if receipt.artifact_type != "voice_harness_host"
        || receipt.schema_version != 2
        || receipt.run_id != "90000000-0000-4000-8000-000000000001"
        || receipt.case_id != "owned-native-oracle-v1"
        || receipt.fixture_id != FIXTURE_ID
        || receipt.recipe_sha256 != RECIPE_SHA256
        || receipt.title != WINDOW_TITLE
        || receipt.control != CONTROL_IDENTIFIER
        || receipt.other_control != "vf-fixture-90000000-0000-4000-8000-000000000002-other"
        || setup.source != "FixtureHost_main_run_loop_after_applicationDidFinishLaunching"
        || setup.local_state != "declared_setup_observed"
        || setup.independent_ax_readiness != "not_observed"
        || !setup.window_visible
        || !setup.target_attached
        || !setup.other_attached
        || !setup.declared_text_and_range
        || !setup.target_first_responder
        || native.source != "FixtureHost_Bundle.main"
        || native.state != "observed_app_bundle"
        || native.bundle_path.as_deref() != Some(prepared.manifest.host_app_path.as_str())
        || native.bundle_identifier.as_deref() != Some(BUNDLE_IDENTIFIER)
        || native.executable_path.as_deref()
            != Some(prepared.manifest.host_executable_path.as_str())
        || native.info_plist_sha256.as_deref() != Some(INFO_PLIST_SHA256)
        || native.reason.is_some()
        || parse_canonical_decimal(&receipt.ready_uptime_ns)
            .filter(|value| *value > 0)
            .is_none()
    {
        return Err("native_oracle_setup_identity_mismatch".into());
    }
    validate_process_identity(
        &receipt.process_identity,
        &prepared.manifest.host_executable_path,
    )?;
    Ok(())
}

fn validate_setup_receipt_file(
    outcome: &NativeTargetOracleOutcomeV1,
    session_path: &Path,
) -> Result<(), String> {
    match (
        outcome.setup.receipt_sha256.as_deref(),
        outcome.setup.receipt.as_ref(),
    ) {
        (Some(expected), Some(_)) if is_sha256(expected) => {
            let receipt = read_regular_file(&session_path.join("ready.json"), 32_768)?;
            if sha256(&receipt) != expected {
                return Err("native_oracle_setup_receipt_file_hash_mismatch".into());
            }
            Ok(())
        }
        (None, None) if outcome.setup.state == "unknown" => Ok(()),
        _ => Err("native_oracle_setup_receipt_projection_invalid".into()),
    }
}

fn validate_actual_sample(
    sample: &NativeTargetActualSampleV1,
    expected_host_executable: &str,
    helper_pid: i32,
) -> Result<(), String> {
    let evidence = sample
        .ax_readback
        .as_ref()
        .ok_or_else(|| "native_oracle_ax_evidence_missing".to_owned())?;
    let before = sample
        .process_identity_before
        .as_ref()
        .ok_or_else(|| "native_oracle_process_identity_before_missing".to_owned())?;
    let after = sample
        .process_identity_after
        .as_ref()
        .ok_or_else(|| "native_oracle_process_identity_after_missing".to_owned())?;
    let value = sample
        .value
        .as_ref()
        .ok_or_else(|| "native_oracle_actual_value_missing".to_owned())?;
    let actual_sha = sha256(value.as_bytes());
    let sampled_at = sample
        .sampled_uptime_ns
        .as_deref()
        .and_then(parse_canonical_decimal);
    let target_value_reads = evidence
        .attributes
        .iter()
        .filter(|attribute| attribute.attribute == "AXValue")
        .collect::<Vec<_>>();
    if sample.state != "observed"
        || sample.source != "AXTargetReader_owned_declared_target_v1"
        || evidence.source != "AXTargetReader_owned_declared_target_v1"
        || evidence.applicability != "matched"
        || evidence.reason != "observed"
        || evidence.process_identity_before != "matched_live_owned_kernel"
        || evidence.process_identity_after != "matched_live_owned_kernel"
        || evidence.process_identity_error.is_some()
        || evidence.binding.coverage != "complete_exposed_hierarchy"
        || evidence.binding.window_count != Some(1)
        || evidence.binding.window_titles_examined != Some(1)
        || evidence.binding.matching_windows_observed != Some(1)
        || evidence.binding.elements_examined.unwrap_or(0) == 0
        || evidence.binding.pending_elements != Some(0)
        || evidence.binding.matching_controls_observed != Some(1)
        || evidence.binding.matching_other_controls_observed != Some(1)
        || target_value_reads.len() != 2
        || !target_value_reads.iter().any(|attribute| {
            attribute.element_path == "declared_target"
                && attribute.operation == "AXUIElementCopyAttributeValue"
                && attribute.error_code == Some(0)
                && attribute.state == "observed"
        })
        || !target_value_reads.iter().any(|attribute| {
            attribute.element_path == "declared_other"
                && attribute.operation == "AXUIElementCopyAttributeValue"
                && attribute.error_code == Some(0)
                && attribute.state == "observed"
        })
        || sample.binding_mode
            != "same_owned_control_identifier_for_reader_pair; target_AXValue_queried_twice; second_control_value_not_exported"
        || sample.value_sha256.as_deref() != Some(actual_sha.as_str())
        || sample.value_byte_length != Some(value.len())
        || sampled_at.filter(|value| *value > 0).is_none()
        || value.len() > 65_536
    {
        return Err("native_oracle_actual_sample_invalid".into());
    }
    validate_process_identity(before, expected_host_executable)?;
    validate_process_identity(after, expected_host_executable)?;
    if before.kernel != after.kernel
        || before
            .kernel
            .as_ref()
            .is_none_or(|kernel| kernel.parent_pid != helper_pid)
    {
        return Err("native_oracle_process_identity_changed_during_read".into());
    }
    Ok(())
}

fn validate_process_identity(
    observation: &ProcessIdentityObservationV1,
    expected_executable: &str,
) -> Result<(), String> {
    let kernel = observation
        .kernel
        .as_ref()
        .ok_or_else(|| "native_oracle_kernel_identity_missing".to_owned())?;
    if observation.kernel_state != "observed"
        || observation.kernel_error.is_some()
        || observation.requested_pid != kernel.pid
        || kernel.source != "darwin_proc_pidinfo_bsdinfo_proc_pidpath"
        || kernel.pid <= 0
        || kernel.parent_pid <= 0
        || kernel.executable_path != expected_executable
        || parse_canonical_decimal(&kernel.start_seconds)
            .filter(|value| *value > 0)
            .is_none()
        || parse_canonical_decimal(&kernel.start_microseconds)
            .filter(|value| *value < 1_000_000)
            .is_none()
    {
        return Err("native_oracle_process_identity_invalid".into());
    }
    Ok(())
}

fn discard_late_sample(
    mut outcome: NativeTargetOracleOutcomeV1,
    cancelled: bool,
) -> NativeTargetOracleOutcomeV1 {
    outcome.overall_state = "unknown".into();
    outcome.actual_sample.state = "unknown".into();
    outcome.actual_sample.sampled_uptime_ns = None;
    outcome.actual_sample.value = None;
    outcome.actual_sample.value_sha256 = None;
    outcome.actual_sample.value_byte_length = None;
    outcome.actual_sample.ax_readback = None;
    outcome.actual_sample.process_identity_before = None;
    outcome.actual_sample.process_identity_after = None;
    outcome.unknown_reason = Some(if cancelled {
        "phase_cancelled_after_owned_host_closed_result_discarded".into()
    } else {
        "phase_expired_after_owned_host_closed_result_discarded".into()
    });
    outcome
}

fn verify_file_hash(path: &Path, limit: usize, expected: &str) -> Result<(), String> {
    let bytes = read_regular_file(path, limit)?;
    if sha256(&bytes) != expected {
        return Err("native_oracle_prepared_file_hash_mismatch".into());
    }
    Ok(())
}

fn validate_prepared_directories(expected_root: &Path) -> Result<(), String> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directories = [
        manifest_dir.to_path_buf(),
        manifest_dir.join("target"),
        manifest_dir.join("target/research"),
        manifest_dir.join("target/research/learning-native-oracle"),
        expected_root.to_path_buf(),
    ];
    for directory in directories {
        let metadata = fs::symlink_metadata(&directory)
            .map_err(|_| "native_oracle_prepared_directory_unavailable".to_owned())?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err("native_oracle_prepared_directory_invalid".into());
        }
    }
    Ok(())
}

fn read_regular_file(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "native_oracle_file_unavailable".to_owned())?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > limit as u64
    {
        return Err("native_oracle_file_invalid_or_over_budget".into());
    }
    let file = File::open(path).map_err(|_| "native_oracle_file_open_failed".to_owned())?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "native_oracle_file_read_failed".to_owned())?;
    if bytes.len() > limit {
        return Err("native_oracle_file_over_budget".into());
    }
    Ok(bytes)
}

fn create_new_file(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| "native_oracle_session_output_already_exists".to_owned())
}

fn write_cancel_marker(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    let mut file = create_new_file(path)?;
    file.write_all(b"{\"cancelled\":true}\n")
        .and_then(|_| file.sync_all())
        .map_err(|_| "native_oracle_cancel_marker_write_failed".to_owned())
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    if !path.exists() {
        fs::create_dir_all(path)
            .map_err(|_| "native_oracle_cache_sessions_unavailable".to_owned())?;
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| "native_oracle_cache_sessions_unavailable".to_owned())?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err("native_oracle_cache_sessions_invalid".into());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| "native_oracle_cache_sessions_permissions_failed".to_owned())
}

fn validate_uuid(value: &str) -> Result<(), String> {
    crate::onboarding::validate_id(value).map_err(|_| "native_oracle_uuid_noncanonical".to_owned())
}

fn parse_canonical_decimal(value: &str) -> Option<u64> {
    value
        .parse::<u64>()
        .ok()
        .filter(|parsed| parsed.to_string() == value)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
