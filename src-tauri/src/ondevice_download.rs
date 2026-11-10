//! On-device model download, hash verify, and safe extract. No ORT / infer.

use crate::ondevice_asr::{
    delete_model, list_models, model_dir, validate_model_id, OnDeviceModelState,
    OnDeviceModelStatus,
};
use crate::ondevice_models::{is_mlx_model, mlx_model, SENSEVOICE_ARCHIVE_SHA256, SENSEVOICE_ID};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const DOWNLOAD_EVENT: &str = "ondevice://download";
pub const SENSEVOICE_LABEL: &str = "SenseVoice Small";
const MLX_INSTALLED_MANIFEST: &str = ".voiceflow-model-manifest.json";

#[derive(Clone)]
pub struct DownloadManager {
    inner: Arc<Mutex<ManagerInner>>,
}

struct ManagerInner {
    job: Option<ActiveJob>,
    last_error: Option<(String, String)>,
    next_generation: u64,
    deleting: bool,
}

struct ActiveJob {
    id: String,
    generation: u64,
    cancel: CancellationToken,
    downloaded_bytes: Arc<AtomicU64>,
}

pub struct JobHandle {
    id: String,
    generation: u64,
    cancel: CancellationToken,
    downloaded_bytes: Arc<AtomicU64>,
}

struct DownloadWriter {
    file: File,
    hasher: Sha256,
    written: u64,
}

impl DownloadWriter {
    fn create(path: &Path) -> Result<Self, String> {
        let file = File::create(path).map_err(|error| error.to_string())?;
        restrict_file_mode(path)?;
        Ok(Self {
            file,
            hasher: Sha256::new(),
            written: 0,
        })
    }

    fn write_chunk(
        &mut self,
        chunk: &[u8],
        expected_bytes: u64,
        progress: Option<(Arc<AtomicU64>, u64)>,
        exceeds_message: &str,
    ) -> Result<(), String> {
        self.written = self
            .written
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| exceeds_message.to_owned())?;
        if self.written > expected_bytes {
            return Err(exceeds_message.into());
        }
        self.file
            .write_all(chunk)
            .map_err(|error| error.to_string())?;
        self.hasher.update(chunk);
        if let Some((downloaded_bytes, base)) = progress {
            downloaded_bytes.store(base.saturating_add(self.written), Ordering::Relaxed);
        }
        Ok(())
    }

    fn finish(
        self,
        expected_bytes: u64,
        expected_sha256: &str,
        size_mismatch_message: &str,
        hash_mismatch_message: &str,
    ) -> Result<(), String> {
        if self.written != expected_bytes {
            return Err(size_mismatch_message.into());
        }
        self.file.sync_all().map_err(|error| error.to_string())?;
        let digest = hex::encode(self.hasher.finalize());
        if !digest.eq_ignore_ascii_case(expected_sha256) {
            return Err(hash_mismatch_message.into());
        }
        Ok(())
    }
}

