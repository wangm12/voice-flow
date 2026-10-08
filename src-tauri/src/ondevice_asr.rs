use crate::asr::{AsrCapabilities, AsrError, AsrFuture, AsrOptions, AsrProvider};
use crate::ondevice_models::{
    inner_file_name, is_mlx_model, mlx_model, MlxLanguagePolicy, MLX_MODELS, SENSEVOICE_ID,
    SENSEVOICE_INNER_ONNX, SENSEVOICE_INNER_TOKENS,
};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const DEFAULT_MODEL_ID: &str = SENSEVOICE_ID;
#[cfg(test)]
pub const MODEL_WEIGHTS_FILE: &str = "model.int8.onnx";
#[cfg(test)]
pub const MODEL_TOKENS_FILE: &str = "tokens.txt";
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OnDeviceModelState {
    Missing,
    Downloading,
    Ready,
    Corrupt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OnDeviceModelInfo {
    pub id: String,
    pub label: String,
    pub state: OnDeviceModelState,
    /// True only after a usable runtime is present on this host.
    pub inference_ready: bool,
    pub revision: Option<String>,
    pub bytes: u64,
    pub file_count: usize,
    pub platform_supported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OnDeviceModelStatus {
    pub id: String,
    pub label: String,
    pub bytes: u64,
    pub sha256: Option<String>,
    pub state: OnDeviceModelState,
    pub inference_ready: bool,
    pub downloaded_bytes: u64,
    pub error: Option<String>,
    pub revision: Option<String>,
    pub file_count: usize,
    pub platform_supported: bool,
    pub runtime_status: String,
    pub loaded: bool,
}

#[derive(Debug, Clone)]
pub struct OnDeviceAsrProvider {
    models_root: PathBuf,
    asr_model: String,
}

impl OnDeviceAsrProvider {
    pub fn new(models_root: PathBuf, asr_model: impl Into<String>) -> Self {
        Self {
            models_root,
            asr_model: asr_model.into(),
        }
    }
}

impl AsrProvider for OnDeviceAsrProvider {
    fn transcribe_batch(&self, audio: Vec<u8>, options: AsrOptions) -> AsrFuture {
        let models_root = self.models_root.clone();
        let asr_model = self.asr_model.clone();
        Box::pin(async move {
            if !model_files_are_ready(&models_root, &asr_model) {
                return Err(AsrError::OnDeviceModelMissing(
                    "on-device model files are not downloaded".into(),
                ));
            }
            let language = options
                .language
                .as_deref()
                .filter(|language| *language != "auto");
            crate::ondevice_runtime::shared_runtime(&models_root)
                .transcribe(&asr_model, language, audio)
                .await
                .map(|(text, language)| crate::asr::Transcript {
                    text,
                    asr_text: None,
                    provider_cleaned_candidate: None,
                    language,
                    confidence: None,
                    segments: Vec::new(),
                    words: Vec::new(),
                    tokens: Vec::new(),
                    limits: crate::asr::RateLimits::default(),
                })
                .map_err(AsrError::OnDeviceInferenceUnavailable)
        })
    }

    fn capabilities(&self) -> AsrCapabilities {
        AsrCapabilities {
            batch_transcription: true,
            background_prefetch: false,
            realtime_streaming: false,
            streaming_partial_results: false,
            streaming_final_results: false,
            max_audio_duration_secs: Some(15 * 60),
            max_audio_file_bytes: Some(30_000_000),
            cancellation: true,
            protocol: crate::asr::AsrProtocol::OnDevice,
            audio_input: crate::asr::AsrAudioInput::LocalSamples,
            response_format: crate::asr::AsrResponseFormat::NativeTranscript,
            language_support: if mlx_model(&self.asr_model)
                .is_some_and(|model| model.language_policy == MlxLanguagePolicy::AutoOrExplicit)
            {
                crate::asr::AsrLanguageSupport::AutoDetectOrFixedLanguage
            } else if is_mlx_model(&self.asr_model) {
                crate::asr::AsrLanguageSupport::OptionalFixedLanguage
            } else {
                crate::asr::AsrLanguageSupport::AutoDetectOnly
            },
            context_support: crate::asr::AsrContextSupport::None,
            confidence_support: crate::asr::AsrConfidenceSupport::None,
            segment_timestamps: false,
            word_timestamps: false,
            retry_429: false,
            retry_5xx: false,
            honors_retry_after: false,
        }
    }
}

const REJECTED_MODEL_ID: &str = ".rejected-model-id";

fn allowlisted_model_id(model: &str) -> Option<&str> {
    if model == SENSEVOICE_ID || is_mlx_model(model) {
        Some(model)
    } else {
        None
    }
}

fn safe_model_id(model: &str) -> &str {
    allowlisted_model_id(model).unwrap_or(REJECTED_MODEL_ID)
}

pub fn model_dir(models_root: &Path, model: &str) -> PathBuf {
    models_root.join(safe_model_id(model))
}

pub fn archive_sha_path(models_root: &Path, model: &str) -> PathBuf {
    models_root.join(format!("{}.archive.sha256", safe_model_id(model)))
}

fn file_nonempty(path: &Path) -> bool {
    std::fs::metadata(path)
        .ok()
        .is_some_and(|meta| meta.is_file() && meta.len() > 0)
}

fn sidecar_sha_matches(models_root: &Path, model: &str) -> bool {
    let Ok(raw) = std::fs::read_to_string(archive_sha_path(models_root, model)) else {
        return false;
    };
    raw.trim()
        .eq_ignore_ascii_case(crate::ondevice_models::SENSEVOICE_ARCHIVE_SHA256)
}

pub fn model_files_are_ready(models_root: &Path, model: &str) -> bool {
    matches!(model_state(models_root, model), OnDeviceModelState::Ready)
}

pub fn inference_is_ready(models_root: &Path, model: &str) -> bool {
    if model == SENSEVOICE_ID || !model_files_are_ready(models_root, model) {
        return false;
    }
    let snapshot = crate::ondevice_runtime::shared_runtime(models_root).snapshot();
    snapshot.platform_supported
        && matches!(snapshot.runtime_status.as_str(), "runtime_ready" | "loaded")
}

/// File setup is enough to allow selecting a model; runtime handshake and load
/// state are separate capabilities shown in the model manager.
pub fn model_setup_is_ready(models_root: &Path, model: &str) -> bool {
    if model == SENSEVOICE_ID || !model_files_are_ready(models_root, model) {
        return false;
    }
    platform_supported() && crate::ondevice_runtime::sidecar_executable().is_some()
}

pub fn model_state(models_root: &Path, model: &str) -> OnDeviceModelState {
    if allowlisted_model_id(model).is_none() {
        return OnDeviceModelState::Missing;
    }
    if let Some(manifest) = mlx_model(model) {
        return mlx_model_state(models_root, manifest);
    }
    let dir = model_dir(models_root, model);
    let weights = dir.join(inner_file_name(SENSEVOICE_INNER_ONNX));
    let tokens = dir.join(inner_file_name(SENSEVOICE_INNER_TOKENS));
    let files_ok = file_nonempty(&weights) && file_nonempty(&tokens);
    let sha_ok = sidecar_sha_matches(models_root, model);
    if files_ok && sha_ok {
        return OnDeviceModelState::Ready;
    }
    if !dir.exists() && !archive_sha_path(models_root, model).exists() {
        OnDeviceModelState::Missing
    } else {
        OnDeviceModelState::Corrupt
    }
}

pub fn list_models(models_root: &Path) -> Vec<OnDeviceModelInfo> {
    let mut models = vec![OnDeviceModelInfo {
        id: DEFAULT_MODEL_ID.to_owned(),
        label: crate::ondevice_download::SENSEVOICE_LABEL.to_owned(),
        state: model_state(models_root, DEFAULT_MODEL_ID),
        inference_ready: false,
        revision: None,
        bytes: crate::ondevice_models::SENSEVOICE_ARCHIVE_BYTES,
        file_count: 2,
        platform_supported: false,
    }];
    models.extend(MLX_MODELS.iter().map(|manifest| OnDeviceModelInfo {
        id: manifest.id.to_owned(),
        label: manifest.label.to_owned(),
        state: model_state(models_root, manifest.id),
        inference_ready: inference_is_ready(models_root, manifest.id),
        revision: Some(manifest.revision.to_owned()),
        bytes: manifest.bytes,
        file_count: manifest.files.len(),
        platform_supported: platform_supported(),
    }));
    models
}

pub fn validate_model_id(model: &str) -> Result<&str, String> {
    if model != SENSEVOICE_ID && !is_mlx_model(model) {
        return Err("invalid on-device model id".into());
    }
    Ok(model)
}

pub fn platform_supported() -> bool {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        macos_major_version().is_some_and(|major| major >= 14)
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        false
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn macos_major_version() -> Option<u32> {
    let mut length = 0usize;
    let name = b"kern.osrelease\0";
    let first = unsafe {
        libc::sysctlbyname(
            name.as_ptr().cast(),
            std::ptr::null_mut(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if first != 0 || length == 0 || length > 128 {
        return None;
    }
    let mut bytes = vec![0u8; length];
    let second = unsafe {
        libc::sysctlbyname(
            name.as_ptr().cast(),
            bytes.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if second != 0 {
        return None;
    }
    let release = std::ffi::CStr::from_bytes_until_nul(&bytes)
        .ok()?
        .to_str()
        .ok()?;
    // Darwin major 23 is macOS 14, 24 is macOS 15, and so on.
    release
        .split('.')
        .next()?
        .parse::<u32>()
        .ok()
        .map(|darwin| darwin.saturating_sub(9))
}

const INSTALLED_MANIFEST: &str = ".voiceflow-model-manifest.json";

#[derive(serde::Deserialize)]
struct InstalledModelManifest {
    id: String,
    revision: String,
    files: Vec<InstalledModelFile>,
}

#[derive(serde::Deserialize)]
struct InstalledModelFile {
    path: String,
    bytes: u64,
    sha256: String,
}

fn mlx_model_state(
    models_root: &Path,
    manifest: &crate::ondevice_models::MlxModelManifest,
) -> OnDeviceModelState {
    let dir = model_dir(models_root, manifest.id);
    let dir_metadata = std::fs::symlink_metadata(&dir);
    if dir_metadata.is_err()
        && !dir
            .with_file_name(format!("{}.partial", manifest.id))
            .exists()
    {
        return OnDeviceModelState::Missing;
    }
    let Some(dir_metadata) = dir_metadata.ok() else {
        return OnDeviceModelState::Corrupt;
    };
    if dir_metadata.file_type().is_symlink() || !dir_metadata.is_dir() {
        return OnDeviceModelState::Corrupt;
    }
    let marker_path = dir.join(INSTALLED_MANIFEST);
    if std::fs::symlink_metadata(&marker_path)
        .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return OnDeviceModelState::Corrupt;
    }
    let marker = match std::fs::read(marker_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<InstalledModelManifest>(&bytes).ok())
    {
        Some(marker) => marker,
        None => return OnDeviceModelState::Corrupt,
    };
    if marker.id != manifest.id || marker.revision != manifest.revision {
        return OnDeviceModelState::Corrupt;
    }
    if marker.files.len() != manifest.files.len() {
        return OnDeviceModelState::Corrupt;
    }
    for expected in manifest.files {
        let Some(actual) = marker.files.iter().find(|file| file.path == expected.path) else {
            return OnDeviceModelState::Corrupt;
        };
        let path = dir.join(expected.path);
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            return OnDeviceModelState::Corrupt;
        };
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() != expected.bytes
            || actual.bytes != expected.bytes
            || !actual.sha256.eq_ignore_ascii_case(expected.sha256)
        {
            return OnDeviceModelState::Corrupt;
        }
    }
    OnDeviceModelState::Ready
}

pub fn delete_model(models_root: &Path, model: &str) -> Result<(), String> {
    let id = validate_model_id(model)?;
    let dir = model_dir(models_root, id);
    if !dir.starts_with(models_root) {
        return Err("invalid on-device model path".into());
    }
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|error| error.to_string())?;
    }
    let sidecar = archive_sha_path(models_root, id);
    if sidecar.exists() {
        std::fs::remove_file(&sidecar).map_err(|error| error.to_string())?;
    }
    for leftover in [
        models_root.join(format!("{id}.partial")),
        models_root.join(format!("{id}.extracting")),
        models_root.join(format!("{id}.old")),
    ] {
        if leftover.is_dir() {
            let _ = std::fs::remove_dir_all(&leftover);
        } else if leftover.exists() {
            let _ = std::fs::remove_file(&leftover);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::{AsrError, AsrOptions, AsrProvider};

    fn temp_root(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "voiceflow-ondevice-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_fixture(root: &Path) {
        let dir = root.join(DEFAULT_MODEL_ID);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MODEL_WEIGHTS_FILE), b"onnx").unwrap();
        std::fs::write(dir.join(MODEL_TOKENS_FILE), b"tokens").unwrap();
        std::fs::write(
            archive_sha_path(root, DEFAULT_MODEL_ID),
            crate::ondevice_models::SENSEVOICE_ARCHIVE_SHA256,
        )
        .unwrap();
    }

    #[test]
    fn ready_helper_uses_catalog_inner_names() {
        assert_eq!(
            MODEL_WEIGHTS_FILE,
            crate::ondevice_models::inner_file_name(crate::ondevice_models::SENSEVOICE_INNER_ONNX)
        );
        assert_eq!(
            MODEL_TOKENS_FILE,
            crate::ondevice_models::inner_file_name(
                crate::ondevice_models::SENSEVOICE_INNER_TOKENS
            )
        );
        assert_eq!(DEFAULT_MODEL_ID, crate::ondevice_models::SENSEVOICE_ID);
        let root = temp_root("ready-inner");
        write_fixture(&root);
        assert!(model_files_are_ready(
            &root,
            crate::ondevice_models::SENSEVOICE_ID
        ));
        assert!(!inference_is_ready(
            &root,
            crate::ondevice_models::SENSEVOICE_ID
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ready_requires_matching_sha_and_expected_files() {
        let root = temp_root("ready-sha");
        write_fixture(&root);
        let sidecar = root.join(format!("{DEFAULT_MODEL_ID}.archive.sha256"));
        std::fs::write(
            &sidecar,
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap();
        assert_eq!(
            model_state(&root, DEFAULT_MODEL_ID),
            OnDeviceModelState::Corrupt
        );
        std::fs::write(&sidecar, crate::ondevice_models::SENSEVOICE_ARCHIVE_SHA256).unwrap();
        assert_eq!(
            model_state(&root, DEFAULT_MODEL_ID),
            OnDeviceModelState::Ready
        );
        std::fs::remove_file(root.join(DEFAULT_MODEL_ID).join(MODEL_TOKENS_FILE)).unwrap();
        assert_eq!(
            model_state(&root, DEFAULT_MODEL_ID),
            OnDeviceModelState::Corrupt
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn delete_removes_dir_and_sidecar_hash() {
        let root = temp_root("delete-sidecar");
        write_fixture(&root);
        let dir = root.join(DEFAULT_MODEL_ID);
        let sidecar = archive_sha_path(&root, DEFAULT_MODEL_ID);
        assert!(dir.is_dir());
        assert!(sidecar.is_file());
        delete_model(&root, DEFAULT_MODEL_ID).unwrap();
        assert!(!dir.exists());
        assert!(!sidecar.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn allowlist_rejects_path_traversal_id() {
        let root = temp_root("allowlist");
        let err = delete_model(&root, "../x").expect_err("traversal id must fail");
        assert!(err.contains("invalid"), "{err}");
        let err = delete_model(&root, "other-model").expect_err("unknown id must fail");
        assert!(err.contains("invalid"), "{err}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ready_check_rejects_path_traversal_id() {
        use std::path::Component;

        let sandbox = temp_root("ready-traverse-sandbox");
        let root = sandbox.join("models");
        std::fs::create_dir_all(&root).unwrap();
        write_fixture(&root);
        assert_eq!(
            model_state(&root, DEFAULT_MODEL_ID),
            OnDeviceModelState::Ready
        );

        let escaped = sandbox.join("x");
        std::fs::create_dir_all(&escaped).unwrap();
        std::fs::write(escaped.join(MODEL_WEIGHTS_FILE), b"onnx").unwrap();
        std::fs::write(escaped.join(MODEL_TOKENS_FILE), b"tokens").unwrap();
        std::fs::write(
            sandbox.join("x.archive.sha256"),
            crate::ondevice_models::SENSEVOICE_ARCHIVE_SHA256,
        )
        .unwrap();

        assert!(
            !model_files_are_ready(&root, "../x"),
            "settings/probe ids must not treat ../x as Ready"
        );
        assert_ne!(model_state(&root, "../x"), OnDeviceModelState::Ready);
        assert_eq!(model_state(&root, "../x"), OnDeviceModelState::Missing);

        let dir = model_dir(&root, "../x");
        assert!(
            !dir.components()
                .any(|component| matches!(component, Component::ParentDir)),
            "model_dir must not join ../x, got {}",
            dir.display()
        );
        assert!(
            dir.starts_with(&root),
            "model_dir must stay under models root, got {}",
            dir.display()
        );

        let sidecar = archive_sha_path(&root, "../x");
        assert!(
            !sidecar
                .components()
                .any(|component| matches!(component, Component::ParentDir)),
            "archive_sha_path must not join ../x, got {}",
            sidecar.display()
        );
        assert!(
            sidecar.starts_with(&root),
            "archive_sha_path must stay under models root, got {}",
            sidecar.display()
        );

        let _ = std::fs::remove_dir_all(sandbox);
    }

    #[test]
    fn lists_missing_ready_and_corrupt_models() {
        let root = temp_root("list");
        let missing = list_models(&root);
        assert_eq!(missing[0].id, DEFAULT_MODEL_ID);
        assert_eq!(missing[0].state, OnDeviceModelState::Missing);
        assert!(!missing[0].inference_ready);
        write_fixture(&root);
        assert_eq!(
            model_state(&root, DEFAULT_MODEL_ID),
            OnDeviceModelState::Ready
        );
        std::fs::remove_file(root.join(DEFAULT_MODEL_ID).join(MODEL_TOKENS_FILE)).unwrap();
        assert_eq!(
            model_state(&root, DEFAULT_MODEL_ID),
            OnDeviceModelState::Corrupt
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn stub_transcribe_distinguishes_missing_files_from_missing_runtime() {
        let root = temp_root("stub");
        let provider = OnDeviceAsrProvider::new(root.clone(), DEFAULT_MODEL_ID);
        let error = provider
            .transcribe_batch(Vec::new(), AsrOptions::default())
            .await
            .expect_err("stub must not transcribe");
        assert!(matches!(error, AsrError::OnDeviceModelMissing(_)));
        write_fixture(&root);
        let error = provider
            .transcribe_batch(Vec::new(), AsrOptions::default())
            .await
            .expect_err("verified model files do not imply inference readiness");
        assert!(matches!(error, AsrError::OnDeviceInferenceUnavailable(_)));
        assert!(provider.capabilities().batch_transcription);
        let _ = std::fs::remove_dir_all(root);
    }
}
