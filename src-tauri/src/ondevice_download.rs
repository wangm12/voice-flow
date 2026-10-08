//! On-device model download, hash verify, and safe extract. No ORT / infer.

use crate::ondevice_asr::{
    archive_sha_path, delete_model, list_models, model_dir, validate_model_id, OnDeviceModelState,
    OnDeviceModelStatus,
};
use crate::ondevice_models::{
    inner_file_name, is_mlx_model, mlx_model, SENSEVOICE_ARCHIVE_BYTES, SENSEVOICE_ARCHIVE_SHA256,
    SENSEVOICE_ID, SENSEVOICE_INNER_ONNX, SENSEVOICE_INNER_TOKENS, SENSEVOICE_URL,
};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tar::EntryType;
use tokio_util::sync::CancellationToken;

pub const DOWNLOAD_EVENT: &str = "ondevice://download";
pub const SENSEVOICE_LABEL: &str = "SenseVoice Small";
const MAX_EXTRACTED_BYTES: u64 = 512 * 1024 * 1024;
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
        error: if downloading {
            None
        } else {
            manager
                .last_error(&info.id)
                .or_else(|| runtime.and_then(|runtime| runtime.error.clone()))
        },
        revision: info.revision,
        file_count: info.file_count,
        platform_supported: info.platform_supported,
        runtime_status: runtime_status.to_owned(),
        loaded,
    }
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

pub fn extracting_path(models_root: &Path, id: &str) -> PathBuf {
    models_root.join(format!("{id}.extracting"))
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
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

fn archive_root_name() -> Option<&'static str> {
    Path::new(SENSEVOICE_INNER_ONNX)
        .components()
        .next()
        .and_then(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
}

fn flatten_tar_path(path: &Path) -> Result<Option<PathBuf>, String> {
    if path.is_absolute() {
        return Err("tar path must be relative".into());
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_owned()),
            Component::CurDir => {}
            _ => return Err("tar path is not allowed".into()),
        }
    }
    if parts.is_empty() {
        return Ok(None);
    }
    if archive_root_name().is_some_and(|root| parts[0] == root) {
        parts.remove(0);
    }
    if parts.is_empty() {
        return Ok(None);
    }
    Ok(Some(parts.into_iter().collect()))
}

pub fn safe_extract_tar_bz2(archive: &Path, dest: &Path) -> Result<(), String> {
    let archive_len = fs::metadata(archive)
        .map_err(|error| error.to_string())?
        .len();
    if archive_len > SENSEVOICE_ARCHIVE_BYTES {
        return Err("archive exceeds pinned size".into());
    }
    if dest.exists() {
        fs::remove_dir_all(dest).map_err(|error| error.to_string())?;
    }
    ensure_private_dir(dest)?;
    let file = File::open(archive).map_err(|error| error.to_string())?;
    let decoder = bzip2::read::BzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    tar.set_overwrite(false);
    let mut extracted = 0u64;
    for entry in tar.entries().map_err(|error| error.to_string())? {
        let mut entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.header().entry_type();
        if matches!(
            kind,
            EntryType::Symlink | EntryType::Link | EntryType::XGlobalHeader | EntryType::XHeader
        ) || kind.is_symlink()
            || kind.is_hard_link()
        {
            return Err("tar links are not allowed".into());
        }
        if !kind.is_file() && !kind.is_dir() {
            return Err("unsupported tar entry".into());
        }
        let entry_path = entry.path().map_err(|error| error.to_string())?;
        let Some(relative) = flatten_tar_path(&entry_path)? else {
            continue;
        };
        let dest_path = dest.join(&relative);
        if !dest_path.starts_with(dest) {
            return Err("tar path escaped destination".into());
        }
        if kind.is_dir() {
            ensure_private_dir(&dest_path)?;
            continue;
        }
        let declared = entry.header().size().map_err(|error| error.to_string())?;
        if extracted.saturating_add(declared) > MAX_EXTRACTED_BYTES {
            return Err("extracted archive is too large".into());
        }
        if let Some(parent) = dest_path.parent() {
            ensure_private_dir(parent)?;
        }
        let mut out = File::create(&dest_path).map_err(|error| error.to_string())?;
        restrict_file_mode(&dest_path)?;
        let copied = std::io::copy(&mut entry, &mut out).map_err(|error| error.to_string())?;
        out.sync_all().map_err(|error| error.to_string())?;
        if copied != declared {
            return Err("tar file size mismatch".into());
        }
        extracted += copied;
    }
    verify_inner_files(dest)
}