impl DownloadManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ManagerInner {
                job: None,
                last_error: None,
                next_generation: 0,
                deleting: false,
            })),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ManagerInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn begin(&self, id: &str) -> Result<JobHandle, String> {
        let id = validate_model_id(id)?;
        let mut inner = self.lock();
        if inner.deleting {
            return Err("model is being deleted".into());
        }
        if inner.job.is_some() {
            return Err("download already in progress".into());
        }
        inner.last_error = None;
        inner.next_generation = inner.next_generation.wrapping_add(1);
        let generation = inner.next_generation;
        let cancel = CancellationToken::new();
        let downloaded_bytes = Arc::new(AtomicU64::new(0));
        inner.job = Some(ActiveJob {
            id: id.to_owned(),
            generation,
            cancel: cancel.clone(),
            downloaded_bytes: Arc::clone(&downloaded_bytes),
        });
        Ok(JobHandle {
            id: id.to_owned(),
            generation,
            cancel,
            downloaded_bytes,
        })
    }

    pub fn cancel(&self, id: &str) -> Result<(), String> {
        let id = validate_model_id(id)?;
        let inner = self.lock();
        if let Some(job) = inner.job.as_ref() {
            if job.id == id {
                job.cancel.cancel();
            }
        }
        Ok(())
    }

    pub fn cancel_all(&self) {
        if let Some(job) = self.lock().job.as_ref() {
            job.cancel.cancel();
        }
    }

    pub fn finish(&self, id: &str, generation: u64, error: Option<String>) {
        let mut inner = self.lock();
        let Some(job) = inner.job.as_ref() else {
            return;
        };
        if job.id != id || job.generation != generation {
            return;
        }
        inner.job = None;
        inner.last_error = error.map(|message| (id.to_owned(), sanitize_download_error(&message)));
    }

    fn job_generation(&self, id: &str) -> Option<u64> {
        self.lock()
            .job
            .as_ref()
            .filter(|job| job.id == id)
            .map(|job| job.generation)
    }

    fn start_delete(&self, id: &str) -> Result<Option<u64>, String> {
        let id = validate_model_id(id)?;
        let mut inner = self.lock();
        if inner.deleting {
            return Err("model is being deleted".into());
        }
        inner.deleting = true;
        if let Some(job) = inner.job.as_ref() {
            if job.id == id {
                job.cancel.cancel();
                return Ok(Some(job.generation));
            }
        }
        Ok(None)
    }

    fn finish_delete(&self) {
        self.lock().deleting = false;
    }

    fn should_delete_files(&self, id: &str, cancelled_generation: Option<u64>) -> bool {
        match (cancelled_generation, self.job_generation(id)) {
            (_, Some(_)) => false,
            (None, None) | (Some(_), None) => true,
        }
    }

    fn delete_files_if_safe(
        &self,
        models_root: &Path,
        id: &str,
        cancelled_generation: Option<u64>,
    ) -> Result<(), String> {
        if !self.should_delete_files(id, cancelled_generation) {
            return Ok(());
        }
        delete_model(models_root, id)
    }

    fn is_current(&self, handle: &JobHandle) -> bool {
        self.lock()
            .job
            .as_ref()
            .is_some_and(|job| job.id == handle.id && job.generation == handle.generation)
    }

    fn clear_last_error(&self, id: &str) {
        let mut inner = self.lock();
        if inner
            .last_error
            .as_ref()
            .is_some_and(|(job_id, _)| job_id == id)
        {
            inner.last_error = None;
        }
    }

    async fn wait_until_stopped(&self, id: &str, generation: u64) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            if self.job_generation(id) != Some(generation) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("download is still stopping".into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub fn downloaded_bytes(&self, id: &str) -> u64 {
        self.lock()
            .job
            .as_ref()
            .filter(|job| job.id == id)
            .map(|job| job.downloaded_bytes.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    pub fn last_error(&self, id: &str) -> Option<String> {
        self.lock()
            .last_error
            .as_ref()
            .filter(|(job_id, _)| job_id == id)
            .map(|(_, message)| message.clone())
    }
}

impl Default for DownloadManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for DownloadManager {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            self.cancel_all();
        }
    }
}

#[cfg(test)]
pub fn catalog_status(models_root: &Path, manager: &DownloadManager) -> OnDeviceModelStatus {
    status_for(models_root, manager, SENSEVOICE_ID)
        .expect("catalog always includes SenseVoice Small")
}

pub fn list_statuses_with_runtime(
    models_root: &Path,
    manager: &DownloadManager,
    runtime: Option<&crate::ondevice_runtime::RuntimeSnapshot>,
) -> Vec<OnDeviceModelStatus> {
    list_models(models_root)
        .into_iter()
        .map(|info| status_from_info(models_root, manager, info, runtime))
        .collect()
}

fn status_for(
    models_root: &Path,
    manager: &DownloadManager,
    id: &str,
) -> Option<OnDeviceModelStatus> {
    list_models(models_root)
        .into_iter()
        .find(|info| info.id == id)
        .map(|info| status_from_info(models_root, manager, info, None))
}

