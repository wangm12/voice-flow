use crate::{
    gguf_cleanup_store, invalidate_cleanup_warmup_for_model_transition, llama_cleanup_runtime,
    lock_recover, model_resources, modelstore, ollama_local, ondevice_asr, ondevice_download,
    ondevice_runtime, schedule_startup_cleanup_warmup, settings_error::SettingsCommandError,
    startup_model_is_available, store, AppState,
};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{Emitter, Manager, State};

pub(crate) const LOCAL_MODELS_STATUS_EVENT: &str = "local-models://status";

#[derive(Clone, serde::Serialize)]
#[serde(tag = "domain", rename_all = "snake_case")]
pub(crate) enum LocalModelStatusEvent {
    OnDeviceAsr {
        status: crate::ondevice_asr::OnDeviceModelStatus,
    },
    OnDeviceRuntime,
    CleanupStore {
        status: CleanupModelStoreRuntimeStatus,
    },
    MlxCleanup {
        status: crate::modelstore::MlxCleanupModelStatus,
    },
    GgufCleanup {
        status: crate::gguf_cleanup_store::ModelStatus,
    },
}

pub(crate) fn cleanup_model_store_runtime_status(
    state: &AppState,
) -> CleanupModelStoreRuntimeStatus {
    let runtime = state.mlx_cleanup_runtime.status();
    CleanupModelStoreRuntimeStatus {
        store: state.cleanup_model_store.status(&state.models_root),
        platform_supported: cleanup_model_platform_supported(),
        runtime_state: runtime.state,
        runtime_model_loaded: runtime.model_loaded,
    }
}

pub(crate) fn emit_cleanup_store_status(app: &tauri::AppHandle, state: &AppState) {
    let status = cleanup_model_store_runtime_status(state);
    let _ = app.emit(
        LOCAL_MODELS_STATUS_EVENT,
        LocalModelStatusEvent::CleanupStore { status },
    );
}

pub(crate) fn emit_mlx_cleanup_status_by_id(app: &tauri::AppHandle, state: &AppState, id: &str) {
    let compatibility_error =
        modelstore::mlx_model(id).and_then(mlx_cleanup_model_compatibility_error);
    if let Ok(status) =
        state
            .mlx_cleanup_catalog
            .status(&state.models_root, id, compatibility_error)
    {
        let _ = app.emit(MLX_CLEANUP_MODEL_STATUS_EVENT, status.clone());
        let _ = app.emit(
            LOCAL_MODELS_STATUS_EVENT,
            LocalModelStatusEvent::MlxCleanup { status },
        );
    }
}

fn emit_gguf_cleanup_status_by_id(app: &tauri::AppHandle, state: &AppState, id: &str) {
    if let Some(manifest) = gguf_cleanup_store::model(id) {
        let compatibility_error = gguf_model_compatibility_error(state, manifest);
        let status = state.gguf_cleanup_store.status(
            &state.models_root,
            manifest.id,
            compatibility_error.is_none(),
            compatibility_error,
        );
        let _ = app.emit(gguf_cleanup_store::DOWNLOAD_EVENT, status.clone());
        let _ = app.emit(
            LOCAL_MODELS_STATUS_EVENT,
            LocalModelStatusEvent::GgufCleanup { status },
        );
    }
}

async fn emit_on_device_asr_status_by_id(app: &tauri::AppHandle, state: &AppState, id: &str) {
    let runtime = ondevice_runtime::shared_runtime(&state.models_root)
        .status()
        .await;
    if let Some(status) = ondevice_download::list_statuses_with_runtime(
        &state.models_root,
        &state.downloads,
        Some(&runtime),
    )
    .into_iter()
    .find(|item| item.id == id)
    {
        let _ = app.emit(ondevice_download::DOWNLOAD_EVENT, status.clone());
        let _ = app.emit(
            LOCAL_MODELS_STATUS_EVENT,
            LocalModelStatusEvent::OnDeviceAsr { status },
        );
    }
}