fn verify_inner_files(dir: &Path) -> Result<(), String> {
    let onnx = dir.join(inner_file_name(SENSEVOICE_INNER_ONNX));
    let tokens = dir.join(inner_file_name(SENSEVOICE_INNER_TOKENS));
    let onnx_ok = fs::metadata(&onnx)
        .ok()
        .is_some_and(|meta| meta.is_file() && meta.len() > 0);
    let tokens_ok = fs::metadata(&tokens)
        .ok()
        .is_some_and(|meta| meta.is_file() && meta.len() > 0);
    if onnx_ok && tokens_ok {
        Ok(())
    } else {
        Err("archive missing model files".into())
    }
}

fn cleanup_temps(models_root: &Path, id: &str) {
    remove_path(&partial_path(models_root, id));
    remove_path(&extracting_path(models_root, id));
}

#[cfg(test)]
pub async fn install_from_stream<S, B, E>(
    manager: &DownloadManager,
    models_root: &Path,
    id: &str,
    stream: S,
    expected_sha: &str,
    expected_bytes: u64,
) -> Result<(), String>
where
    S: futures_util::Stream<Item = Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let handle = manager.begin(id)?;
    let result = consume_stream(
        manager,
        &handle,
        models_root,
        stream,
        expected_sha,
        expected_bytes,
    )
    .await;
    manager.finish(id, handle.generation, result.as_ref().err().cloned());
    result
}

