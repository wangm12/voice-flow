#![cfg(all(feature = "research-harness", target_os = "macos"))]

//! Research-only owner for one registered same-Host paste/edit/readback slice.
//! It owns the helper process until its Host cleanup is confirmed and never
//! opens learning SQL or Settings.

use super::learning_native_oracle::{
    AxReadbackV1, HostReceiptV1, KernelProcessIdentityV1, ProcessIdentityObservationV1,
};
use super::learning_phase_v3::OwnedPasteSessionBindingV1;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

const SOURCE_MANIFEST_SHA256: &str =
    "4e143f250d877feebe36098020df5545cf200d6273e95a84eb4af16656d6668f";
const HELPER_SOURCE_SHA256: &str =
    "6a378415e579adffa017d012ef1f6a34da3d443896de2369f52aee175c249bd9";
const BUILD_SCRIPT_SHA256: &str =
    "b31607f368b59fc4be3d212b296babef03cdb3f513f5fb35c114d23d4639671e";
const RECIPE_SHA256: &str = "498a5af64c3c7626239484a3ae899da7c6f4f58a7198e7c72ac2716a8009f9cf";
const INFO_PLIST_SHA256: &str = "8b7bee050e73472f890e0767f9e036c023d712fb9239ff7d41ca6b18613b74d5";
const INSERTED_TEXT_SHA256: &str =
    "c72a301aa81e686da1dae98534e6b3333222cd305e3a1fef714491697afcce95";
const INSERTED_TEXT_BYTE_LENGTH: usize = 8;
const POST_INSERT_SHA256: &str = "56ceef9d7bb12e9bc5a5bbe8b661a60432ad5ac741cd84c64e1bb021e1bbbcc9";
const POST_INSERT_BYTE_LENGTH: usize = 14;
const SETTLED_SHA256: &str = "901c32b29acb890cc90420da89d82ea6361f718dc45fc0b8afb1000169deb304";
const SETTLED_BYTE_LENGTH: usize = 15;
const HOST_BUNDLE_ID: &str = "local.voiceflow.learning-oracle-fixture.v1";
const FIXTURE_ID: &str = "90000000-0000-4000-8000-000000000013";
const RUN_ID: &str = "90000000-0000-4000-8000-000000000011";
const CASE_ID: &str = "90000000-0000-4000-8000-000000000012";
const WINDOW_TITLE: &str = "Voice harness owned fixture 90000000-0000-4000-8000-000000000013";
const CONTROL_ID: &str = "vf-fixture-90000000-0000-4000-8000-000000000013-target";
const OTHER_CONTROL_ID: &str = "vf-fixture-90000000-0000-4000-8000-000000000013-other";
const EVENT_MAX_BYTES: usize = 524_288;
const FINAL_AX_SOURCE: &str = "AXTargetReader_owned_declared_target_v1";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct PreparedManifestV1 {
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
    inserted_text_sha256: String,
    inserted_text_byte_length: usize,
    expected_post_insert_sha256: String,
    expected_post_insert_byte_length: usize,
    expected_settled_sha256: String,
    expected_settled_byte_length: usize,
    host_bundle_identifier: String,
    helper_path: String,
    host_app_path: String,
    host_executable_path: String,
    recipe_path: String,
    source_manifest_path: String,
    info_plist_path: String,
}

#[derive(Clone)]
pub(crate) struct PreparedOwnedPasteSessionV1 {
    manifest_path: PathBuf,
    cache_root: PathBuf,
    manifest: PreparedManifestV1,
}

impl PreparedOwnedPasteSessionV1 {
    pub(crate) fn load(manifest_path: &Path) -> Result<Self, String> {
        let bytes = read_regular_file(manifest_path, 32_768)?;
        let manifest: PreparedManifestV1 = serde_json::from_slice(&bytes)
            .map_err(|_| "owned_paste_session_manifest_invalid".to_owned())?;
        let cache_root = manifest_path
            .parent()
            .ok_or_else(|| "owned_paste_session_manifest_path_invalid".to_owned())?
            .to_path_buf();
        let prepared = Self {
            manifest_path: manifest_path.to_path_buf(),
            cache_root,
            manifest,
        };
        prepared.validate_files()?;
        Ok(prepared)
    }

    pub(crate) fn validate_binding(
        &self,
        binding: &OwnedPasteSessionBindingV1,
    ) -> Result<(), String> {
        let manifest = &self.manifest;
        if binding.fixture_id != FIXTURE_ID
            || binding.source_manifest_sha256 != manifest.source_manifest_sha256
            || binding.helper_executable_sha256 != manifest.helper_executable_sha256
            || binding.host_executable_sha256 != manifest.host_executable_sha256
            || binding.host_bundle_identifier != manifest.host_bundle_identifier
            || binding.host_info_plist_sha256 != manifest.host_info_plist_sha256
            || binding.recipe_sha256 != manifest.recipe_sha256
            || binding.window_title != WINDOW_TITLE
            || binding.control_identifier != CONTROL_ID
            || binding.inserted_text_sha256 != manifest.inserted_text_sha256
            || binding.inserted_text_byte_length != manifest.inserted_text_byte_length
            || binding.expected_post_insert_sha256 != manifest.expected_post_insert_sha256
            || binding.expected_post_insert_byte_length != manifest.expected_post_insert_byte_length
            || binding.expected_settled_sha256 != manifest.expected_settled_sha256
            || binding.expected_settled_byte_length != manifest.expected_settled_byte_length
        {
            return Err("owned_paste_session_binding_not_prepared".into());
        }
        Ok(())
    }