pub(crate) fn watch_cleanup_model_store_completion(app: tauri::AppHandle) {
    let Some(warmup_enabled) = app
        .try_state::<AppState>()
        .map(|state| state.cleanup_warmup_enabled)
    else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let mut revisions = state.cleanup_model_store.subscribe_revision();
        loop {
            let (operation_finished, operation_succeeded, runtime_status) = {
                let Some(state) = app.try_state::<AppState>() else {
                    return;
                };
                if state.exit_state.load(Ordering::Acquire) != 0 {
                    return;
                }
                let runtime_status = cleanup_model_store_runtime_status(&state);
                let operation_finished = runtime_status.store.operation.is_none();
                let succeeded = cleanup_model_store_completion_succeeded(&runtime_status.store);
                (operation_finished, succeeded, runtime_status)
            };
            let _ = app.emit(
                LOCAL_MODELS_STATUS_EVENT,
                LocalModelStatusEvent::CleanupStore {
                    status: runtime_status,
                },
            );
            if operation_finished {
                if warmup_enabled && operation_succeeded {
                    schedule_startup_cleanup_warmup(&app, false);
                }
                return;
            }
            tokio::select! {
                changed = revisions.changed() => {
                    if changed.is_err() { return; }
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            }
        }
    });
}

pub(crate) fn cleanup_model_store_completion_succeeded(
    status: &modelstore::CleanupModelStatus,
) -> bool {
    status.last_operation_error.is_none()
        && matches!(
            status.last_operation.as_deref(),
            Some("import" | "download")
        )
        && status.id == modelstore::MODEL_ID
        && status.repository == modelstore::REPOSITORY
        && status.revision == modelstore::REVISION
        && status.managed
        && matches!(status.state.as_str(), "installed" | "ready")
}

#[tauri::command]
pub(crate) async fn get_local_cleanup_status(
    state: State<'_, AppState>,
) -> Result<ollama_local::LocalCleanupStatus, String> {
    let settings = lock_recover(&state.settings).clone();
    Ok(ollama_local::status(&settings.ollama_base_url, ollama_local::MODEL).await)
}

#[derive(Clone, serde::Serialize)]
pub(crate) struct CleanupModelStoreRuntimeStatus {
    #[serde(flatten)]
    pub(crate) store: modelstore::CleanupModelStatus,
    pub(crate) platform_supported: bool,
    pub(crate) runtime_state: &'static str,
    pub(crate) runtime_model_loaded: bool,
}

pub(crate) const CLEANUP_MODEL_PLATFORM_UNSUPPORTED: &str =
    "VoiceFlow MLX cleanup requires Apple Silicon and macOS 14 or later.";
pub(crate) const MLX_CLEANUP_MODEL_STATUS_EVENT: &str = "mlx-cleanup-model://status";

pub(crate) fn cleanup_model_platform_supported() -> bool {
    crate::ondevice_asr::platform_supported()
}

pub(crate) fn cleanup_model_platform_gate(platform_supported: bool) -> Result<(), &'static str> {
    if platform_supported {
        Ok(())
    } else {
        Err(CLEANUP_MODEL_PLATFORM_UNSUPPORTED)
    }
}

pub(crate) fn require_cleanup_model_platform() -> Result<(), String> {
    cleanup_model_platform_gate(cleanup_model_platform_supported())
        .map_err(|error| error.to_owned())
}

#[tauri::command]
pub(crate) fn get_cleanup_model_store_status(
    state: State<'_, AppState>,
) -> Result<CleanupModelStoreRuntimeStatus, String> {
    let runtime = state.mlx_cleanup_runtime.status();
    Ok(CleanupModelStoreRuntimeStatus {
        store: state.cleanup_model_store.status(&state.models_root),
        platform_supported: cleanup_model_platform_supported(),
        runtime_state: runtime.state,
        runtime_model_loaded: runtime.model_loaded,
    })
}