async fn consume_stream<S, B, E>(
    manager: &DownloadManager,
    handle: &JobHandle,
    models_root: &Path,
    mut stream: S,
    expected_sha: &str,
    expected_bytes: u64,
) -> Result<(), String>
where
    S: futures_util::Stream<Item = Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    if expected_bytes == 0 {
        return Err("invalid archive size".into());
    }
    ensure_private_dir(models_root)?;
    cleanup_temps(models_root, &handle.id);
    let partial = partial_path(models_root, &handle.id);
    let extracting = extracting_path(models_root, &handle.id);
    let fail = |message: String| {
        cleanup_temps(models_root, &handle.id);
        Err(sanitize_download_error(&message))
    };

    let write_result = async {
        let mut file = File::create(&partial).map_err(|error| error.to_string())?;
        restrict_file_mode(&partial)?;
        let mut hasher = Sha256::new();
        let mut written = 0u64;
        loop {
            tokio::select! {
                _ = handle.cancel.cancelled() => {
                    return Err("download cancelled".into());
                }
                item = stream.next() => {
                    match item {
                        None => break,
                        Some(Err(error)) => {
                            return Err(sanitize_download_error(&error.to_string()));
                        }
                        Some(Ok(chunk)) => {
                            let chunk = chunk.as_ref();
                            if chunk.is_empty() {
                                continue;
                            }
                            written = written
                                .checked_add(chunk.len() as u64)
                                .ok_or_else(|| "archive too large".to_string())?;
                            if written > expected_bytes {
                                return Err("archive exceeds pinned size".into());
                            }
                            file.write_all(chunk).map_err(|error| error.to_string())?;
                            hasher.update(chunk);
                            handle.downloaded_bytes.store(written, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
        if handle.cancel.is_cancelled() {
            return Err("download cancelled".into());
        }
        if written != expected_bytes {
            return Err("archive size mismatch".into());
        }
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        let digest = hex::encode(hasher.finalize());
        if !digest.eq_ignore_ascii_case(expected_sha) {
            return Err("archive hash mismatch".into());
        }
        Ok(())
    }
    .await;
    if let Err(error) = write_result {
        return fail(error);
    }

    if handle.cancel.is_cancelled() || !manager.is_current(handle) {
        return fail("download cancelled".into());
    }
    if let Err(error) = safe_extract_tar_bz2(&partial, &extracting) {
        return fail(error);
    }
    let dest = model_dir(models_root, &handle.id);
    if let Err(error) = commit_extracted_model(
        manager,
        handle,
        &extracting,
        &dest,
        expected_sha,
        models_root,
    ) {
        return fail(error);
    }
    remove_path(&partial);
    Ok(())
}

fn commit_extracted_model(
    manager: &DownloadManager,
    handle: &JobHandle,
    extracting: &Path,
    dest: &Path,
    expected_sha: &str,
    models_root: &Path,
) -> Result<(), String> {
    if handle.cancel.is_cancelled() || !manager.is_current(handle) {
        return Err("download cancelled".into());
    }
    replace_model_dir(extracting, dest)?;
    if let Err(error) = write_atomic_text(
        &archive_sha_path(models_root, &handle.id),
        &expected_sha.to_ascii_lowercase(),
    ) {
        remove_path(dest);
        return Err(error);
    }
    Ok(())
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
    if matches!(
        crate::ondevice_asr::model_state(&models_root, &id),
        OnDeviceModelState::Ready
    ) {
        return Err("model is already downloaded".into());
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
        let result = if id == SENSEVOICE_ID {
            download_official_archive(&handle, &manager, &models_root).await
        } else {
            download_mlx_model(&handle, &manager, &models_root).await
        };
        manager.finish(&id, handle.generation, result.err());
        stop.cancel();
        if let Some(status) = status_for(&models_root, &manager, &id) {
            emit(status);
        }
    });
    Ok(())
}

async fn download_official_archive(
    handle: &JobHandle,
    manager: &DownloadManager,
    models_root: &Path,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30 * 60))
        .user_agent("VoiceFlow/0.1")
        .https_only(true)
        .build()
        .map_err(|error| sanitize_download_error(&error.to_string()))?;
    let response = tokio::select! {
        _ = handle.cancel.cancelled() => {
            return Err("download cancelled".into());
        }
        response = client.get(SENSEVOICE_URL).send() => {
            response.map_err(|error| sanitize_download_error(&error.to_string()))?
        }
    };
    if !response.status().is_success() {
        return Err(format!("download failed ({})", response.status().as_u16()));
    }
    let stream = response
        .bytes_stream()
        .map(|chunk| chunk.map_err(|error| sanitize_download_error(&error.to_string())));
    consume_stream(
        manager,
        handle,
        models_root,
        stream,
        SENSEVOICE_ARCHIVE_SHA256,
        SENSEVOICE_ARCHIVE_BYTES,
    )
    .await
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
    ensure_private_dir(&staging)?;

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
        validate_manifest_path(file.path)?;
        let staged_file = staging.join(file.path);
        if verify_pinned_file(&staged_file, file.bytes, file.sha256)? {
            completed_bytes = completed_bytes.saturating_add(file.bytes);
            handle
                .downloaded_bytes
                .store(completed_bytes, Ordering::Relaxed);
            continue;
        }
        remove_path(&staged_file);
        let temporary = staging.join(format!(".{}.download", file.path));
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
        let mut stream = response.bytes_stream();
        let mut output = File::create(&temporary).map_err(|error| error.to_string())?;
        restrict_file_mode(&temporary)?;
        let mut hasher = Sha256::new();
        let mut written = 0u64;
        loop {
            tokio::select! {
                _ = handle.cancel.cancelled() => {
                    remove_path(&temporary);
                    return Err("download cancelled".into());
                }
                item = stream.next() => {
                    match item {
                        None => break,
                        Some(Err(error)) => {
                            remove_path(&temporary);
                            return Err(sanitize_download_error(&error.to_string()));
                        }
                        Some(Ok(chunk)) => {
                            written = written
                                .checked_add(chunk.len() as u64)
                                .ok_or_else(|| "model file exceeds pinned size".to_owned())?;
                            if written > file.bytes {
                                remove_path(&temporary);
                                return Err("model file exceeds pinned size".into());
                            }
                            output.write_all(&chunk).map_err(|error| error.to_string())?;
                            hasher.update(&chunk);
                            handle.downloaded_bytes.store(
                                completed_bytes.saturating_add(written),
                                Ordering::Relaxed,
                            );
                        }
                    }
                }
            }
        }
        if written != file.bytes
            || !hex::encode(hasher.finalize()).eq_ignore_ascii_case(file.sha256)
        {
            remove_path(&temporary);
            return Err("model file size or checksum mismatch".into());
        }
        output.sync_all().map_err(|error| error.to_string())?;
        drop(output);
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

fn validate_manifest_path(path: &str) -> Result<(), String> {
    let candidate = Path::new(path);
    if candidate.is_absolute()
        || candidate.components().count() != 1
        || !candidate
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err("model manifest path is not allowlisted".into());
    }
    Ok(())
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
    use crate::ondevice_asr::{
        model_files_are_ready, model_state, DEFAULT_MODEL_ID, MODEL_TOKENS_FILE, MODEL_WEIGHTS_FILE,
    };
    use futures_util::stream;
    use std::io::Write;
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

    fn pack_tar_bz2(entries: &[(&str, Option<&[u8]>, EntryType)]) -> Vec<u8> {
        let mut tar_buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buf);
            for (path, data, kind) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(*kind);
                match kind {
                    EntryType::Symlink | EntryType::Link => {
                        header.set_size(0);
                        header.set_cksum();
                        let target = std::str::from_utf8(data.unwrap_or(b"/tmp/evil")).unwrap();
                        builder.append_link(&mut header, path, target).unwrap();
                    }
                    EntryType::Directory => {
                        header.set_size(0);
                        header.set_cksum();
                        builder.append_data(&mut header, path, &[][..]).unwrap();
                    }
                    _ => {
                        let bytes = data.unwrap_or(b"");
                        header.set_size(bytes.len() as u64);
                        header.set_cksum();
                        builder.append_data(&mut header, path, bytes).unwrap();
                    }
                }
            }
            builder.finish().unwrap();
        }
        let mut encoded = Vec::new();
        {
            let mut encoder =
                bzip2::write::BzEncoder::new(&mut encoded, bzip2::Compression::default());
            encoder.write_all(&tar_buf).unwrap();
            encoder.finish().unwrap();
        }
        encoded
    }

    fn tiny_ready_archive() -> (Vec<u8>, String) {
        let bytes = pack_tar_bz2(&[
            (
                "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17/",
                None,
                EntryType::Directory,
            ),
            (
                SENSEVOICE_INNER_ONNX,
                Some(b"tiny-onnx"),
                EntryType::Regular,
            ),
            (
                SENSEVOICE_INNER_TOKENS,
                Some(b"tiny-tokens"),
                EntryType::Regular,
            ),
            (
                "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17/LICENSE",
                Some(b"license"),
                EntryType::Regular,
            ),
        ]);
        let sha = hex::encode(Sha256::digest(&bytes));
        (bytes, sha)
    }

    fn pack_tar_bz2_with_raw_name(name: &[u8], data: &[u8]) -> Vec<u8> {
        let mut tar_buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buf);
            let mut header = tar::Header::new_gnu();
            {
                let old = header.as_old_mut();
                old.name = [0; 100];
                old.name[..name.len()].copy_from_slice(name);
            }
            header.set_size(data.len() as u64);
            header.set_entry_type(EntryType::Regular);
            header.set_cksum();
            builder.append(&header, data).unwrap();
            builder.finish().unwrap();
        }
        let mut encoded = Vec::new();
        {
            let mut encoder =
                bzip2::write::BzEncoder::new(&mut encoded, bzip2::Compression::default());
            encoder.write_all(&tar_buf).unwrap();
            encoder.finish().unwrap();
        }
        encoded
    }

    #[test]
    fn tar_slip_is_rejected() {
        assert!(flatten_tar_path(Path::new("../evil.txt")).is_err());
        assert!(flatten_tar_path(Path::new("/tmp/evil.txt")).is_err());
        assert!(flatten_tar_path(Path::new("foo/../../etc/passwd")).is_err());

        let root = temp_root("slip");
        let dest = root.join("dest");
        let archive = root.join("slip.tar.bz2");
        fs::write(
            &archive,
            pack_tar_bz2_with_raw_name(b"../evil.txt", b"nope"),
        )
        .unwrap();
        let err = safe_extract_tar_bz2(&archive, &dest).expect_err("slip must fail");
        assert!(
            err.contains("not allowed") || err.contains("relative") || err.contains("missing"),
            "{err}"
        );
        assert!(!root.join("evil.txt").exists());
        assert!(!dest.join("evil.txt").exists());

        fs::write(
            &archive,
            pack_tar_bz2_with_raw_name(b"/tmp/voiceflow-tar-slip", b"nope"),
        )
        .unwrap();
        assert!(safe_extract_tar_bz2(&archive, &dest).is_err());
        assert!(!dest.join("tmp").exists());
        assert!(!dest.join("voiceflow-tar-slip").exists());

        let link = pack_tar_bz2(&[(
            "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17/model.int8.onnx",
            Some(b"/etc/passwd"),
            EntryType::Symlink,
        )]);
        fs::write(&archive, link).unwrap();
        let err = safe_extract_tar_bz2(&archive, &dest).expect_err("symlink must fail");
        assert!(err.contains("link") || err.contains("not allowed"), "{err}");

        let hard = pack_tar_bz2(&[(
            "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17/model.int8.onnx",
            Some(b"tokens.txt"),
            EntryType::Link,
        )]);
        fs::write(&archive, hard).unwrap();
        let err = safe_extract_tar_bz2(&archive, &dest).expect_err("hardlink must fail");
        assert!(err.contains("link") || err.contains("not allowed"), "{err}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extract_flattens_official_inner_paths() {
        let root = temp_root("flatten");
        let (bytes, _) = tiny_ready_archive();
        let archive = root.join("tiny.tar.bz2");
        let dest = root.join("dest");
        fs::write(&archive, bytes).unwrap();
        safe_extract_tar_bz2(&archive, &dest).unwrap();
        assert_eq!(
            fs::read(dest.join(MODEL_WEIGHTS_FILE)).unwrap(),
            b"tiny-onnx"
        );
        assert_eq!(
            fs::read(dest.join(MODEL_TOKENS_FILE)).unwrap(),
            b"tiny-tokens"
        );
        assert!(dest.join("LICENSE").is_file());
        assert!(!dest
            .join("sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17")
            .exists());
        let _ = fs::remove_dir_all(root);
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
        let src = extracting_path(&root, SENSEVOICE_ID);
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

    #[tokio::test]
    async fn cancel_transitions() {
        let root = temp_root("cancel");
        let manager = DownloadManager::new();
        let install = {
            let manager = manager.clone();
            let root = root.clone();
            tokio::spawn(async move {
                let stream =
                    stream::iter([Ok::<_, String>(vec![7u8; 32])]).chain(stream::pending());
                install_from_stream(
                    &manager,
                    &root,
                    SENSEVOICE_ID,
                    stream,
                    &"ab".repeat(32),
                    1_000_000,
                )
                .await
            })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if manager.downloaded_bytes(SENSEVOICE_ID) > 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("download should accept the first chunk");
        assert_eq!(
            catalog_status(&root, &manager).state,
            OnDeviceModelState::Downloading
        );
        manager.cancel(SENSEVOICE_ID).unwrap();
        let err = install
            .await
            .unwrap()
            .expect_err("cancel must fail install");
        assert!(err.to_lowercase().contains("cancel"), "{err}");
        assert!(!model_files_are_ready(&root, SENSEVOICE_ID));
        assert_eq!(
            model_state(&root, SENSEVOICE_ID),
            OnDeviceModelState::Missing
        );
        assert!(!partial_path(&root, SENSEVOICE_ID).exists());
        assert!(!extracting_path(&root, SENSEVOICE_ID).exists());
        assert!(!model_dir(&root, SENSEVOICE_ID).exists());
        let _ = fs::remove_dir_all(root);
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
    fn cancel_immediately_before_replace_does_not_install() {
        let root = temp_root("cancel-before-replace");
        let manager = DownloadManager::new();
        let handle = manager.begin(SENSEVOICE_ID).unwrap();
        let dest = model_dir(&root, SENSEVOICE_ID);
        let extracting = extracting_path(&root, SENSEVOICE_ID);
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("old.txt"), b"keep").unwrap();
        fs::create_dir_all(&extracting).unwrap();
        fs::write(extracting.join(MODEL_WEIGHTS_FILE), b"new-onnx").unwrap();
        fs::write(extracting.join(MODEL_TOKENS_FILE), b"new-tokens").unwrap();

        manager.cancel(SENSEVOICE_ID).unwrap();
        let err = commit_extracted_model(&manager, &handle, &extracting, &dest, "deadbeef", &root)
            .expect_err("cancelled job must not install");
        assert!(err.to_lowercase().contains("cancel"), "{err}");
        assert_eq!(
            fs::read(dest.join("old.txt")).unwrap(),
            b"keep",
            "cancel after the last extract guard must not replace dest"
        );
        assert!(!dest.join(MODEL_WEIGHTS_FILE).exists());
        assert!(!model_files_are_ready(&root, SENSEVOICE_ID));
        manager.finish(SENSEVOICE_ID, handle.generation, Some(err));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn install_from_bytes_reaches_files_without_network() {
        let root = temp_root("install");
        let manager = DownloadManager::new();
        let (bytes, sha) = tiny_ready_archive();
        let expected_bytes = bytes.len() as u64;
        let stream = stream::iter([Ok::<_, String>(bytes)]);
        install_from_stream(&manager, &root, SENSEVOICE_ID, stream, &sha, expected_bytes)
            .await
            .unwrap();
        assert!(model_dir(&root, SENSEVOICE_ID)
            .join(MODEL_WEIGHTS_FILE)
            .is_file());
        assert!(model_dir(&root, SENSEVOICE_ID)
            .join(MODEL_TOKENS_FILE)
            .is_file());
        assert_eq!(
            fs::read_to_string(archive_sha_path(&root, SENSEVOICE_ID))
                .unwrap()
                .trim(),
            sha
        );
        assert!(!partial_path(&root, SENSEVOICE_ID).exists());
        assert_eq!(DEFAULT_MODEL_ID, SENSEVOICE_ID);
        let _ = fs::remove_dir_all(root);
    }
}