fn status_from_info(
    _models_root: &Path,
    manager: &DownloadManager,
    info: crate::ondevice_asr::OnDeviceModelInfo,
    runtime: Option<&crate::ondevice_runtime::RuntimeSnapshot>,
) -> OnDeviceModelStatus {
    let downloading = manager
        .lock()
        .job
        .as_ref()
        .is_some_and(|job| job.id == info.id);
    let loading_this_model = runtime
        .and_then(|runtime| runtime.loading_model_id.as_deref())
        .is_some_and(|loading_id| loading_id == info.id);
    let runtime_status = if !is_mlx_model(&info.id) {
        "legacy_no_runtime"
    } else if let Some(runtime) = runtime {
        if loading_this_model {
            "loading"
        } else if runtime.runtime_status == "loading" {
            if runtime.runtime_version.is_some() {
                "runtime_ready"
            } else {
                "not_checked"
            }
        } else if runtime.runtime_status == "loaded"
            && runtime.loaded_model_id.as_deref() != Some(info.id.as_str())
        {
            "runtime_ready"
        } else {
            runtime.runtime_status.as_str()
        }
    } else if !info.platform_supported {
        "unsupported_platform"
    } else if crate::ondevice_runtime::sidecar_executable().is_none() {
        "sidecar_missing"
    } else {
        "not_checked"
    };
    let loaded = runtime.is_some_and(|runtime| {
        runtime.runtime_status == "loaded"
            && runtime.loaded_model_id.as_deref() == Some(info.id.as_str())
    });
    let error = if downloading {
        None
    } else {
        manager
            .last_error(&info.id)
            .or_else(|| runtime.and_then(|runtime| runtime.error.clone()))
    };
    let memory_compatibility_error = if info.download_available && is_mlx_model(&info.id) {
        info.revision
            .as_deref()
            .and_then(|revision| {
                crate::model_resources::static_asr_memory_compatibility(&info.id, revision).err()
            })
            .map(asr_memory_compatibility_error_code)
            .map(str::to_owned)
    } else {
        None
    };
    OnDeviceModelStatus {
        id: info.id.clone(),
        label: info.label,
        bytes: info.bytes,
        sha256: (info.id == SENSEVOICE_ID).then(|| SENSEVOICE_ARCHIVE_SHA256.to_owned()),
        state: if downloading {
            OnDeviceModelState::Downloading
        } else {
            info.state
        },
        inference_ready: is_mlx_model(&info.id)
            && info.platform_supported
            && matches!(&info.state, crate::ondevice_asr::OnDeviceModelState::Ready)
            && runtime.is_some_and(|runtime| {
                matches!(runtime.runtime_status.as_str(), "runtime_ready" | "loaded")
                    || (runtime.runtime_status == "loading" && runtime.runtime_version.is_some())
            })
            && matches!(runtime_status, "runtime_ready" | "loaded"),
        downloaded_bytes: if downloading {
            manager.downloaded_bytes(&info.id)
        } else {
            0
        },
        error_category: error
            .as_deref()
            .and_then(download_error_category)
            .map(str::to_owned),
        error,
        revision: info.revision,
        file_count: info.file_count,
        platform_supported: info.platform_supported,
        runtime_status: runtime_status.to_owned(),
        loaded,
        download_available: info.download_available,
        language_note: info.language_note,
        memory_compatibility_error,
    }
}

fn asr_memory_compatibility_error_code(
    reason: crate::model_resources::MemoryUnavailableReason,
) -> &'static str {
    match reason {
        crate::model_resources::MemoryUnavailableReason::BudgetExceeded => "insufficient_memory",
        crate::model_resources::MemoryUnavailableReason::UnknownPhysicalMemory => "memory_unknown",
        _ => "memory_unavailable",
    }
}

fn download_error_category(message: &str) -> Option<&'static str> {
    let normalized = message.to_ascii_lowercase();
    (normalized.contains("no space left on device") || normalized.contains("enospc"))
        .then_some("disk_full")
}