#[tauri::command]
pub(crate) async fn import_cleanup_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), SettingsCommandError> {
    let result: Result<(), String> = async {
        if state.exit_state.load(Ordering::Acquire) != 0 {
            return Err("VoiceFlow is shutting down.".into());
        }
        require_cleanup_model_platform()?;
        use tauri_plugin_dialog::DialogExt;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        app.dialog().file().pick_folder(move |selected| {
            let selected = selected
                .map(|path| path.into_path().map_err(|error| error.to_string()))
                .transpose();
            let _ = sender.send(selected);
        });
        let Some(selected) = receiver
            .await
            .map_err(|_| "The model folder picker did not return a result.".to_owned())??
        else {
            return Ok(());
        };
        let result = state
            .cleanup_model_store
            .start_import(state.models_root.clone(), selected);
        if result.is_ok() {
            invalidate_cleanup_warmup_for_model_transition(&state);
            watch_cleanup_model_store_completion(app.clone());
        }
        result
    }
    .await;
    result.map_err(SettingsCommandError::from_string)
}

#[tauri::command]
pub(crate) fn download_cleanup_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), SettingsCommandError> {
    let result: Result<(), String> = (|| {
        if state.exit_state.load(Ordering::Acquire) != 0 {
            return Err("VoiceFlow is shutting down.".into());
        }
        require_cleanup_model_platform()?;
        let result = state
            .cleanup_model_store
            .start_download(state.models_root.clone());
        if result.is_ok() {
            invalidate_cleanup_warmup_for_model_transition(&state);
            watch_cleanup_model_store_completion(app);
        }
        result
    })();
    result.map_err(SettingsCommandError::from_string)
}

#[tauri::command]
pub(crate) fn cancel_cleanup_model_store_operation(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> bool {
    let cancelled = state.cleanup_model_store.cancel();
    emit_cleanup_store_status(&app, &state);
    cancelled
}

#[tauri::command]
pub(crate) async fn delete_cleanup_model_store(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), SettingsCommandError> {
    let result: Result<(), String> = async {
        if state.exit_state.load(Ordering::Acquire) != 0 {
            return Err("VoiceFlow is shutting down.".into());
        }
        invalidate_cleanup_warmup_for_model_transition(&state);
        let runtime = Arc::clone(&state.mlx_cleanup_runtime);
        let result = state
            .cleanup_model_store
            .delete_with_unload(state.models_root.clone(), move || async move {
                runtime.unload().await
            })
            .await;
        emit_cleanup_store_status(&app, &state);
        if result.is_err() && startup_model_is_available(&state, modelstore::MODEL_ID) {
            schedule_startup_cleanup_warmup(&app, false);
        }
        result
    }
    .await;
    result.map_err(SettingsCommandError::from_string)
}

pub(crate) fn mlx_cleanup_model_compatibility_error(
    model: &modelstore::MlxCleanupModel,
) -> Option<String> {
    if !cleanup_model_platform_supported() {
        return Some("unsupported_platform".to_owned());
    }
    crate::model_resources::static_model_memory_compatibility(model.id, model.revision)
        .err()
        .map(|reason| match reason {
            crate::model_resources::MemoryUnavailableReason::BudgetExceeded => {
                "insufficient_memory"
            }
            crate::model_resources::MemoryUnavailableReason::UnknownPhysicalMemory => {
                "memory_unknown"
            }
            _ => "memory_unavailable",
        })
        .map(str::to_owned)
}

#[tauri::command]
pub(crate) fn list_mlx_cleanup_models(
    state: State<'_, AppState>,
) -> Result<Vec<modelstore::MlxCleanupModelStatus>, String> {
    Ok(state
        .mlx_cleanup_catalog
        .statuses(&state.models_root, |model| {
            mlx_cleanup_model_compatibility_error(model)
        }))
}

#[tauri::command]
pub(crate) fn download_mlx_cleanup_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), SettingsCommandError> {
    let result: Result<(), String> = (|| {
        if state.exit_state.load(Ordering::Acquire) != 0 {
            return Err("VoiceFlow is shutting down.".into());
        }
        let model = modelstore::mlx_model(&id)
            .ok_or_else(|| "unsupported local MLX cleanup model".to_owned())?;
        let compatibility_error = mlx_cleanup_model_compatibility_error(model);
        state.mlx_cleanup_catalog.start_background_download(
            state.models_root.clone(),
            &id,
            compatibility_error,
            move |status| {
                let _ = app.emit(MLX_CLEANUP_MODEL_STATUS_EVENT, status.clone());
                let _ = app.emit(
                    LOCAL_MODELS_STATUS_EVENT,
                    LocalModelStatusEvent::MlxCleanup { status },
                );
            },
        )
    })();
    result.map_err(SettingsCommandError::from_string)
}

