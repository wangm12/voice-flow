use serde::Serialize;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use tauri::Manager;

const RETENTION_MS: u128 = 30 * 24 * 60 * 60 * 1_000;
const MAX_LOG_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DeliveryDiagnostic {
    pub(crate) code: &'static str,
    pub(crate) user_reason: &'static str,
    pub(crate) stage: &'static str,
    /// The target app bundle identifier is useful for diagnosis and contains
    /// no page, window, or focused-field content.
    pub(crate) target_app: Option<String>,
    pub(crate) accessibility_available: bool,
    pub(crate) target_verified: Option<bool>,
    pub(crate) focused_input_available: Option<bool>,
    pub(crate) clipboard_snapshot_captured: bool,
    pub(crate) clipboard_unchanged_before_write: Option<bool>,
    pub(crate) clipboard_write_attempted: bool,
    pub(crate) clipboard_write_failed: bool,
    pub(crate) clipboard_write_owned: Option<bool>,
    pub(crate) keyboard_paste_attempted: bool,
    pub(crate) keyboard_paste_may_have_been_posted: bool,
    pub(crate) paste_verified: Option<bool>,
    pub(crate) clipboard_restore_attempted: bool,
    pub(crate) clipboard_restored: Option<bool>,
}

#[derive(Serialize)]
struct LogRecord<'a> {
    timestamp_ms: u128,
    diagnostic: &'a DeliveryDiagnostic,
}

/// Store only bounded, structured delivery facts. The caller deliberately
/// passes no transcript or raw system error text.
pub(crate) fn record(app: &tauri::AppHandle, diagnostic: &DeliveryDiagnostic) {
    let Some(dir) = app
        .path()
        .app_data_dir()
        .ok()
        .map(|path| path.join("diagnostics"))
    else {
        return;
    };
    let _guard = write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if write_bounded_jsonl(&dir, diagnostic).is_err() {
        log::debug!("could not persist local delivery diagnostic");
    }
}

pub(crate) fn prune_expired(app: &tauri::AppHandle) {
    let Some(dir) = app
        .path()
        .app_data_dir()
        .ok()
        .map(|path| path.join("diagnostics"))
    else {
        return;
    };
    let _guard = write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if prune_existing_log(&dir).is_err() {
        log::debug!("could not prune local delivery diagnostics");
    }
}

pub(crate) fn clear(app_data_dir: &Path) -> anyhow::Result<()> {
    let _guard = write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = app_data_dir.join("diagnostics");
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn write_bounded_jsonl(dir: &Path, diagnostic: &DeliveryDiagnostic) -> anyhow::Result<()> {
    fs::create_dir_all(dir)?;
    restrict_directory_mode(dir)?;
    let path = dir.join("paste-delivery.jsonl");
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis();
    let record = serde_json::to_vec(&LogRecord {
        timestamp_ms: now_ms,
        diagnostic,
    })?;
    let mut next = Vec::<Vec<u8>>::new();
    if let Ok(contents) = fs::read(&path) {
        let cutoff = now_ms.saturating_sub(RETENTION_MS);
        next.extend(contents.split(|byte| *byte == b'\n').filter_map(|line| {
            if line.is_empty() {
                return None;
            }
            let value: serde_json::Value = serde_json::from_slice(line).ok()?;
            let timestamp = value.get("timestamp_ms")?.as_u64()? as u128;
            (timestamp >= cutoff).then(|| line.to_vec())
        }));
    }
    next.push(record);
    let mut total_bytes = next.iter().map(|line| line.len() + 1).sum::<usize>();
    while total_bytes > MAX_LOG_BYTES && next.len() > 1 {
        if let Some(removed) = next.first() {
            total_bytes = total_bytes.saturating_sub(removed.len() + 1);
        }
        next.remove(0);
    }

    let temporary = dir.join(format!("paste-delivery.{}.tmp", std::process::id()));
    let mut file = fs::File::create(&temporary)?;
    restrict_file_mode(&temporary)?;
    for line in next {
        file.write_all(&line)?;
        file.write_all(b"\n")?;
    }
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, path)?;
    restrict_file_mode(&dir.join("paste-delivery.jsonl"))?;
    Ok(())
}

fn prune_existing_log(dir: &Path) -> anyhow::Result<()> {
    let path = dir.join("paste-delivery.jsonl");
    let contents = match fs::read(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis();
    let cutoff = now_ms.saturating_sub(RETENTION_MS);
    let mut lines = contents
        .split(|byte| *byte == b'\n')
        .filter_map(|line| {
            if line.is_empty() {
                return None;
            }
            let value: serde_json::Value = serde_json::from_slice(line).ok()?;
            let timestamp = value.get("timestamp_ms")?.as_u64()? as u128;
            (timestamp >= cutoff).then(|| line.to_vec())
        })
        .collect::<Vec<_>>();
    let mut total_bytes = lines.iter().map(|line| line.len() + 1).sum::<usize>();
    while total_bytes > MAX_LOG_BYTES && !lines.is_empty() {
        if let Some(removed) = lines.first() {
            total_bytes = total_bytes.saturating_sub(removed.len() + 1);
        }
        lines.remove(0);
    }
    if lines.is_empty() {
        fs::remove_file(path)?;
        return Ok(());
    }

    let temporary = dir.join(format!("paste-delivery.{}.tmp", std::process::id()));
    let mut file = fs::File::create(&temporary)?;
    restrict_file_mode(&temporary)?;
    for line in lines {
        file.write_all(&line)?;
        file.write_all(b"\n")?;
    }
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, path)?;
    restrict_file_mode(&dir.join("paste-delivery.jsonl"))?;
    Ok(())
}

#[cfg(unix)]
fn restrict_directory_mode(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_directory_mode(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn restrict_file_mode(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_file_mode(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}