pub fn sanitize_download_error(message: &str) -> String {
    let mut result = String::new();
    let mut rest = message;
    while let Some(scheme_at) = rest.find("://") {
        result.push_str(&rest[..scheme_at]);
        let after = &rest[scheme_at..];
        let url_end = after.find(char::is_whitespace).unwrap_or(after.len());
        let url = &after[..url_end];
        match url.find('?') {
            Some(query) => result.push_str(&url[..query]),
            None => result.push_str(url),
        }
        rest = &after[url_end..];
    }
    result.push_str(rest);
    result
}

pub fn partial_path(models_root: &Path, id: &str) -> PathBuf {
    models_root.join(format!("{id}.partial"))
}

fn replace_model_dir(src: &Path, dest: &Path) -> Result<(), String> {
    if !src.is_dir() {
        return Err("extract directory is missing".into());
    }
    if !dest.exists() {
        fs::rename(src, dest).map_err(|error| error.to_string())?;
        return Ok(());
    }
    let backup = dest.with_file_name(format!(
        "{}.old",
        dest.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("model")
    ));
    if backup.exists() {
        remove_path(&backup);
    }
    fs::rename(dest, &backup).map_err(|error| error.to_string())?;
    if let Err(error) = fs::rename(src, dest) {
        let _ = fs::rename(&backup, dest);
        return Err(error.to_string());
    }
    remove_path(&backup);
    Ok(())
}

fn ensure_private_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| error.to_string())?;
    restrict_directory_mode(path)
}

fn restrict_directory_mode(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("model staging path contains a symlink or non-directory".into());
    }
    Ok(())
}

fn ensure_safe_model_parent(
    staging: &Path,
    model_id: &str,
    manifest_path: &str,
) -> Result<(), String> {
    let parent = Path::new(manifest_path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let Some(parent) = parent else {
        return Ok(());
    };
    if model_id != crate::ondevice_models::FUN_ASR_NANO_2512_ID {
        return Err("nested model files are not allowlisted".into());
    }
    let mut directory = staging.to_path_buf();
    for component in parent.components() {
        let Component::Normal(name) = component else {
            return Err("model manifest path is not allowlisted".into());
        };
        directory.push(name);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err("model staging path contains a symlink or non-directory".into());
            }
            Ok(_) => restrict_directory_mode(&directory)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&directory).map_err(|error| error.to_string())?;
                restrict_directory_mode(&directory)?;
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

fn restrict_file_mode(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }
    let _ = path;
    Ok(())
}

fn write_atomic_text(path: &Path, text: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "invalid sidecar path".to_string())?;
    ensure_private_dir(parent)?;
    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("sha"),
        std::process::id()
    ));
    let result = (|| -> Result<(), String> {
        let mut file = File::create(&tmp).map_err(|error| error.to_string())?;
        restrict_file_mode(&tmp)?;
        file.write_all(text.as_bytes())
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&tmp, path).map_err(|error| error.to_string())?;
        restrict_file_mode(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn remove_path(path: &Path) {
    if path.is_dir() {
        let _ = fs::remove_dir_all(path);
    } else if path.exists() {
        let _ = fs::remove_file(path);
    }
}

pub fn start_background_download<E>(
    models_root: PathBuf,
    manager: DownloadManager,
    id: String,
    emit: E,
) -> Result<(), String>
where
    E: Fn(OnDeviceModelStatus) + Send + Sync + 'static,
{
    let id = validate_model_id(&id)?.to_owned();
    let Some(manifest) = mlx_model(&id) else {
        return Err("this on-device model is no longer available for download".into());
    };
    if !manifest.download_available {
        return Err("this on-device model is not available for new downloads".into());
    }
    if matches!(
        crate::ondevice_asr::model_state(&models_root, &id),
        OnDeviceModelState::Ready
    ) {
        return Err("model is already downloaded".into());
    }
    if let Err(reason) =
        crate::model_resources::static_asr_memory_compatibility(&id, manifest.revision)
    {
        return Err(asr_memory_compatibility_error_code(reason).into());
    }
    let handle = manager.begin(&id)?;
    let emit = Arc::new(emit);
    let status_id = id.clone();
    if let Some(status) = status_for(&models_root, &manager, &id) {
        emit(status);
    }
    let stop = CancellationToken::new();
    {
        let emit = Arc::clone(&emit);
        let models_root = models_root.clone();
        let manager = manager.clone();
        let job_cancel = handle.cancel.clone();
        let stop = stop.clone();
        let status_id = status_id.clone();
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(250));
            loop {
                tokio::select! {
                    _ = job_cancel.cancelled() => break,
                    _ = stop.cancelled() => break,
                    _ = interval.tick() => {
                        if let Some(status) = status_for(&models_root, &manager, &status_id) {
                            emit(status);
                        }
                    }
                }
            }
        });
    }
    tauri::async_runtime::spawn(async move {
        let result = download_mlx_model(&handle, &manager, &models_root).await;
        manager.finish(&id, handle.generation, result.err());
        stop.cancel();
        if let Some(status) = status_for(&models_root, &manager, &id) {
            emit(status);
        }
    });
    Ok(())
}