#[tauri::command]
pub(crate) fn verify_mlx_cleanup_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), SettingsCommandError> {
    let result: Result<(), String> = (|| {
        if state.exit_state.load(Ordering::Acquire) != 0 {
            return Err("VoiceFlow is shutting down.".into());
        }
        let model = modelstore::mlx_model(&id)
            .ok_or_else(|| "unsupported local MLX cleanup model".to_owned())?;
        let compatibility_error = mlx_cleanup_model_compatibility_error(model);
        state.mlx_cleanup_catalog.start_background_verification(
            state.models_root.clone(),
            &id,
            compatibility_error,
            move |status| {
                let _ = app.emit(MLX_CLEANUP_MODEL_STATUS_EVENT, status.clone());
                let _ = app.emit(
                    LOCAL_MODELS_STATUS_EVENT,
                    LocalModelStatusEvent::MlxCleanup { status },
                );
            },
        )
    })();
    result.map_err(SettingsCommandError::from_string)
}

#[tauri::command]
pub(crate) fn cancel_mlx_cleanup_model_download(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<bool, SettingsCommandError> {
    let result: Result<bool, String> = (|| {
        if state.exit_state.load(Ordering::Acquire) != 0 {
            return Ok(false);
        }
        let cancelled = state.mlx_cleanup_catalog.cancel_download(&id)?;
        emit_mlx_cleanup_status_by_id(&app, &state, &id);
        Ok(cancelled)
    })();
    result.map_err(SettingsCommandError::from_string)
}

#[tauri::command]
pub(crate) async fn delete_mlx_cleanup_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), SettingsCommandError> {
    let result: Result<(), String> = async {
        if state.exit_state.load(Ordering::Acquire) != 0 {
            return Err("VoiceFlow is shutting down.".into());
        }
        if modelstore::mlx_model(&id).is_none() {
            return Err("unsupported local MLX cleanup model".into());
        }
        let _settings_gate = state.settings_gate.lock().await;
        let settings = lock_recover(&state.settings).clone();
        if settings.cleanup_enabled
            && settings.cleanup_provider == crate::providers::EngineProvider::OnDeviceMlx
            && settings.cleanup_model == id
        {
            return Err("local_cleanup_model_is_active".into());
        }
        invalidate_cleanup_warmup_for_model_transition(&state);
        let runtime = Arc::clone(&state.mlx_cleanup_runtime);
        let unload_id = id.clone();
        let result = state
            .mlx_cleanup_catalog
            .delete_with_unload(state.models_root.clone(), &id, move || async move {
                runtime.unload_model(Some(&unload_id)).await
            })
            .await;
        emit_mlx_cleanup_status_by_id(&app, &state, &id);
        result
    }
    .await;
    result.map_err(SettingsCommandError::from_string)
}

pub(crate) fn local_cleanup_compatibility_error(state: &AppState) -> Option<&'static str> {
    llama_cleanup_runtime::compatibility_error(state.llama_cleanup_runtime.helper_path())
}