    pub(crate) fn artifact_identity_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "artifact_type": self.manifest.artifact_type,
            "schema_version": self.manifest.schema_version,
            "cache_id": self.manifest.cache_id,
            "source_manifest_sha256": self.manifest.source_manifest_sha256,
            "helper_source_sha256": self.manifest.helper_source_sha256,
            "build_script_sha256": self.manifest.build_script_sha256,
            "helper_executable_sha256": self.manifest.helper_executable_sha256,
            "host_executable_sha256": self.manifest.host_executable_sha256,
            "recipe_sha256": self.manifest.recipe_sha256,
            "host_info_plist_sha256": self.manifest.host_info_plist_sha256,
            "host_bundle_identifier": self.manifest.host_bundle_identifier,
            "inserted_text_sha256": self.manifest.inserted_text_sha256,
            "inserted_text_byte_length": self.manifest.inserted_text_byte_length,
            "expected_post_insert_sha256": self.manifest.expected_post_insert_sha256,
            "expected_post_insert_byte_length": self.manifest.expected_post_insert_byte_length,
            "expected_settled_sha256": self.manifest.expected_settled_sha256,
            "expected_settled_byte_length": self.manifest.expected_settled_byte_length,
        })
    }

    fn validate_files(&self) -> Result<(), String> {
        let manifest = &self.manifest;
        if manifest.artifact_type != "voice_flow_owned_paste_session_prepared"
            || manifest.schema_version != 1
            || manifest.source_manifest_sha256 != SOURCE_MANIFEST_SHA256
            || manifest.helper_source_sha256 != HELPER_SOURCE_SHA256
            || manifest.build_script_sha256 != BUILD_SCRIPT_SHA256
            || manifest.recipe_sha256 != RECIPE_SHA256
            || manifest.host_info_plist_sha256 != INFO_PLIST_SHA256
            || manifest.inserted_text_sha256 != INSERTED_TEXT_SHA256
            || manifest.inserted_text_byte_length != INSERTED_TEXT_BYTE_LENGTH
            || manifest.expected_post_insert_sha256 != POST_INSERT_SHA256
            || manifest.expected_post_insert_byte_length != POST_INSERT_BYTE_LENGTH
            || manifest.expected_settled_sha256 != SETTLED_SHA256
            || manifest.expected_settled_byte_length != SETTLED_BYTE_LENGTH
            || manifest.host_bundle_identifier != HOST_BUNDLE_ID
        {
            return Err("owned_paste_session_registry_mismatch".into());
        }
        for digest in [
            manifest.source_manifest_sha256.as_str(),
            manifest.helper_source_sha256.as_str(),
            manifest.build_script_sha256.as_str(),
            manifest.helper_executable_sha256.as_str(),
            manifest.host_executable_sha256.as_str(),
            manifest.recipe_sha256.as_str(),
            manifest.host_info_plist_sha256.as_str(),
            manifest.inserted_text_sha256.as_str(),
            manifest.expected_post_insert_sha256.as_str(),
            manifest.expected_settled_sha256.as_str(),
        ] {
            if !is_sha256(digest) {
                return Err("owned_paste_session_hash_invalid".into());
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
            return Err("owned_paste_session_cache_identity_invalid".into());
        }

        let target_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/research/learning-native-paste-session");
        let expected_root = target_root.join(&manifest.cache_id);
        if self.cache_root != expected_root
            || self.manifest_path != expected_root.join("prepared.json")
        {
            return Err("owned_paste_session_path_not_registered".into());
        }
        let research_root = target_root
            .parent()
            .ok_or("owned_paste_session_path_invalid")?
            .to_path_buf();
        let directories = [
            research_root,
            target_root.clone(),
            expected_root.clone(),
            expected_root.join("bin"),
            expected_root.join("FixtureHost.app"),
            expected_root.join("FixtureHost.app/Contents"),
            expected_root.join("FixtureHost.app/Contents/MacOS"),
        ];
        for directory in &directories {
            require_real_directory(directory)?;
        }

        let expected_helper = expected_root.join("bin/OwnedPasteSession");
        let expected_app = expected_root.join("FixtureHost.app");
        let expected_host = expected_app.join("Contents/MacOS/FixtureHost");
        let expected_recipe = expected_root.join("Recipe.json");
        let expected_source_manifest = expected_root.join("source-manifest.txt");
        let expected_info = expected_app.join("Contents/Info.plist");
        if Path::new(&manifest.helper_path) != expected_helper
            || Path::new(&manifest.host_app_path) != expected_app
            || Path::new(&manifest.host_executable_path) != expected_host
            || Path::new(&manifest.recipe_path) != expected_recipe
            || Path::new(&manifest.source_manifest_path) != expected_source_manifest
            || Path::new(&manifest.info_plist_path) != expected_info
        {
            return Err("owned_paste_session_prepared_path_invalid".into());
        }
        verify_file_hash(&expected_helper, &manifest.helper_executable_sha256)?;
        verify_file_hash(&expected_host, &manifest.host_executable_sha256)?;
        verify_file_hash(&expected_recipe, &manifest.recipe_sha256)?;
        verify_file_hash(&expected_source_manifest, &manifest.source_manifest_sha256)?;
        verify_file_hash(&expected_info, &manifest.host_info_plist_sha256)?;
        for executable in [&expected_helper, &expected_host] {
            let metadata = fs::symlink_metadata(executable)
                .map_err(|_| "owned_paste_session_executable_unavailable".to_owned())?;
            if metadata.permissions().mode() & 0o111 == 0 {
                return Err("owned_paste_session_executable_not_executable".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OwnedPasteSessionReadyV1 {
    pub(crate) helper_pid: i32,
    pub(crate) host_identity: KernelProcessIdentityV1,
    pub(crate) host_receipt_sha256: String,
}

#[derive(Debug, Clone)]
pub(crate) struct OwnedPasteSessionFinalV1 {
    pub(crate) programmatic_edit_receipt_sha256: String,
    pub(crate) source: String,
    pub(crate) actual_sha256: Option<String>,
    pub(crate) actual_byte_length: Option<usize>,
    pub(crate) actual_state: String,
    pub(crate) expected_comparison: String,
}

#[derive(Debug, Clone)]
pub(crate) struct OwnedPasteSessionCleanupV1 {
    pub(crate) helper_exit_confirmed: bool,
    pub(crate) helper_exit_status: Option<i32>,
    pub(crate) host_exit_confirmed: bool,
    pub(crate) host_exit_status: Option<i32>,
    pub(crate) host_pid: Option<i32>,
    pub(crate) reason: Option<String>,
}

pub(crate) struct OwnedPasteSessionProcessV1 {
    prepared: PreparedOwnedPasteSessionV1,
    plan_id: String,
    phase_id: String,
    operation_id: String,
    session_id: String,
    expires_at: Instant,
    cancellation: CancellationToken,
    child: Child,
    stdin: Option<ChildStdin>,
    events: Receiver<Result<serde_json::Value, String>>,
    reader_thread: Option<JoinHandle<()>>,
    pub(crate) ready: OwnedPasteSessionReadyV1,
    closed: bool,
}

/// Validate the fixed source/field contract in a child process after it has
/// received the parent-authenticated sequence-zero Ready frame. Artifact
/// digests whose values come from the prepared cache remain the parent's
/// responsibility and are checked by `PreparedOwnedPasteSessionV1` before
/// Ready is sent.
#[cfg(all(target_os = "macos", feature = "research-harness"))]
pub(crate) fn validate_registered_binding_fields(
    binding: &OwnedPasteSessionBindingV1,
) -> Result<(), String> {
    fn is_sha256(value: &str) -> bool {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }

    if binding.fixture_id != FIXTURE_ID
        || binding.source_manifest_sha256 != SOURCE_MANIFEST_SHA256
        || !is_sha256(&binding.helper_executable_sha256)
        || !is_sha256(&binding.host_executable_sha256)
        || binding.host_bundle_identifier != HOST_BUNDLE_ID
        || binding.host_info_plist_sha256 != INFO_PLIST_SHA256
        || binding.recipe_sha256 != RECIPE_SHA256
        || binding.window_title != WINDOW_TITLE
        || binding.control_identifier != CONTROL_ID
        || binding.inserted_text_sha256 != INSERTED_TEXT_SHA256
        || binding.inserted_text_byte_length != INSERTED_TEXT_BYTE_LENGTH
        || binding.expected_post_insert_sha256 != POST_INSERT_SHA256
        || binding.expected_post_insert_byte_length != POST_INSERT_BYTE_LENGTH
        || binding.expected_settled_sha256 != SETTLED_SHA256
        || binding.expected_settled_byte_length != SETTLED_BYTE_LENGTH
    {
        return Err("owned_paste_session_registered_binding_rejected".into());
    }
    Ok(())
}

impl OwnedPasteSessionProcessV1 {
    pub(crate) fn start(
        prepared: &PreparedOwnedPasteSessionV1,
        plan_id: &str,
        phase_id: &str,
        operation_id: &str,
        binding: &OwnedPasteSessionBindingV1,
        expires_at: Instant,
        cancellation: &CancellationToken,
    ) -> Result<Self, String> {
        validate_uuid(plan_id)?;
        validate_uuid(phase_id)?;
        validate_uuid(operation_id)?;
        require_owner_permit(expires_at, cancellation)?;
        prepared.validate_files()?;
        prepared.validate_binding(binding)?;
        require_owner_permit(expires_at, cancellation)?;
        let remaining_ms = expires_at
            .saturating_duration_since(Instant::now())
            .as_millis();
        let remaining_ms = u64::try_from(remaining_ms)
            .map_err(|_| "owned_paste_session_deadline_invalid".to_owned())?;
        if !(1..=30_000).contains(&remaining_ms) {
            return Err("owned_paste_session_deadline_invalid".into());
        }
        let session_id = format!("{plan_id}-{phase_id}-{operation_id}");
        let session_root = prepared.cache_root.join("sessions");
        ensure_private_directory(&session_root)?;
        let mut child = Command::new(&prepared.manifest.helper_path)
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
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "owned_paste_session_helper_spawn_failed".to_owned())?;
        let helper_pid = match i32::try_from(child.id()) {
            Ok(pid) => pid,
            Err(_) => {
                drop(child.stdin.take());
                drop(child.stdout.take());
                let _ = wait_for_child_status(&mut child);
                return Err("owned_paste_session_helper_pid_invalid".into());
            }
        };
        let (stdin, stdout) = match (child.stdin.take(), child.stdout.take()) {
            (Some(stdin), Some(stdout)) => (stdin, stdout),
            (stdin, stdout) => {
                drop(stdin);
                drop(stdout);
                let _ = wait_for_child_status(&mut child);
                return Err("owned_paste_session_helper_stdio_unavailable".into());
            }
        };
        let (sender, events) = mpsc::sync_channel(4);
        let reader_thread = Some(spawn_event_reader(stdout, sender));
        let mut process = Self {
            prepared: prepared.clone(),
            plan_id: plan_id.to_owned(),
            phase_id: phase_id.to_owned(),
            operation_id: operation_id.to_owned(),
            session_id: session_id.clone(),
            expires_at,
            cancellation: cancellation.clone(),
            child,
            stdin: Some(stdin),
            events,
            reader_thread,
            ready: OwnedPasteSessionReadyV1 {
                helper_pid,
                host_identity: KernelProcessIdentityV1 {
                    source: String::new(),
                    pid: 0,
                    parent_pid: 0,
                    user_id: 0,
                    start_seconds: String::new(),
                    start_microseconds: String::new(),
                    executable_path: String::new(),
                },
                host_receipt_sha256: String::new(),
            },
            closed: false,
        };
        let event_value = process.receive_event()?;
        require_owner_permit(expires_at, cancellation)?;
        process.ready = validate_ready_event(
            event_value,
            &process.prepared,
            binding,
            OwnedPasteSessionEventIdentity {
                plan_id: &process.plan_id,
                phase_id: &process.phase_id,
                operation_id: &process.operation_id,
                session_id: &process.session_id,
            },
            helper_pid,
        )?;
        Ok(process)
    }

    pub(crate) fn apply_declared_edit(
        &mut self,
        request_id: &str,
    ) -> Result<OwnedPasteSessionFinalV1, String> {
        validate_uuid(request_id)?;
        require_owner_permit(self.expires_at, &self.cancellation)?;
        self.send_command(request_id, "apply_declared_edit")?;
        let event_value = self.receive_event()?;
        require_owner_permit(self.expires_at, &self.cancellation)?;
        validate_final_event(
            event_value,
            &self.prepared,
            OwnedPasteSessionEventIdentity {
                plan_id: &self.plan_id,
                phase_id: &self.phase_id,
                operation_id: &self.operation_id,
                session_id: &self.session_id,
            },
            &self.ready.host_identity,
            request_id,
        )
    }

    /// Check the already-held helper/Host pair without asking the helper to
    /// perform another edit or AX read. This is used immediately before the
    /// registered paste owner receives post authority.
    pub(crate) fn registered_host_and_helper_are_live(&mut self) -> Result<bool, String> {
        if self
            .child
            .try_wait()
            .map_err(|_| "owned_paste_session_helper_status_failed".to_owned())?
            .is_some()
        {
            return Ok(false);
        }

        let ready = &self.ready.host_identity;
        if ready.source != "darwin_proc_pidinfo_bsdinfo_proc_pidpath"
            || ready.pid <= 0
            || ready.parent_pid != self.ready.helper_pid
            || ready.user_id == 0
            || !Path::new(&ready.executable_path).is_absolute()
        {
            return Ok(false);
        }
        let Some(start_seconds) = ready
            .start_seconds
            .parse::<u64>()
            .ok()
            .filter(|value| value.to_string() == ready.start_seconds)
            .filter(|value| *value > 0)
        else {
            return Ok(false);
        };
        let Some(start_microseconds) = ready
            .start_microseconds
            .parse::<u32>()
            .ok()
            .filter(|value| value.to_string() == ready.start_microseconds)
            .filter(|value| *value < 1_000_000)
        else {
            return Ok(false);
        };
        let identity = crate::paste::RegisteredResearchHostIdentityV1 {
            pid: ready.pid,
            parent_pid: ready.parent_pid,
            user_id: ready.user_id,
            start_seconds,
            start_microseconds,
            executable_path: PathBuf::from(&ready.executable_path),
        };
        Ok(crate::paste::registered_research_host_matches(&identity))
    }

    pub(crate) fn close(mut self, request_id: &str) -> Result<OwnedPasteSessionCleanupV1, String> {
        validate_uuid(request_id)?;
        if !self.closed {
            if Instant::now() < self.expires_at && !self.cancellation.is_cancelled() {
                self.send_command(request_id, "close")?;
            }
            // Closing stdin is also the helper's EOF shutdown input. It is
            // safe after the phase lease expires; the helper still owns and
            // waits for its Host child before it can report cleanup.
            self.stdin.take();
            let event_value = self.receive_cleanup_event()?;
            let cleanup = validate_closed_event(
                event_value,
                &self.plan_id,
                &self.phase_id,
                &self.operation_id,
                &self.session_id,
                self.ready.host_identity.pid,
                request_id,
            )?;
            let status = self
                .child
                .wait()
                .map_err(|_| "owned_paste_session_helper_wait_failed".to_owned())?;
            self.closed = true;
            if let Some(reader) = self.reader_thread.take() {
                reader
                    .join()
                    .map_err(|_| "owned_paste_session_stdout_owner_failed".to_owned())?;
            }
            let mut cleanup = cleanup;
            cleanup.helper_exit_confirmed = true;
            cleanup.helper_exit_status = status.code();
            if !status.success()
                || !cleanup.host_exit_confirmed
                || cleanup.host_exit_status != Some(0)
            {
                return Err("owned_paste_session_cleanup_not_confirmed".into());
            }
            return Ok(cleanup);
        }
        Err("owned_paste_session_already_closed".into())
    }

    fn send_command(&mut self, request_id: &str, command: &str) -> Result<(), String> {
        let request = serde_json::json!({
            "request_id": request_id,
            "command": command,
        });
        let bytes = serde_json::to_vec(&request)
            .map_err(|_| "owned_paste_session_command_encode_failed".to_owned())?;
        if bytes.len() > 8_192 {
            return Err("owned_paste_session_command_over_budget".into());
        }
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "owned_paste_session_stdin_closed".to_owned())?;
        stdin
            .write_all(&bytes)
            .and_then(|_| stdin.write_all(b"\n"))
            .and_then(|_| stdin.flush())
            .map_err(|_| "owned_paste_session_command_write_failed".to_owned())
    }

    fn receive_event(&mut self) -> Result<serde_json::Value, String> {
        loop {
            require_owner_permit(self.expires_at, &self.cancellation)?;
            match self.events.try_recv() {
                Ok(Ok(value)) => {
                    require_owner_permit(self.expires_at, &self.cancellation)?;
                    return Ok(value);
                }
                Ok(Err(reason)) => return Err(reason),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("owned_paste_session_stdout_closed".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|_| "owned_paste_session_helper_status_failed".to_owned())?
            {
                return match self.events.recv() {
                    Ok(Ok(value)) => {
                        require_owner_permit(self.expires_at, &self.cancellation)?;
                        Ok(value)
                    }
                    Ok(Err(reason)) => Err(reason),
                    Err(_) => Err(if status.success() {
                        "owned_paste_session_helper_exited_before_event".into()
                    } else {
                        "owned_paste_session_helper_failed".into()
                    }),
                };
            }
            let timeout = self
                .expires_at
                .saturating_duration_since(Instant::now())
                .min(std::time::Duration::from_millis(25));
            match self.events.recv_timeout(timeout) {
                Ok(Ok(value)) => return Ok(value),
                Ok(Err(reason)) => return Err(reason),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("owned_paste_session_stdout_closed".into());
                }
            }
        }
    }

    fn receive_cleanup_event(&mut self) -> Result<serde_json::Value, String> {
        loop {
            match self.events.try_recv() {
                Ok(Ok(value)) => return Ok(value),
                Ok(Err(reason)) => return Err(reason),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("owned_paste_session_stdout_closed_before_cleanup".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            self.child
                .try_wait()
                .map_err(|_| "owned_paste_session_helper_status_failed".to_owned())?;
            match self
                .events
                .recv_timeout(std::time::Duration::from_millis(25))
            {
                Ok(Ok(value)) => return Ok(value),
                Ok(Err(reason)) => return Err(reason),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("owned_paste_session_stdout_closed_before_cleanup".into());
                }
            }
        }
    }
}

impl Drop for OwnedPasteSessionProcessV1 {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        // EOF is a real helper shutdown command. Keep owning the helper until
        // it has stopped and waited for its retained Host child.
        self.stdin.take();
        // If wait/try_wait fails transiently, keep the Child in this owner and
        // retry with a delay. Dropping it or setting `closed` without an
        // observed exit status would detach the helper/Host chain.
        let _status = wait_for_child_status(&mut self.child);
        if let Some(reader) = self.reader_thread.take() {
            let _ = reader.join();
        }
        self.closed = true;
    }
}

fn wait_for_child_status(child: &mut Child) -> std::process::ExitStatus {
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
        thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn spawn_event_reader(
    stdout: impl Read + Send + 'static,
    sender: SyncSender<Result<serde_json::Value, String>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = Vec::new();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => return,
                Ok(_) if line.len() > EVENT_MAX_BYTES => {
                    let _ = sender.send(Err("owned_paste_session_event_over_budget".into()));
                    return;
                }
                Ok(_) => {
                    if line.last() == Some(&b'\n') {
                        line.pop();
                    }
                    match serde_json::from_slice(&line) {
                        Ok(value) => {
                            if sender.send(Ok(value)).is_err() {
                                return;
                            }
                        }
                        Err(_) => {
                            let _ = sender.send(Err("owned_paste_session_event_invalid".into()));
                            return;
                        }
                    }
                }
                Err(_) => {
                    let _ = sender.send(Err("owned_paste_session_stdout_read_failed".into()));
                    return;
                }
            }
        }
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct ReadyEventV1 {
    artifact_type: String,
    schema_version: u8,
    event: String,
    plan_id: String,
    phase_id: String,
    operation_id: String,
    session_id: String,
    helper_pid: i32,
    source_manifest_sha256: String,
    helper_executable_sha256: String,
    host_executable_sha256: String,
    host_bundle_identifier: String,
    recipe_sha256: String,
    host_receipt_sha256: Option<String>,
    host_receipt: Option<HostReceiptV1>,
    host_kernel_identity: Option<KernelProcessIdentityV1>,
    target_scope: TargetScopeV1,
    paste_input: HashFactV1,
    declared_settled_value: HashFactV1,
    learning: String,
    sql: String,
    settings: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct TargetScopeV1 {
    bundle_id: String,
    window_title: String,
    control_identifier: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct HashFactV1 {
    sha256: String,
    byte_length: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct FinalEventV1 {
    artifact_type: String,
    schema_version: u8,
    event: String,
    request_id: String,
    plan_id: String,
    phase_id: String,
    operation_id: String,
    session_id: String,
    host_pid: i32,
    host_kernel_identity: KernelProcessIdentityV1,
    programmatic_edit: ProgrammaticEditV1,
    actual_sample: FinalSampleV1,
    same_host_still_live: bool,
    target_terminal: String,
    learning: String,
    sql: String,
    settings: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct ProgrammaticEditV1 {
    state: String,
    actor: String,
    receipt_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct FinalSampleV1 {
    state: String,
    source: String,
    sha256: Option<String>,
    byte_length: Option<usize>,
    ax_readback: Option<AxReadbackV1>,
    process_identity_before: Option<ProcessIdentityObservationV1>,
    process_identity_after: Option<ProcessIdentityObservationV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct ClosedEventV1 {
    artifact_type: String,
    schema_version: u8,
    event: String,
    request_id: Option<String>,
    plan_id: String,
    phase_id: String,
    operation_id: String,
    session_id: Option<String>,
    cleanup: CleanupFactV1,
    reason: Option<String>,
    learning: Option<String>,
    sql: Option<String>,
    settings: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct CleanupFactV1 {
    state: String,
    child_pid: Option<i32>,
    termination_status: Option<i32>,
    exit_confirmed: bool,
    reason: Option<String>,
}

struct OwnedPasteSessionEventIdentity<'a> {
    plan_id: &'a str,
    phase_id: &'a str,
    operation_id: &'a str,
    session_id: &'a str,
}

fn validate_ready_event(
    value: serde_json::Value,
    prepared: &PreparedOwnedPasteSessionV1,
    binding: &OwnedPasteSessionBindingV1,
    identity: OwnedPasteSessionEventIdentity<'_>,
    helper_pid: i32,
) -> Result<OwnedPasteSessionReadyV1, String> {
    let event: ReadyEventV1 = serde_json::from_value(value)
        .map_err(|_| "owned_paste_session_ready_event_invalid".to_owned())?;
    let manifest = &prepared.manifest;
    if event.artifact_type != "voice_flow_owned_paste_session_event"
        || event.schema_version != 1
        || event.event != "session_ready"
        || event.plan_id != identity.plan_id
        || event.phase_id != identity.phase_id
        || event.operation_id != identity.operation_id
        || event.session_id != identity.session_id
        || event.helper_pid != helper_pid
        || event.source_manifest_sha256 != manifest.source_manifest_sha256
        || event.helper_executable_sha256 != manifest.helper_executable_sha256
        || event.host_executable_sha256 != manifest.host_executable_sha256
        || event.host_bundle_identifier != manifest.host_bundle_identifier
        || event.recipe_sha256 != manifest.recipe_sha256
        || event.learning != "not_run"
        || event.sql != "not_run"
        || event.settings != "not_run"
        || event.paste_input.sha256 != binding.inserted_text_sha256
        || event.paste_input.byte_length != binding.inserted_text_byte_length
        || event.declared_settled_value.sha256 != binding.expected_settled_sha256
        || event.declared_settled_value.byte_length != binding.expected_settled_byte_length
        || event.target_scope.bundle_id != binding.host_bundle_identifier
        || event.target_scope.window_title != binding.window_title
        || event.target_scope.control_identifier != binding.control_identifier
    {
        return Err("owned_paste_session_ready_binding_mismatch".into());
    }
    let host_identity = event
        .host_kernel_identity
        .ok_or_else(|| "owned_paste_session_host_identity_unknown".to_owned())?;
    if host_identity.pid <= 0
        || host_identity.parent_pid != helper_pid
        || host_identity.executable_path != manifest.host_executable_path
    {
        return Err("owned_paste_session_host_identity_mismatch".into());
    }
    let receipt = event
        .host_receipt
        .ok_or_else(|| "owned_paste_session_host_receipt_missing".to_owned())?;
    let receipt_sha256 = event
        .host_receipt_sha256
        .filter(|value| is_sha256(value))
        .ok_or_else(|| "owned_paste_session_host_receipt_hash_missing".to_owned())?;
    let session_dir = prepared
        .cache_root
        .join("sessions")
        .join(identity.session_id);
    require_real_directory(&session_dir)?;
    let ready_path = session_dir.join("ready.json");
    let ready_bytes = read_regular_file(&ready_path, 32_768)?;
    if sha256(&ready_bytes) != receipt_sha256 {
        return Err("owned_paste_session_host_receipt_hash_mismatch".into());
    }
    let persisted_receipt: HostReceiptV1 = serde_json::from_slice(&ready_bytes)
        .map_err(|_| "owned_paste_session_host_receipt_invalid".to_owned())?;
    let setup = receipt
        .setup_observation
        .as_ref()
        .ok_or_else(|| "owned_paste_session_setup_receipt_incomplete".to_owned())?;
    let native = receipt
        .native_application
        .as_ref()
        .ok_or_else(|| "owned_paste_session_bundle_receipt_incomplete".to_owned())?;
    if receipt.artifact_type != "voice_harness_host"
        || receipt.schema_version != 2
        || receipt.run_id != RUN_ID
        || receipt.case_id != CASE_ID
        || receipt.fixture_id != FIXTURE_ID
        || receipt.recipe_sha256 != binding.recipe_sha256
        || receipt.title != binding.window_title
        || receipt.control != binding.control_identifier
        || receipt.other_control != OTHER_CONTROL_ID
        || receipt.process_identity.kernel_state != "observed"
        || receipt.process_identity.kernel.as_ref() != Some(&host_identity)
        || parse_positive_canonical_decimal(&receipt.ready_uptime_ns).is_none()
        || setup.source != "FixtureHost_main_run_loop_after_applicationDidFinishLaunching"
        || setup.local_state != "declared_setup_observed"
        || setup.independent_ax_readiness != "not_observed"
        || !setup.target_first_responder
        || !setup.window_visible
        || !setup.target_attached
        || !setup.other_attached
        || !setup.declared_text_and_range
        || native.source != "FixtureHost_Bundle.main"
        || native.state != "observed_app_bundle"
        || native.bundle_path.as_deref() != Some(manifest.host_app_path.as_str())
        || native.bundle_identifier.as_deref() != Some(binding.host_bundle_identifier.as_str())
        || native.executable_path.as_deref() != Some(manifest.host_executable_path.as_str())
        || native.info_plist_sha256.as_deref() != Some(manifest.host_info_plist_sha256.as_str())
        || native.reason.is_some()
        || persisted_receipt != receipt
    {
        return Err("owned_paste_session_host_receipt_mismatch".into());
    }
    Ok(OwnedPasteSessionReadyV1 {
        helper_pid,
        host_identity,
        host_receipt_sha256: receipt_sha256,
    })
}

fn validate_final_event(
    value: serde_json::Value,
    prepared: &PreparedOwnedPasteSessionV1,
    identity: OwnedPasteSessionEventIdentity<'_>,
    host_identity: &KernelProcessIdentityV1,
    request_id: &str,
) -> Result<OwnedPasteSessionFinalV1, String> {
    let event: FinalEventV1 = serde_json::from_value(value)
        .map_err(|_| "owned_paste_session_final_event_invalid".to_owned())?;
    if event.artifact_type != "voice_flow_owned_paste_session_event"
        || event.schema_version != 1
        || event.event != "programmatic_edit_and_final_ax"
        || event.request_id != request_id
        || event.plan_id != identity.plan_id
        || event.phase_id != identity.phase_id
        || event.operation_id != identity.operation_id
        || event.session_id != identity.session_id
        || event.host_pid != host_identity.pid
        || &event.host_kernel_identity != host_identity
        || !event.same_host_still_live
        || event.target_terminal != "not_product_target_terminal"
        || event.learning != "not_run"
        || event.sql != "not_run"
        || event.settings != "not_run"
        || event.programmatic_edit.state != "observed"
        || event.programmatic_edit.actor != "owned_fixture_host_programmatic_setter"
        || !is_sha256(&event.programmatic_edit.receipt_sha256)
    {
        return Err("owned_paste_session_final_binding_mismatch".into());
    }
    let sample = event.actual_sample;
    if sample.source != FINAL_AX_SOURCE
        || sample
            .ax_readback
            .as_ref()
            .is_some_and(|readback| readback.source != sample.source)
    {
        return Err("owned_paste_session_final_ax_source_mismatch".into());
    }
    let session_dir = prepared
        .cache_root
        .join("sessions")
        .join(identity.session_id);
    let edit_path = session_dir.join("learning-edit.json");
    let edit_bytes = read_regular_file(&edit_path, 8_192)?;
    if sha256(&edit_bytes) != event.programmatic_edit.receipt_sha256 {
        return Err("owned_paste_session_edit_receipt_hash_mismatch".into());
    }
    let expected_comparison = match sample.state.as_str() {
        "observed" => {
            let ax = sample
                .ax_readback
                .as_ref()
                .ok_or_else(|| "owned_paste_session_final_ax_evidence_missing".to_owned())?;
            let digest = sample
                .sha256
                .as_deref()
                .filter(|value| is_sha256(value))
                .ok_or_else(|| "owned_paste_session_final_ax_hash_missing".to_owned())?;
            let byte_length = sample
                .byte_length
                .filter(|length| *length <= 65_536)
                .ok_or_else(|| "owned_paste_session_final_ax_length_missing".to_owned())?;
            if ax.source != FINAL_AX_SOURCE
                || ax.applicability != "matched"
                || ax.reason != "observed"
                || !identity_observation_matches(
                    sample.process_identity_before.as_ref(),
                    host_identity,
                )
                || !identity_observation_matches(
                    sample.process_identity_after.as_ref(),
                    host_identity,
                )
            {
                return Err("owned_paste_session_final_ax_binding_mismatch".into());
            }
            if digest == prepared.manifest.expected_settled_sha256
                && byte_length == prepared.manifest.expected_settled_byte_length
            {
                "match"
            } else {
                "mismatch"
            }
        }
        "changed" | "unknown" => {
            if sample.sha256.is_some() || sample.byte_length.is_some() {
                return Err("owned_paste_session_nonobserved_sample_has_value".into());
            }
            "unknown"
        }
        _ => return Err("owned_paste_session_final_sample_state_invalid".into()),
    };
    Ok(OwnedPasteSessionFinalV1 {
        programmatic_edit_receipt_sha256: event.programmatic_edit.receipt_sha256,
        source: sample.source,
        actual_sha256: sample.sha256,
        actual_byte_length: sample.byte_length,
        actual_state: sample.state,
        expected_comparison: expected_comparison.to_owned(),
    })
}

fn validate_closed_event(
    value: serde_json::Value,
    plan_id: &str,
    phase_id: &str,
    operation_id: &str,
    session_id: &str,
    expected_host_pid: i32,
    request_id: &str,
) -> Result<OwnedPasteSessionCleanupV1, String> {
    let event: ClosedEventV1 = serde_json::from_value(value)
        .map_err(|_| "owned_paste_session_closed_event_invalid".to_owned())?;
    if event.artifact_type != "voice_flow_owned_paste_session_event"
        || event.schema_version != 1
        || event.event != "session_closed"
        || event
            .request_id
            .as_deref()
            .is_some_and(|id| id != request_id)
        || (event.request_id.is_none()
            && !matches!(
                event.reason.as_deref(),
                Some("stdin_closed" | "phase_deadline_expired")
            ))
        || event.plan_id != plan_id
        || event.phase_id != phase_id
        || event.operation_id != operation_id
        || event
            .session_id
            .as_deref()
            .is_some_and(|id| id != session_id)
        || (event.session_id.is_none() && event.request_id.is_some())
        || event
            .learning
            .as_deref()
            .is_some_and(|value| value != "not_run")
        || event.sql.as_deref().is_some_and(|value| value != "not_run")
        || event
            .settings
            .as_deref()
            .is_some_and(|value| value != "not_run")
        || event.cleanup.state != "owned_host_exit_confirmed"
        || !event.cleanup.exit_confirmed
        || event.cleanup.child_pid != Some(expected_host_pid)
        || event.cleanup.termination_status != Some(0)
    {
        return Err("owned_paste_session_cleanup_unknown".into());
    }
    Ok(OwnedPasteSessionCleanupV1 {
        helper_exit_confirmed: false,
        helper_exit_status: None,
        host_exit_confirmed: true,
        host_exit_status: event.cleanup.termination_status,
        host_pid: event.cleanup.child_pid,
        reason: event.reason.or(event.cleanup.reason),
    })
}

fn parse_positive_canonical_decimal(value: &str) -> Option<u128> {
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return None;
    }
    value.parse::<u128>().ok().filter(|parsed| *parsed > 0)
}

fn identity_observation_matches(
    observation: Option<&ProcessIdentityObservationV1>,
    expected: &KernelProcessIdentityV1,
) -> bool {
    observation.is_some_and(|observation| {
        observation.kernel_state == "observed"
            && observation.requested_pid == expected.pid
            && observation.kernel.as_ref() == Some(expected)
    })
}

fn require_owner_permit(
    expires_at: Instant,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    if cancellation.is_cancelled() {
        return Err("owned_paste_session_cancelled".into());
    }
    if Instant::now() >= expires_at {
        return Err("owned_paste_session_expired".into());
    }
    Ok(())
}

fn validate_uuid(value: &str) -> Result<(), String> {
    crate::onboarding::validate_id(value).map_err(|_| "owned_paste_session_uuid_invalid".to_owned())
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

fn verify_file_hash(path: &Path, expected: &str) -> Result<(), String> {
    let bytes = read_regular_file(path, 67_108_864)?;
    if sha256(&bytes) != expected {
        return Err("owned_paste_session_prepared_hash_mismatch".into());
    }
    Ok(())
}

fn read_regular_file(path: &Path, max_bytes: usize) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| "owned_paste_session_file_unavailable".to_owned())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > max_bytes as u64
    {
        return Err("owned_paste_session_file_rejected".into());
    }
    let mut options = OpenOptions::new();
    options.read(true).custom_flags(libc::O_NOFOLLOW);
    let file = options
        .open(path)
        .map_err(|_| "owned_paste_session_file_unavailable".to_owned())?;
    let mut bytes = Vec::new();
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "owned_paste_session_file_read_failed".to_owned())?;
    if bytes.len() > max_bytes {
        return Err("owned_paste_session_file_rejected".into());
    }
    Ok(bytes)
}

fn require_real_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| "owned_paste_session_directory_unavailable".to_owned())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("owned_paste_session_directory_rejected".into());
    }
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    if !path.exists() {
        fs::create_dir(path).map_err(|_| "owned_paste_session_cache_unavailable".to_owned())?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| "owned_paste_session_cache_permissions_failed".to_owned())?;
    }
    require_real_directory(path)
}