#[derive(serde::Serialize)]
struct InstalledManifest<'a> {
    schema_version: u32,
    id: &'a str,
    revision: &'a str,
    files: Vec<InstalledManifestFile<'a>>,
}

#[derive(serde::Serialize)]
struct InstalledManifestFile<'a> {
    path: &'a str,
    bytes: u64,
    sha256: &'a str,
}

async fn download_mlx_model(
    handle: &JobHandle,
    manager: &DownloadManager,
    models_root: &Path,
) -> Result<(), String> {
    let manifest = mlx_model(&handle.id).ok_or_else(|| "invalid on-device model id".to_owned())?;
    ensure_private_dir(models_root)?;
    let staging = partial_path(models_root, &handle.id);
    match fs::symlink_metadata(&staging) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err("model staging path contains a symlink or non-directory".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&staging).map_err(|error| error.to_string())?;
        }
        Err(error) => return Err(error.to_string()),
    }
    restrict_directory_mode(&staging)?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60 * 60))
        .user_agent("VoiceFlow/0.1")
        .https_only(true)
        .build()
        .map_err(|error| sanitize_download_error(&error.to_string()))?;

    let mut completed_bytes = 0u64;
    for file in manifest.files {
        if handle.cancel.is_cancelled() || !manager.is_current(handle) {
            return Err("download cancelled".into());
        }
        validate_manifest_path(&handle.id, file.path)?;
        ensure_safe_model_parent(&staging, &handle.id, file.path)?;
        let staged_file = staging.join(file.path);
        let verify_path = staged_file.clone();
        let expected_bytes = file.bytes;
        let expected_sha = file.sha256.to_owned();
        let verified = tokio::task::spawn_blocking(move || {
            verify_pinned_file(&verify_path, expected_bytes, &expected_sha)
        })
        .await
        .map_err(|error| format!("model verification task failed: {error}"))??;
        if verified {
            completed_bytes = completed_bytes.saturating_add(file.bytes);
            handle
                .downloaded_bytes
                .store(completed_bytes, Ordering::Relaxed);
            continue;
        }
        remove_path(&staged_file);
        let file_name = staged_file
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "invalid model manifest path".to_owned())?;
        let temporary = staged_file.with_file_name(format!(".{file_name}.download"));
        remove_path(&temporary);
        let url = format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            manifest.repository, manifest.revision, file.path
        );
        let response = tokio::select! {
            _ = handle.cancel.cancelled() => return Err("download cancelled".into()),
            response = client.get(url).send() => {
                response.map_err(|error| sanitize_download_error(&error.to_string()))?
            }
        };
        if !response.status().is_success() {
            return Err(format!(
                "model file download failed ({})",
                response.status().as_u16()
            ));
        }
        write_mlx_file_stream(
            response.bytes_stream(),
            &temporary,
            file.bytes,
            file.sha256,
            Arc::clone(&handle.downloaded_bytes),
            completed_bytes,
            &handle.cancel,
        )
        .await?;
        fs::rename(&temporary, &staged_file).map_err(|error| error.to_string())?;
        restrict_file_mode(&staged_file)?;
        completed_bytes = completed_bytes.saturating_add(file.bytes);
        handle
            .downloaded_bytes
            .store(completed_bytes, Ordering::Relaxed);
    }

    let installed = InstalledManifest {
        schema_version: 1,
        id: manifest.id,
        revision: manifest.revision,
        files: manifest
            .files
            .iter()
            .map(|file| InstalledManifestFile {
                path: file.path,
                bytes: file.bytes,
                sha256: file.sha256,
            })
            .collect(),
    };
    let marker = serde_json::to_string(&installed).map_err(|error| error.to_string())?;
    write_atomic_text(&staging.join(MLX_INSTALLED_MANIFEST), &marker)?;
    if handle.cancel.is_cancelled() || !manager.is_current(handle) {
        return Err("download cancelled".into());
    }
    replace_model_dir(&staging, &model_dir(models_root, &handle.id))?;
    Ok(())
}