pub(crate) fn validate_new_ondevice_asr_selection(
    previous: &store::Settings,
    next: &store::Settings,
) -> Result<(), String> {
    let retired_model_selected = next.asr_provider == crate::providers::EngineProvider::OnDevice
        && crate::ondevice_models::mlx_model_download_retired(&next.asr_model);
    let same_saved_route = previous.asr_provider == crate::providers::EngineProvider::OnDevice
        && previous.asr_model == next.asr_model;
    if retired_model_selected && !same_saved_route {
        return Err("retired_on_device_asr_model".to_owned());
    }
    Ok(())
}

pub(crate) fn gguf_model_compatibility_error(
    state: &AppState,
    manifest: &gguf_cleanup_store::ModelManifest,
) -> Option<&'static str> {
    local_cleanup_compatibility_error(state).or_else(|| {
        crate::model_resources::static_model_memory_compatibility(manifest.id, manifest.revision)
            .err()
            .map(|reason| match reason {
                crate::model_resources::MemoryUnavailableReason::BudgetExceeded => {
                    "insufficient_memory"
                }
                crate::model_resources::MemoryUnavailableReason::UnknownPhysicalMemory => {
                    "memory_unknown"
                }
                _ => "memory_unavailable",
            })
    })
}

pub(crate) fn validate_new_gguf_cleanup_selection(
    state: &AppState,
    previous: &store::Settings,
    next: &store::Settings,
) -> Result<(), String> {
    let newly_active = next.cleanup_enabled
        && next.cleanup_provider == crate::providers::EngineProvider::OnDeviceLlamaCpp
        && (!previous.cleanup_enabled
            || previous.cleanup_provider != next.cleanup_provider
            || previous.cleanup_model != next.cleanup_model);
    if !newly_active {
        return Ok(());
    }
    let manifest = gguf_cleanup_store::model(&next.cleanup_model)
        .ok_or_else(|| "unsupported local cleanup model".to_owned())?;
    let verified = state
        .gguf_cleanup_store
        .resolve_ready(&state.models_root, manifest.id)
        .map_err(|_| "selected local cleanup model is missing or not verified".to_owned())?;
    if let Some(error) = gguf_model_compatibility_error(state, manifest) {
        return Err(error.to_owned());
    }
    let saved_route = previous.cleanup_provider
        == crate::providers::EngineProvider::OnDeviceLlamaCpp
        && previous.cleanup_model == next.cleanup_model;
    if !saved_route && !gguf_cleanup_store::is_user_available_model(manifest.id) {
        return Err("local GGUF cleanup model is not available for new selection".to_owned());
    }
    drop(verified);
    Ok(())
}

pub(crate) fn is_active_gguf_cleanup_model(settings: &store::Settings, id: &str) -> bool {
    settings.cleanup_enabled
        && settings.cleanup_provider == crate::providers::EngineProvider::OnDeviceLlamaCpp
        && settings.cleanup_model == id
}

pub(crate) fn gguf_route_needs_retirement(
    previous: &store::Settings,
    next: &store::Settings,
) -> bool {
    previous.cleanup_provider == crate::providers::EngineProvider::OnDeviceLlamaCpp
        && (previous.cleanup_provider != next.cleanup_provider
            || previous.cleanup_model != next.cleanup_model
            || (previous.cleanup_enabled && !next.cleanup_enabled))
}