async fn write_mlx_file_stream<S, B, E>(
    mut stream: S,
    temporary: &Path,
    expected_bytes: u64,
    expected_sha256: &str,
    downloaded_bytes: Arc<AtomicU64>,
    base: u64,
    cancel: &CancellationToken,
) -> Result<(), String>
where
    S: futures_util::Stream<Item = Result<B, E>> + Unpin,
    B: AsRef<[u8]> + Send + 'static,
    E: std::fmt::Display,
{
    let writer_path = temporary.to_path_buf();
    let mut writer = tokio::task::spawn_blocking(move || DownloadWriter::create(&writer_path))
        .await
        .map_err(|error| format!("model writer task failed: {error}"))??;
    let write_result = async {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return Err("download cancelled".into()),
                item = stream.next() => match item {
                    None => break,
                    Some(Err(error)) => return Err(sanitize_download_error(&error.to_string())),
                    Some(Ok(chunk)) => {
                        let bytes = chunk.as_ref().to_vec();
                        let downloaded_bytes = Arc::clone(&downloaded_bytes);
                        writer = tokio::task::spawn_blocking(move || {
                            writer.write_chunk(
                                &bytes,
                                expected_bytes,
                                Some((downloaded_bytes, base)),
                                "model file exceeds pinned size",
                            )?;
                            Ok::<_, String>(writer)
                        })
                        .await
                        .map_err(|error| format!("model writer task failed: {error}"))??;
                    }
                }
            }
        }
        let expected_sha256 = expected_sha256.to_owned();
        tokio::task::spawn_blocking(move || {
            writer.finish(
                expected_bytes,
                &expected_sha256,
                "model file size or checksum mismatch",
                "model file size or checksum mismatch",
            )
        })
        .await
        .map_err(|error| format!("model writer task failed: {error}"))?
    }
    .await;
    if let Err(error) = write_result {
        remove_path(temporary);
        return Err(error);
    }
    Ok(())
}

fn validate_manifest_path(model_id: &str, path: &str) -> Result<(), String> {
    let candidate = Path::new(path);
    if candidate.is_absolute()
        || !candidate
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err("model manifest path is not allowlisted".into());
    }
    if candidate.components().count() == 1 {
        return Ok(());
    }
    if model_id == crate::ondevice_models::FUN_ASR_NANO_2512_ID
        && matches!(
            path,
            "Qwen3-0.6B/config.json"
                | "Qwen3-0.6B/generation_config.json"
                | "Qwen3-0.6B/merges.txt"
                | "Qwen3-0.6B/tokenizer.json"
                | "Qwen3-0.6B/tokenizer_config.json"
                | "Qwen3-0.6B/vocab.json"
        )
    {
        return Ok(());
    }
    Err("model manifest path is not allowlisted".into())
}

fn verify_pinned_file(
    path: &Path,
    expected_bytes: u64,
    expected_sha256: &str,
) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != expected_bytes
    {
        return Ok(false);
    }
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read =
            std::io::Read::read(&mut file, &mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()).eq_ignore_ascii_case(expected_sha256))
}

pub async fn delete_and_unload(
    models_root: &Path,
    manager: &DownloadManager,
    id: &str,
) -> Result<(), String> {
    let id = validate_model_id(id)?;
    if is_mlx_model(id) {
        crate::ondevice_runtime::shared_runtime(models_root)
            .unload(Some(id))
            .await?;
    }
    let generation = manager.start_delete(id)?;
    let result = async {
        if let Some(generation) = generation {
            manager.wait_until_stopped(id, generation).await?;
        }
        if manager.should_delete_files(id, generation) {
            manager.clear_last_error(id);
        }
        manager.delete_files_if_safe(models_root, id, generation)
    }
    .await;
    manager.finish_delete();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "voiceflow-ondevice-dl-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn oversized_mlx_file_stream_cleans_temporary_after_writer_failure() {
        let root = temp_root("mlx-file-oversized-cleanup");
        let temporary = root.join(".tiny-model.bin.download");
        let staged = root.join("tiny-model.bin");
        let downloaded_bytes = Arc::new(AtomicU64::new(0));
        let result = write_mlx_file_stream(
            stream::iter([Ok::<_, std::io::Error>(b"too long".to_vec())]),
            &temporary,
            4,
            "00000000",
            downloaded_bytes,
            0,
            &CancellationToken::new(),
        )
        .await;

        let temporary_exists = temporary.exists();
        let staged_exists = staged.exists();
        let committed_marker_exists = root.join(MLX_INSTALLED_MANIFEST).exists();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(
            result.expect_err("oversized MLX file stream must fail"),
            "model file exceeds pinned size"
        );
        assert!(
            !temporary_exists,
            "the .download temporary must be removed after the writer worker settles"
        );
        assert!(
            !staged_exists,
            "failed file must not be renamed into staging"
        );
        assert!(
            !committed_marker_exists,
            "failed file must not publish the installed manifest"
        );
    }

    #[test]
    fn download_error_strips_url_query() {
        let raw = "failed https://example.com/model.tar.bz2?token=secret after reset";
        let clean = sanitize_download_error(raw);
        assert!(!clean.contains("token=secret"));
        assert!(!clean.contains('?'));
        assert!(clean.contains("https://example.com/model.tar.bz2"));
    }

    #[test]
    fn finish_ignores_generation_mismatch() {
        let root = temp_root("gen-mismatch");
        let manager = DownloadManager::new();
        let first = manager.begin(SENSEVOICE_ID).unwrap();
        manager.finish(
            SENSEVOICE_ID,
            first.generation.wrapping_add(1),
            Some("stale".into()),
        );
        assert_eq!(
            catalog_status(&root, &manager).state,
            OnDeviceModelState::Downloading,
            "wrong generation must leave the live job in place"
        );
        assert!(manager.last_error(SENSEVOICE_ID).is_none());

        manager.finish(SENSEVOICE_ID, first.generation, None);
        assert_ne!(
            catalog_status(&root, &manager).state,
            OnDeviceModelState::Downloading
        );

        let first = manager.begin(SENSEVOICE_ID).unwrap();
        manager.finish(SENSEVOICE_ID, first.generation, None);
        let second = manager.begin(SENSEVOICE_ID).unwrap();
        manager.finish(SENSEVOICE_ID, first.generation, Some("old-worker".into()));
        assert_eq!(
            catalog_status(&root, &manager).state,
            OnDeviceModelState::Downloading,
            "old worker finish must not clear a newer job"
        );
        assert!(manager.last_error(SENSEVOICE_ID).is_none());
        manager.finish(SENSEVOICE_ID, second.generation, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn replace_model_dir_does_not_delete_dest_first() {
        let root = temp_root("replace-keep");
        let dest = root.join(SENSEVOICE_ID);
        let src = partial_path(&root, SENSEVOICE_ID);
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("old.txt"), b"keep-me").unwrap();

        replace_model_dir(&src, &dest).expect_err("missing extract must fail");
        assert!(
            dest.join("old.txt").is_file(),
            "re-download must not remove_dir_all the live Ready dir before replace"
        );

        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("new.txt"), b"new").unwrap();
        replace_model_dir(&src, &dest).unwrap();
        assert_eq!(fs::read(dest.join("new.txt")).unwrap(), b"new");
        assert!(!dest.join("old.txt").exists());
        assert!(!src.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn duplicate_download_rejected() {
        let manager = DownloadManager::new();
        manager.begin(SENSEVOICE_ID).unwrap();
        let err = match manager.begin(SENSEVOICE_ID) {
            Ok(_) => panic!("second begin must fail"),
            Err(error) => error,
        };
        assert!(err.contains("progress"), "{err}");
        assert!(manager.begin("../x").is_err());
    }

    #[test]
    fn begin_rejected_while_deleting() {
        let manager = DownloadManager::new();
        manager.start_delete(SENSEVOICE_ID).unwrap();
        let err = match manager.begin(SENSEVOICE_ID) {
            Ok(_) => panic!("begin must not start during delete"),
            Err(error) => error,
        };
        assert!(
            err.contains("delet"),
            "begin must fail because delete still holds the manager: {err}"
        );
        manager.finish_delete();
        manager.begin(SENSEVOICE_ID).unwrap();
    }

    #[tokio::test]
    async fn stale_delete_does_not_wipe_newer_job_files() {
        let root = temp_root("stale-delete");
        let manager = DownloadManager::new();
        let first = manager.begin(SENSEVOICE_ID).unwrap();
        let gen1 = first.generation;
        manager.finish(SENSEVOICE_ID, gen1, None);

        let second = manager.begin(SENSEVOICE_ID).unwrap();
        let dest = model_dir(&root, SENSEVOICE_ID);
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("new-worker.bin"), b"live").unwrap();
        fs::write(partial_path(&root, SENSEVOICE_ID), b"partial").unwrap();

        assert!(
            !manager.should_delete_files(SENSEVOICE_ID, Some(gen1)),
            "a newer live job must block a stale delete"
        );
        manager
            .delete_files_if_safe(&root, SENSEVOICE_ID, Some(gen1))
            .unwrap();
        assert_eq!(
            fs::read(dest.join("new-worker.bin")).unwrap(),
            b"live",
            "stale delete must not wipe files the new worker is writing"
        );
        assert!(
            partial_path(&root, SENSEVOICE_ID).exists(),
            "stale delete must not remove the newer job partial"
        );

        manager.finish(SENSEVOICE_ID, second.generation, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn downloads_reject_legacy_models_before_starting_a_job() {
        let root = temp_root("retired-downloads");
        let manager = DownloadManager::new();
        for (id, expected) in [
            (SENSEVOICE_ID, "no longer available for download"),
            (
                crate::ondevice_models::COHERE_TRANSCRIBE_ID,
                "not available for new downloads",
            ),
        ] {
            let error = start_background_download(root.clone(), manager.clone(), id.into(), |_| {})
                .expect_err("retired model download must be rejected");
            assert!(error.contains(expected), "{id}: {error}");
            assert!(manager.begin(id).is_ok_and(|job| {
                manager.finish(id, job.generation, None);
                true
            }));
        }
        assert!(!partial_path(&root, SENSEVOICE_ID).exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn disk_full_is_classified() {
        assert_eq!(
            download_error_category("write failed: No space left on device"),
            Some("disk_full")
        );
        assert_eq!(download_error_category("request timed out"), None);
    }

    #[test]
    fn enospc_manager_failure_is_exposed_as_disk_full_status() {
        let manager = DownloadManager::default();
        let job = manager.begin(SENSEVOICE_ID).unwrap();
        let error = std::io::Error::from_raw_os_error(28).to_string();
        manager.finish(SENSEVOICE_ID, job.generation, Some(error.clone()));

        let status = status_from_info(
            Path::new("/unused-test-model-root"),
            &manager,
            crate::ondevice_asr::OnDeviceModelInfo {
                id: SENSEVOICE_ID.to_owned(),
                label: SENSEVOICE_LABEL.to_owned(),
                state: OnDeviceModelState::Missing,
                inference_ready: false,
                revision: None,
                bytes: 0,
                file_count: 0,
                platform_supported: true,
                download_available: false,
                language_note: String::new(),
            },
            None,
        );
        let serialized = serde_json::to_value(status).unwrap();
        assert_eq!(serialized["error"], error);
        assert_eq!(serialized["error_category"], "disk_full");
    }
}