pub(crate) fn validate_new_mlx_cleanup_selection(
    state: &AppState,
    previous: &store::Settings,
    next: &store::Settings,
) -> Result<(), String> {
    let newly_active = next.cleanup_enabled
        && next.cleanup_provider == crate::providers::EngineProvider::OnDeviceMlx
        && (!previous.cleanup_enabled
            || previous.cleanup_provider != next.cleanup_provider
            || previous.cleanup_model != next.cleanup_model);
    if !newly_active || next.cleanup_model == modelstore::MODEL_ID {
        return Ok(());
    }
    let model = modelstore::mlx_model(&next.cleanup_model)
        .ok_or_else(|| "unsupported local MLX cleanup model".to_owned())?;
    let compatibility_error = mlx_cleanup_model_compatibility_error(model);
    let status =
        state
            .mlx_cleanup_catalog
            .status(&state.models_root, model.id, compatibility_error)?;
    if !status.compatible {
        return Err(status
            .compatibility_error
            .unwrap_or_else(|| "local MLX cleanup model is incompatible".to_owned()));
    }
    let saved_route = previous.cleanup_provider == crate::providers::EngineProvider::OnDeviceMlx
        && previous.cleanup_model == next.cleanup_model;
    if !saved_route && !modelstore::is_user_available_mlx_cleanup_model(model.id) {
        return Err("local MLX cleanup model is not available for new selection".to_owned());
    }
    let verified = state
        .mlx_cleanup_catalog
        .resolve_ready(&state.models_root, model.id)
        .map_err(|_| "selected local MLX cleanup model is missing or not verified".to_owned())?;
    drop(verified);
    Ok(())
}

pub(crate) fn mlx_route_needs_retirement(
    previous: &store::Settings,
    next: &store::Settings,
) -> bool {
    previous.cleanup_provider == crate::providers::EngineProvider::OnDeviceMlx
        && (previous.cleanup_provider != next.cleanup_provider
            || previous.cleanup_model != next.cleanup_model
            || (previous.cleanup_enabled && !next.cleanup_enabled))
}

#[tauri::command]
pub(crate) fn list_local_cleanup_models(
    state: State<'_, AppState>,
) -> Result<Vec<gguf_cleanup_store::ModelStatus>, String> {
    Ok(gguf_cleanup_store::MODELS
        .iter()
        .map(|manifest| {
            let compatibility_error = gguf_model_compatibility_error(&state, manifest);
            state.gguf_cleanup_store.status(
                &state.models_root,
                manifest.id,
                compatibility_error.is_none(),
                compatibility_error,
            )
        })
        .collect())
}

#[tauri::command]
pub(crate) fn download_local_cleanup_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), SettingsCommandError> {
    let result: Result<(), String> = (|| {
        if state.exit_state.load(Ordering::Acquire) != 0 {
            return Err("VoiceFlow is shutting down.".into());
        }
        let manifest = gguf_cleanup_store::model(&id)
            .ok_or_else(|| "unsupported local cleanup model".to_owned())?;
        if !gguf_cleanup_store::is_user_available_model(manifest.id) {
            return Err("this local cleanup model is not available for new downloads".to_owned());
        }
        let compatibility_error = gguf_model_compatibility_error(&state, manifest);
        let compatible = compatibility_error.is_none();
        state.gguf_cleanup_store.start_background_download(
            state.models_root.clone(),
            id,
            compatible,
            compatibility_error.map(str::to_owned),
            move |status| {
                let _ = app.emit(gguf_cleanup_store::DOWNLOAD_EVENT, status.clone());
                let _ = app.emit(
                    LOCAL_MODELS_STATUS_EVENT,
                    LocalModelStatusEvent::GgufCleanup { status },
                );
            },
        )
    })();
    result.map_err(SettingsCommandError::from_string)
}

#[tauri::command]
pub(crate) fn cancel_local_cleanup_model_download(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<bool, SettingsCommandError> {
    let result: Result<bool, String> = (|| {
        if state.exit_state.load(Ordering::Acquire) != 0 {
            return Ok(false);
        }
        let cancelled = state.gguf_cleanup_store.cancel(&id)?;
        emit_gguf_cleanup_status_by_id(&app, &state, &id);
        Ok(cancelled)
    })();
    result.map_err(SettingsCommandError::from_string)
}

#[tauri::command]
pub(crate) async fn delete_local_cleanup_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), SettingsCommandError> {
    let result: Result<(), String> = async {
        if state.exit_state.load(Ordering::Acquire) != 0 {
            return Err("VoiceFlow is shutting down.".into());
        }
        if gguf_cleanup_store::model(&id).is_none() {
            return Err("unsupported local cleanup model".into());
        }
        let _settings_gate = state.settings_gate.lock().await;
        let settings = lock_recover(&state.settings).clone();
        if is_active_gguf_cleanup_model(&settings, &id) {
            return Err("local_cleanup_model_is_active".into());
        }
        let runtime = Arc::clone(&state.llama_cleanup_runtime);
        let unload_id = id.clone();
        let result = state
            .gguf_cleanup_store
            .delete_with_unload(state.models_root.clone(), &id, move || async move {
                runtime.unload(Some(&unload_id)).await
            })
            .await;
        emit_gguf_cleanup_status_by_id(&app, &state, &id);
        result
    }
    .await;
    result.map_err(SettingsCommandError::from_string)
}

#[tauri::command]
pub(crate) async fn list_on_device_models(
    state: State<'_, AppState>,
) -> Result<Vec<ondevice_asr::OnDeviceModelStatus>, String> {
    let runtime = ondevice_runtime::shared_runtime(&state.models_root)
        .status()
        .await;
    Ok(ondevice_download::list_statuses_with_runtime(
        &state.models_root,
        &state.downloads,
        Some(&runtime),
    ))
}

#[tauri::command]
pub(crate) fn get_on_device_memory_budget() -> model_resources::DeviceMemoryBudget {
    model_resources::current_device_memory_budget()
}

#[tauri::command]
pub(crate) fn download_on_device_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), SettingsCommandError> {
    let result: Result<(), String> = (|| {
        if state.exit_state.load(Ordering::Acquire) != 0 {
            return Err("VoiceFlow is shutting down.".into());
        }
        let models_root = state.models_root.clone();
        let manager = state.downloads.clone();
        ondevice_download::start_background_download(models_root, manager, id, move |status| {
            let _ = app.emit(ondevice_download::DOWNLOAD_EVENT, status.clone());
            let _ = app.emit(
                LOCAL_MODELS_STATUS_EVENT,
                LocalModelStatusEvent::OnDeviceAsr { status },
            );
        })
    })();
    result.map_err(SettingsCommandError::from_string)
}

#[tauri::command]
pub(crate) async fn cancel_on_device_download(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), SettingsCommandError> {
    let result: Result<(), String> = async {
        state.downloads.cancel(&id)?;
        emit_on_device_asr_status_by_id(&app, &state, &id).await;
        Ok(())
    }
    .await;
    result.map_err(SettingsCommandError::from_string)
}

#[tauri::command]
pub(crate) fn cancel_on_device_model_load(
    state: State<'_, AppState>,
    id: String,
) -> Result<bool, SettingsCommandError> {
    let result: Result<bool, String> = (|| {
        if state.exit_state.load(Ordering::Acquire) != 0 {
            return Ok(false);
        }
        Ok(ondevice_runtime::shared_runtime(&state.models_root).cancel_model_load(&id))
    })();
    result.map_err(SettingsCommandError::from_string)
}

#[tauri::command]
pub(crate) async fn delete_on_device_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), SettingsCommandError> {
    let result: Result<(), String> = async {
        if state.exit_state.load(Ordering::Acquire) != 0 {
            return Err("VoiceFlow is shutting down.".into());
        }
        let models_root = state.models_root.clone();
        let downloads = state.downloads.clone();
        ondevice_download::delete_and_unload(&models_root, &downloads, &id).await?;
        emit_on_device_asr_status_by_id(&app, &state, &id).await;
        Ok(())
    }
    .await;
    result.map_err(SettingsCommandError::from_string)
}
