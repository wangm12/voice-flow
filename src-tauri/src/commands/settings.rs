use crate::{
    apply_context_configuration, audio, autostart, begin_cleanup_model_demand,
    cancel_invalidated_dictation, changed_mapping_binding_ids, clamshell,
    commit_learning_settings_snapshot, commit_settings_snapshot, delivery_diagnostics, dictation,
    dictation_has_asr_credential, dictation_trace_sink, dictionary_learn,
    dictionary_learning_audit, emit_context_snapshot_if_changed, engine,
    gguf_route_needs_retirement, groq, hotkey, invalidate_session_context_after_policy_change,
    keychain, lock_recover, metrics, mlx_route_needs_retirement, native_glass, permissions,
    refresh_context_preview_snapshot, release_operation, research_trace,
    restore_session_custom_credential_after_patch,
    revalidate_audio_recovery_after_retention_change, schedule_startup_cleanup_warmup,
    settings_error::SettingsCommandError, store, try_claim_operation,
    validate_new_gguf_cleanup_selection, validate_new_mlx_cleanup_selection,
    validate_new_ondevice_asr_selection, whats_new, AppState, OperationLease, Phase,
    StartupWarmupSettings,
};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{Emitter, Manager, State};

#[tauri::command]
pub(crate) fn get_settings(state: State<'_, AppState>) -> store::SettingsView {
    store::SettingsView::from(&*lock_recover(&state.settings))
}

#[tauri::command]
pub(crate) fn get_guide_acknowledgements(
    state: State<'_, AppState>,
) -> Vec<store::GuideAcknowledgementView> {
    lock_recover(&state.settings).guide_acknowledgements.view()
}

#[tauri::command]
pub(crate) async fn acknowledge_guide(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    guide_id: store::OnDemandGuideId,
    version: u32,
) -> Result<Vec<store::GuideAcknowledgementView>, SettingsCommandError> {
    let result: Result<Vec<store::GuideAcknowledgementView>, String> = async {
        let _guard = state.settings_gate.lock().await;
        if state.exit_state.load(Ordering::Acquire) != 0 {
            return Err("VoiceFlow is shutting down.".into());
        }
        let current = lock_recover(&state.settings).clone();
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?;
        let settings = store::acknowledge_guide(&dir, &current, guide_id, version)
            .map_err(|error| error.to_string())?;
        // This dedicated UI fact cannot change processing config or permissions.
        commit_settings_snapshot(&state, settings);
        Ok(lock_recover(&state.settings).guide_acknowledgements.view())
    }
    .await;
    result.map_err(SettingsCommandError::from_string)
}

#[tauri::command]
pub(crate) fn get_latency_metrics(state: State<'_, AppState>) -> metrics::LatencyMetrics {
    state.metrics.snapshot()
}

pub(crate) const MAX_DICTIONARY_FILE_BYTES: u64 = 1024 * 1024;

pub(crate) fn read_dictionary_file_contents(path: &Path) -> Result<String, String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    if !matches!(extension.as_deref(), Some("csv" | "txt" | "tsv")) {
        return Err("只支持 CSV、TXT 或 TSV 文件".into());
    }

    let metadata = std::fs::metadata(path).map_err(|_| "无法读取词典文件".to_owned())?;
    if !metadata.is_file() {
        return Err("词典路径不是文件".into());
    }
    if metadata.len() > MAX_DICTIONARY_FILE_BYTES {
        return Err("词典文件不能超过 1 MB".into());
    }

    let bytes = std::fs::read(path).map_err(|_| "无法读取词典文件".to_owned())?;
    String::from_utf8(bytes).map_err(|_| "词典文件必须使用 UTF-8 编码".into())
}

#[tauri::command]
pub(crate) fn read_dictionary_file(path: String) -> Result<String, SettingsCommandError> {
    read_dictionary_file_contents(Path::new(&path)).map_err(SettingsCommandError::from_string)
}

pub(crate) fn effective_always_on_microphone(settings: &store::Settings, release: bool) -> bool {
    !release && settings.always_on_microphone
}

pub(crate) async fn sync_microphone_warmup(app: &tauri::AppHandle, settings: &store::Settings) {
    {
        let state = app.state::<AppState>();
        let current = lock_recover(&state.settings);
        if state.exit_state.load(Ordering::Acquire) != 0
            || current.always_on_microphone != settings.always_on_microphone
            || current.input_device != settings.input_device
            || current.clamshell_microphone != settings.clamshell_microphone
        {
            return;
        }
        if !effective_always_on_microphone(settings, cfg!(not(debug_assertions)))
            || !permissions::check().microphone
        {
            audio::configure_warm(app.clone(), false, String::new());
            return;
        }
    }
    let selected = settings.input_device.clone();
    let clamshell = settings.clamshell_microphone.clone();
    let device = research_trace::spawn_blocking(move || {
        let closed = !clamshell.trim().is_empty() && clamshell::lid_closed();
        let selected = clamshell::resolve_input_device(&selected, &clamshell, closed);
        audio::selected_input_device_name(&selected)
    })
    .await
    .unwrap_or_else(|error| Err(error.to_string()));
    let state = app.state::<AppState>();
    let current = lock_recover(&state.settings);
    if state.exit_state.load(Ordering::Acquire) != 0
        || !effective_always_on_microphone(&current, cfg!(not(debug_assertions)))
        || current.input_device != settings.input_device
        || current.clamshell_microphone != settings.clamshell_microphone
        || !permissions::check().microphone
    {
        return;
    }
    // Keep the policy lock through queueing; a later save will enqueue its
    // disable/switch after this command, never before a stale probe finishes.
    match device {
        Ok(device) => audio::configure_warm(app.clone(), true, device),
        Err(error) => {
            log::warn!("microphone prewarm device unavailable: {error}");
            audio::configure_warm(app.clone(), false, String::new());
        }
    }
}

pub(crate) fn is_binding_configuration_patch(
    object: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    !object.is_empty()
        && object.keys().all(|key| {
            matches!(
                key.as_str(),
                "hotkey"
                    | "activation_mode"
                    | "verbatim_hotkey"
                    | "translation_hotkey"
                    | "translation_target_language"
                    | "selected_action_hotkey"
                    | "selected_actions_enabled"
                    | "screen_action_hotkey"
                    | "whats_new_last_seen_version"
            )
        })
}

pub(crate) fn settings_patch_requires_service_readiness(
    object: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    object.iter().any(|(key, value)| match key.as_str() {
        // These fields can change the selected service, endpoint, model, or
        // credential binding. Unknown future patch fields are strict by default.
        "api_key"
        | "asr_api_key"
        | "asr_base_url"
        | "asr_model"
        | "asr_provider"
        | "accurate_asr_base_url"
        | "accurate_asr_model"
        | "accurate_asr_provider"
        | "cleanup_api_key"
        | "cleanup_base_url"
        | "cleanup_model"
        | "cleanup_provider"
        | "custom_base_url"
        | "custom_asr"
        | "custom_llm"
        | "custom_cleanup_parameters"
        | "local_whisper_base_url"
        | "ollama_base_url"
        | "onboarded"
        | "provider_keys"
        | "provider_api_keys"
        | "vision_model"
        | "vision_provider"
        | "window_ocr_enabled"
        | "context_enabled"
        | "browser_access_enabled"
        | "context_mappings"
        | "writing_modes"
        | "output_mode"
        | "translation_target_language"
        | "selected_actions_enabled"
        | "cleanup_intensity"
        | "cascade_timeout_ms"
        | "cascade_proper_noun_threshold" => true,
        "cleanup_enabled" => value.as_bool() != Some(false),
        "strict_offline_enabled" => value.as_bool() != Some(true),
        // Keep the non-service preference list explicit. Newly added settings
        // require service readiness until reviewed here.
        "language"
        | "preferred_languages"
        | "ui_language"
        | "theme"
        | "hud_appearance"
        | "hud_color_variant"
        | "hud_processing_color_variant"
        | "hud_voice_style"
        | "hud_processing_style"
        | "hud_voice_intensity"
        | "dictionary"
        | "hotkey"
        | "activation_mode"
        | "chunk_threshold_secs"
        | "chunk_length_secs"
        | "delivery_policy"
        | "keep_audio_days"
        | "keep_history_days"
        | "keep_success_audio"
        | "screen_action_hotkey"
        | "show_tray_icon"
        | "snippets"
        | "selected_action_hotkey"
        | "dictionary_learn_enabled"
        | "local_edit_learning_enabled"
        | "input_device"
        | "input_gain"
        | "verbatim_hotkey"
        | "translation_hotkey"
        | "extra_recording_buffer_ms"
        | "audio_feedback_enabled"
        | "audio_feedback_volume"
        | "vad_enabled"
        | "always_on_microphone"
        | "clamshell_microphone"
        | "autostart_enabled"
        | "whats_new_last_seen_version"
        | "debug_mode"
        | "fuzzy_dictionary_enabled" => false,
        _ => true,
    })
}

pub(crate) fn bindings_changed(old: &store::Settings, new: &store::Settings) -> bool {
    old.hotkey != new.hotkey
        || old.activation_mode != new.activation_mode
        || old.verbatim_hotkey != new.verbatim_hotkey
        || old.translation_hotkey != new.translation_hotkey
        || old.selected_action_hotkey != new.selected_action_hotkey
        || old.selected_actions_enabled != new.selected_actions_enabled
        || old.screen_action_hotkey != new.screen_action_hotkey
}

pub(crate) fn ensure_bindings_editable(state: &AppState) -> Result<(), String> {
    if lock_recover(&state.manager).phase != Phase::Idle
        || *lock_recover(&state.operation_lease) != OperationLease::Idle
    {
        return Err("录音或处理期间不能修改快捷键和录音方式。".into());
    }
    Ok(())
}

pub(crate) async fn register_all_bindings(
    app: &tauri::AppHandle,
    settings: &store::Settings,
) -> Result<(), String> {
    hotkey::apply_settings_hotkey(app, &settings.hotkey, &settings.activation_mode).await?;
    hotkey::apply_selected_action_hotkey(
        app,
        &settings.selected_action_hotkey,
        settings.selected_actions_enabled,
    )
    .await?;
    hotkey::apply_screen_action_hotkey(app, &settings.screen_action_hotkey).await?;
    hotkey::apply_verbatim_action_hotkey(app, &settings.verbatim_hotkey, &settings.activation_mode)
        .await?;
    hotkey::apply_translation_action_hotkey(
        app,
        &settings.translation_hotkey,
        &settings.activation_mode,
    )
    .await
}

pub(crate) async fn binding_transaction(
    register: impl std::future::Future<Output = Result<(), String>>,
    persist: impl FnOnce() -> Result<(), String>,
    restore: impl std::future::Future<Output = Result<(), String>>,
) -> Result<(), String> {
    let result = match register.await {
        Ok(()) => persist(),
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        return match restore.await {
            Ok(()) => Err(error),
            Err(restore_error) => Err(format!("{error}；原快捷键恢复失败：{restore_error}")),
        };
    }
    Ok(())
}

pub(crate) async fn apply_binding_settings(
    app: &tauri::AppHandle,
    state: &AppState,
    settings: store::Settings,
    restoring_capture: bool,
) -> Result<(), String> {
    let _gate = state.hotkey_gate.lock().await;
    let previous = lock_recover(&state.settings).clone();
    let cleanup_warmup_configuration_changed = StartupWarmupSettings::from_settings(&previous)
        != StartupWarmupSettings::from_settings(&settings);
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let validation = settings
        .validate_binding_changes(&previous)
        .and_then(|_| settings.validate_configuration())
        .map_err(|error| error.to_string());
    validation.as_ref().map_err(Clone::clone)?;
    validate_new_ondevice_asr_selection(&previous, &settings)?;
    validate_new_gguf_cleanup_selection(state, &previous, &settings)?;
    validate_new_mlx_cleanup_selection(state, &previous, &settings)?;
    if gguf_route_needs_retirement(&previous, &settings) {
        state.llama_cleanup_runtime.unload(None).await?;
    }
    if mlx_route_needs_retirement(&previous, &settings) {
        state
            .mlx_cleanup_runtime
            .unload_model(Some(&previous.cleanup_model))
            .await?;
    }
    let changing_bindings = bindings_changed(&previous, &settings);
    if !restoring_capture && changing_bindings {
        ensure_bindings_editable(state)?;
    }
    // Keep native listeners paused through registration, persistence and rollback.
    if changing_bindings || restoring_capture {
        hotkey::set_suspended(true);
    }
    let rollback_restored = std::sync::atomic::AtomicBool::new(true);
    let result = binding_transaction(
        async {
            validation?;
            if changing_bindings || restoring_capture {
                register_all_bindings(app, &settings).await?;
            }
            Ok(())
        },
        || store::save_configuration(&dir, &settings).map_err(|error| error.to_string()),
        async {
            if changing_bindings || restoring_capture {
                hotkey::invalidate_registration_cache();
                if register_all_bindings(app, &previous).await.is_err() {
                    hotkey::invalidate_registration_cache();
                    if let Err(error) = register_all_bindings(app, &previous).await {
                        rollback_restored.store(false, Ordering::SeqCst);
                        return Err(error);
                    }
                }
            }
            Ok(())
        },
    )
    .await;
    if result.is_ok() {
        commit_settings_snapshot(state, settings.clone());
        if cleanup_warmup_configuration_changed {
            schedule_startup_cleanup_warmup(app, false);
        }
        let _ = app.emit("settings://changed", store::SettingsView::from(&settings));
    }
    if (changing_bindings || restoring_capture) && rollback_restored.load(Ordering::SeqCst) {
        hotkey::resume_after_release().await;
    }
    result
}

pub(crate) async fn with_hotkey_registration_rollback(
    registration: impl std::future::Future<Output = Result<(), String>>,
    restore: impl std::future::Future<Output = ()>,
) -> Result<(), String> {
    let result = registration.await;
    if result.is_err() {
        restore.await;
    }
    result
}

pub(crate) async fn restore_auxiliary_hotkeys(app: &tauri::AppHandle, settings: &store::Settings) {
    let _ = hotkey::apply_selected_action_hotkey(
        app,
        &settings.selected_action_hotkey,
        settings.selected_actions_enabled,
    )
    .await;
    let _ = hotkey::apply_screen_action_hotkey(app, &settings.screen_action_hotkey).await;
    let _ = hotkey::apply_verbatim_action_hotkey(
        app,
        &settings.verbatim_hotkey,
        &settings.activation_mode,
    )
    .await;
    let _ = hotkey::apply_translation_action_hotkey(
        app,
        &settings.translation_hotkey,
        &settings.activation_mode,
    )
    .await;
}

pub(crate) struct BindingEditGuard<'a> {
    _gate: tokio::sync::MutexGuard<'a, ()>,
}
impl Drop for BindingEditGuard<'_> {
    fn drop(&mut self) {
        dictation::invalidate_pending_inputs();
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ApplySettingsOptions {
    pub(crate) autostart_explicitly_requested: bool,
    pub(crate) require_service_readiness: bool,
    pub(crate) dictionary_patch_requested: bool,
    pub(crate) hud_appearance_requested: bool,
    pub(crate) hud_patch_requested: bool,
}

pub(crate) async fn apply_settings(
    app: tauri::AppHandle,
    state: &AppState,
    mut settings: store::Settings,
    options: ApplySettingsOptions,
) -> Result<(), String> {
    let ApplySettingsOptions {
        autostart_explicitly_requested,
        require_service_readiness,
        dictionary_patch_requested,
        hud_appearance_requested,
        hud_patch_requested,
    } = options;
    if state.exit_state.load(Ordering::Acquire) != 0 {
        return Err("VoiceFlow is shutting down.".into());
    }
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    settings.always_on_microphone =
        effective_always_on_microphone(&settings, cfg!(not(debug_assertions)));
    let prev = lock_recover(&state.settings).clone();
    // Full or stale settings payloads cannot forge or erase explicit teaching ack.
    settings.guide_acknowledgements = prev.guide_acknowledgements.clone();
    // Dictionary owners are backend-managed provenance. A settings patch may
    // edit the public word projection, but cannot submit its own source map.
    settings.dictionary_sources = prev.dictionary_sources.clone();
    settings
        .validate_binding_changes(&prev)
        .map_err(|error| error.to_string())?;
    if (prev.local_edit_learning_enabled && !settings.local_edit_learning_enabled)
        || (prev.dictionary_learn_enabled && !settings.dictionary_learn_enabled)
    {
        store::clear_writing_preference_metadata(&dir).map_err(|error| {
            format!("failed to clear local History preference metadata after opt-out: {error}")
        })?;
    }
    let requested_dictionary = settings.dictionary.clone();
    let explicitly_removed_dictionary_words = if dictionary_patch_requested {
        store::reconcile_dictionary_patch(&mut settings, requested_dictionary)
    } else {
        Vec::new()
    };
    let binding_edit_guard = if bindings_changed(&prev, &settings) {
        let guard = BindingEditGuard {
            _gate: state.hotkey_gate.lock().await,
        };
        ensure_bindings_editable(state)?;
        Some(guard)
    } else {
        None
    };
    settings.normalize();
    settings.clear_stale_session_custom_credential();
    let cleanup_warmup_configuration_changed = StartupWarmupSettings::from_settings(&prev)
        != StartupWarmupSettings::from_settings(&settings);
    let custom_host_changed = crate::llm::custom_connection_origin_changed(
        &prev.resolved_provider_base(crate::providers::EngineProvider::Custom),
        &settings.resolved_provider_base(crate::providers::EngineProvider::Custom),
    );
    let custom_key_cleared = custom_host_changed
        && settings
            .provider_secret(crate::providers::EngineProvider::Custom)
            .is_empty();
    let incoming_asr_key = settings.asr_api_key.clone();
    settings.asr_api_key = store::bind_asr_key_to_host(
        settings.asr_provider,
        &prev.asr_base_url,
        &settings.asr_base_url,
        &incoming_asr_key,
        &prev.asr_api_key,
    );
    let incoming_cleanup_key = settings.cleanup_api_key.clone();
    settings.cleanup_api_key = store::bind_cleanup_key_to_host(
        settings.cleanup_provider,
        &prev.cleanup_base_url,
        &settings.cleanup_base_url,
        &incoming_cleanup_key,
        &prev.cleanup_api_key,
    );
    let asr_host_changed = if settings.asr_provider == crate::providers::EngineProvider::Custom {
        crate::llm::custom_connection_origin_changed(&prev.asr_base_url, &settings.asr_base_url)
    } else {
        crate::asr::asr_host_changed(&prev.asr_base_url, &settings.asr_base_url)
    };
    let asr_key_cleared = asr_host_changed && incoming_asr_key.trim().is_empty();
    let cleanup_host_changed =
        if settings.cleanup_provider == crate::providers::EngineProvider::Custom {
            crate::llm::custom_connection_origin_changed(
                &prev.cleanup_base_url,
                &settings.cleanup_base_url,
            )
        } else {
            crate::llm::chat_host_changed(&prev.cleanup_base_url, &settings.cleanup_base_url)
        };
    let cleanup_key_cleared = cleanup_host_changed && incoming_cleanup_key.trim().is_empty();
    if require_service_readiness {
        settings
            .validate_with_models_root(Some(&dir.join("models")))
            .map_err(|error| error.to_string())?;
    } else {
        settings
            .validate_configuration()
            .map_err(|error| error.to_string())?;
    }
    validate_new_ondevice_asr_selection(&prev, &settings)?;
    validate_new_gguf_cleanup_selection(state, &prev, &settings)?;
    validate_new_mlx_cleanup_selection(state, &prev, &settings)?;
    if gguf_route_needs_retirement(&prev, &settings) {
        state.llama_cleanup_runtime.unload(None).await?;
    }
    if mlx_route_needs_retirement(&prev, &settings) {
        state
            .mlx_cleanup_runtime
            .unload_model(Some(&prev.cleanup_model))
            .await?;
    }
    if custom_key_cleared {
        store::clear_custom_credentials_before_host_change(&dir)
            .map_err(|error| format!("failed to remove custom API key securely: {error}"))?;
    }
    // Settings edits validate their structure. Route edits and onboarding
    // completion additionally require provider readiness; ordinary preferences
    // never trigger provider requests or credential reads.
    if !prev.always_on_microphone
        && settings.always_on_microphone
        && !permissions::request_microphone_if_needed().await?
    {
        return Err("Microphone permission is required for microphone prewarming".into());
    }
    let mut autostart_change = autostart::Change::for_settings(
        &app,
        prev.autostart_enabled,
        settings.autostart_enabled,
        autostart_explicitly_requested,
    )?;
    let hotkey = settings.hotkey.clone();
    let mode = settings.activation_mode.clone();
    let selected_hotkey = settings.selected_action_hotkey.clone();
    let selected_enabled = settings.selected_actions_enabled;
    let screen_hotkey = settings.screen_action_hotkey.clone();
    let hotkey_changed = prev.hotkey != hotkey || prev.activation_mode != mode;
    let selected_hotkey_changed = prev.selected_action_hotkey != selected_hotkey
        || prev.selected_actions_enabled != selected_enabled;
    let screen_hotkey_changed = prev.screen_action_hotkey != screen_hotkey;
    let context_changed = prev.context_enabled != settings.context_enabled
        || prev.onboarded != settings.onboarded
        || prev.browser_access_enabled != settings.browser_access_enabled
        || prev.context_mappings != settings.context_mappings
        || prev.writing_modes != settings.writing_modes
        || prev.window_ocr_enabled != settings.window_ocr_enabled
        || prev.vision_provider != settings.vision_provider
        || prev.vision_model != settings.vision_model;
    let context_source_policy_changed = prev.context_enabled != settings.context_enabled
        || prev.browser_access_enabled != settings.browser_access_enabled
        || prev.context_mappings != settings.context_mappings
        || prev.window_ocr_enabled != settings.window_ocr_enabled
        || prev.vision_provider != settings.vision_provider
        || prev.vision_model != settings.vision_model;
    let context_enabled = settings.context_enabled;
    let onboarded = settings.onboarded;
    let browser_access_enabled = settings.browser_access_enabled;
    let context_mappings = settings.context_mappings.clone();
    let writing_modes = settings.writing_modes.clone();
    let tray_visible = settings.show_tray_icon;
    let tray_visibility_changed = prev.show_tray_icon != tray_visible;

    if !hotkey::is_suspended() && hotkey_changed {
        with_hotkey_registration_rollback(
            hotkey::apply_settings_hotkey(&app, &hotkey, &mode),
            restore_auxiliary_hotkeys(&app, &prev),
        )
        .await?;
    }
    if !hotkey::is_suspended() && (hotkey_changed || selected_hotkey_changed) {
        if let Err(error) =
            hotkey::apply_selected_action_hotkey(&app, &selected_hotkey, selected_enabled).await
        {
            if hotkey_changed {
                let _ =
                    hotkey::apply_settings_hotkey(&app, &prev.hotkey, &prev.activation_mode).await;
            }
            restore_auxiliary_hotkeys(&app, &prev).await;
            return Err(error);
        }
    }
    if !hotkey::is_suspended() && (hotkey_changed || screen_hotkey_changed) {
        if let Err(error) = hotkey::apply_screen_action_hotkey(&app, &screen_hotkey).await {
            if hotkey_changed {
                let _ =
                    hotkey::apply_settings_hotkey(&app, &prev.hotkey, &prev.activation_mode).await;
            }
            restore_auxiliary_hotkeys(&app, &prev).await;
            return Err(error);
        }
    }
    if !hotkey::is_suspended()
        && (hotkey_changed
            || hotkey::mode_action_binding_changed(
                &prev.verbatim_hotkey,
                &settings.verbatim_hotkey,
                &prev.activation_mode,
                &settings.activation_mode,
            ))
    {
        if let Err(error) = hotkey::apply_verbatim_action_hotkey(
            &app,
            &settings.verbatim_hotkey,
            &settings.activation_mode,
        )
        .await
        {
            let _ = hotkey::apply_settings_hotkey(&app, &prev.hotkey, &prev.activation_mode).await;
            restore_auxiliary_hotkeys(&app, &prev).await;
            return Err(error);
        }
    }
    if !hotkey::is_suspended()
        && (hotkey_changed
            || hotkey::mode_action_binding_changed(
                &prev.translation_hotkey,
                &settings.translation_hotkey,
                &prev.activation_mode,
                &settings.activation_mode,
            ))
    {
        if let Err(error) = hotkey::apply_translation_action_hotkey(
            &app,
            &settings.translation_hotkey,
            &settings.activation_mode,
        )
        .await
        {
            let _ = hotkey::apply_settings_hotkey(&app, &prev.hotkey, &prev.activation_mode).await;
            restore_auxiliary_hotkeys(&app, &prev).await;
            return Err(error);
        }
    }
    let mapping_binding_changes =
        changed_mapping_binding_ids(&prev.context_mappings, &settings.context_mappings);
    let mapping_rules_invalidated =
        match store::invalidate_word_rule_mapping_bindings(&dir, &mapping_binding_changes) {
            Ok(changed) => changed > 0,
            Err(error) => {
                if !hotkey::is_suspended() && hotkey_changed {
                    let _ =
                        hotkey::apply_settings_hotkey(&app, &prev.hotkey, &prev.activation_mode)
                            .await;
                }
                if !hotkey::is_suspended() && (hotkey_changed || selected_hotkey_changed) {
                    let _ = hotkey::apply_selected_action_hotkey(
                        &app,
                        &prev.selected_action_hotkey,
                        prev.selected_actions_enabled,
                    )
                    .await;
                }
                restore_auxiliary_hotkeys(&app, &prev).await;
                return Err(format!(
                    "failed to invalidate mapping-bound word rules: {error}"
                ));
            }
        };
    if mapping_rules_invalidated {
        let _ = app.emit("learn_pairs://changed", ());
    }
    let save_result = if dictionary_patch_requested {
        (|| -> Result<_, String> {
            let mut dictionary_database_change = mapping_rules_invalidated;
            for word in explicitly_removed_dictionary_words {
                let cancelled_pending_add =
                    match store::cancel_pending_dictionary_effects_for_word(&dir, &word) {
                        Ok(cancelled) => cancelled,
                        Err(error) => {
                            return Err(if dictionary_database_change {
                                format!(
                                    "{}: {error}",
                                    dictionary_learn::DICTIONARY_SETTINGS_PARTIAL_ERROR
                                )
                            } else {
                                error.to_string()
                            });
                        }
                    };
                dictionary_database_change |= cancelled_pending_add;
            }
            dictionary_learn::save_settings_with_dictionary_effects(
                &dir,
                &mut settings,
                dictionary_database_change,
                true,
                |settings| {
                    if require_service_readiness {
                        store::save_settings(&dir, settings)
                    } else {
                        store::save_configuration(&dir, settings)
                    }
                },
            )
        })()
    } else if mapping_rules_invalidated {
        dictionary_learn::save_settings_with_dictionary_effects(
            &dir,
            &mut settings,
            true,
            true,
            |settings| {
                if require_service_readiness {
                    store::save_settings(&dir, settings)
                } else {
                    store::save_configuration(&dir, settings)
                }
            },
        )
    } else {
        let save_result = if require_service_readiness {
            store::save_settings(&dir, &settings)
        } else {
            store::save_configuration(&dir, &settings)
        };
        if let Err(error) = save_result {
            if !hotkey::is_suspended() && hotkey_changed {
                let _ =
                    hotkey::apply_settings_hotkey(&app, &prev.hotkey, &prev.activation_mode).await;
            }
            if !hotkey::is_suspended() && (hotkey_changed || selected_hotkey_changed) {
                let _ = hotkey::apply_selected_action_hotkey(
                    &app,
                    &prev.selected_action_hotkey,
                    prev.selected_actions_enabled,
                )
                .await;
            }
            restore_auxiliary_hotkeys(&app, &prev).await;
            return Err(if mapping_rules_invalidated {
                format!("{error}; mapping_change_partial_settings_not_saved")
            } else {
                error.to_string()
            });
        }
        Ok(dictionary_learn::DictionarySettingsSaveOutcome {
            saved: true,
            sync_error: None,
        })
    };
    let dictionary_save_outcome = match save_result {
        Ok(outcome) => outcome,
        Err(error) => {
            if !hotkey::is_suspended() && hotkey_changed {
                let _ =
                    hotkey::apply_settings_hotkey(&app, &prev.hotkey, &prev.activation_mode).await;
            }
            if !hotkey::is_suspended() && (hotkey_changed || selected_hotkey_changed) {
                let _ = hotkey::apply_selected_action_hotkey(
                    &app,
                    &prev.selected_action_hotkey,
                    prev.selected_actions_enabled,
                )
                .await;
            }
            restore_auxiliary_hotkeys(&app, &prev).await;
            return Err(if mapping_rules_invalidated {
                format!("{error}; mapping_change_partial_settings_not_saved")
            } else {
                error
            });
        }
    };
    let dictionary_sync_error = dictionary_save_outcome.sync_error;
    if let Some(change) = autostart_change.as_mut() {
        change.commit();
    }
    if asr_key_cleared && !custom_key_cleared {
        crate::keychain::set_asr_api_key("")
            .map_err(|error| format!("failed to remove ASR API key securely: {error}"))?;
        store::clear_asr_key_sidecar(&dir);
    }
    if cleanup_key_cleared && !custom_key_cleared {
        crate::keychain::set_cleanup_api_key("")
            .map_err(|error| format!("failed to remove cleanup API key securely: {error}"))?;
        store::clear_cleanup_key_sidecar(&dir);
    }
    let history_retention_changed = prev.keep_history_days != settings.keep_history_days;
    let audio_retention_changed = prev.keep_audio_days != settings.keep_audio_days;
    if history_retention_changed || audio_retention_changed {
        let root = dir.clone();
        let expected_root = root.clone();
        let keep_history_days = settings.keep_history_days;
        let keep_audio_days = settings.keep_audio_days;
        match audio::run_retained_spool_store_job(
            "settings-audio-retention".into(),
            root,
            0,
            move |lease| {
                if lease.data_root() != expected_root.as_path() {
                    return Err("settings retention job lease does not own the data root".into());
                }
                let mut errors = Vec::new();
                if history_retention_changed {
                    if let Err(error) = store::purge_history(lease.data_root(), keep_history_days) {
                        errors.push(format!("History retention: {error}"));
                    }
                }
                if audio_retention_changed {
                    if let Err(error) = store::purge_gold_audio(lease.data_root(), keep_audio_days)
                    {
                        errors.push(format!("audio retention: {error}"));
                    }
                }
                Ok(errors)
            },
        )
        .await
        {
            Ok(errors) => {
                for error in errors {
                    log::warn!("settings retention cleanup failed: {error}");
                }
            }
            Err(error) => {
                log::warn!("settings retention cleanup remains unconfirmed: {error}");
            }
        }
    }
    if audio_retention_changed {
        revalidate_audio_recovery_after_retention_change(
            app.clone(),
            state,
            settings.keep_audio_days,
        )
        .await;
    }
    let settings_view = store::SettingsView::from(&settings);
    let configuration_changed = commit_settings_snapshot(state, settings.clone());
    drop(binding_edit_guard);
    cancel_invalidated_dictation(
        &app,
        state,
        configuration_changed || mapping_rules_invalidated,
    )
    .await;
    if prev.hud_appearance != settings.hud_appearance || hud_appearance_requested {
        let status = native_glass::configure_and_publish(&app, &settings.hud_appearance);
        if let Some(reason) = status.fallback_reason {
            log::info!(
                "HUD appearance requested={} effective={} fallback={reason}",
                status.requested,
                status.effective
            );
        }
    }
    if cleanup_warmup_configuration_changed {
        schedule_startup_cleanup_warmup(&app, false);
    }
    if prev.always_on_microphone != settings.always_on_microphone
        || prev.input_device != settings.input_device
        || prev.clamshell_microphone != settings.clamshell_microphone
    {
        sync_microphone_warmup(&app, &settings).await;
    }
    let hud_visuals_changed = prev.hud_appearance != settings.hud_appearance
        || prev.hud_color_variant != settings.hud_color_variant
        || prev.hud_processing_color_variant != settings.hud_processing_color_variant
        || prev.hud_voice_style != settings.hud_voice_style
        || prev.hud_processing_style != settings.hud_processing_style
        || (prev.hud_voice_intensity - settings.hud_voice_intensity).abs() > f32::EPSILON;
    if asr_key_cleared || cleanup_key_cleared || hud_visuals_changed || hud_patch_requested {
        let _ = app.emit("settings://changed", settings_view);
    }
    if tray_visibility_changed {
        if let Some(tray) = app.tray_by_id("voiceflow-status") {
            if let Err(error) = tray.set_visible(tray_visible) {
                log::warn!("failed to update tray icon visibility: {error}");
            }
        }
    }
    if context_changed {
        let local_snapshot_changed = {
            let mut current = lock_recover(&state.context);
            apply_context_configuration(
                &mut current,
                context_enabled,
                browser_access_enabled,
                context_mappings,
                writing_modes,
                onboarded,
            )
        };
        if local_snapshot_changed {
            let snapshot = lock_recover(&state.context).snapshot.clone();
            emit_context_snapshot_if_changed(&app, true, &snapshot);
        }
        if context_source_policy_changed {
            invalidate_session_context_after_policy_change(&app, state);
        }
        refresh_context_preview_snapshot(&app, state).await;
    }
    if hotkey::is_suspended() {
        return dictionary_sync_error.map_or(Ok(()), Err);
    }
    dictionary_sync_error.map_or(Ok(()), Err)
}

pub(crate) fn merge_provider_key_patch_values(
    keys: &mut std::collections::BTreeMap<String, String>,
    incoming: &serde_json::Value,
) {
    let Some(map) = incoming.as_object() else {
        return;
    };
    for (id, value) in map {
        if let Some(secret) = value.as_str().filter(|item| !item.trim().is_empty()) {
            keys.insert(id.clone(), secret.to_owned());
        } else if let Some(secret) = value.get("key").and_then(serde_json::Value::as_str) {
            if !secret.trim().is_empty() {
                keys.insert(id.clone(), secret.to_owned());
            }
        }
    }
}

pub(crate) fn validate_settings_patch_fields(
    object: &serde_json::Map<String, serde_json::Value>,
    release: bool,
) -> Result<(), String> {
    const ALLOWED: &[&str] = &[
        "api_key",
        "language",
        "preferred_languages",
        "ui_language",
        "theme",
        "hud_appearance",
        "hud_color_variant",
        "hud_processing_color_variant",
        "hud_voice_style",
        "hud_processing_style",
        "hud_voice_intensity",
        "dictionary",
        "hotkey",
        "activation_mode",
        "chunk_threshold_secs",
        "chunk_length_secs",
        "delivery_policy",
        "keep_audio_days",
        "keep_history_days",
        "keep_success_audio",
        "onboarded",
        "strict_offline_enabled",
        "cleanup_enabled",
        "cleanup_intensity",
        "accurate_asr_provider",
        "accurate_asr_model",
        "accurate_asr_base_url",
        "cascade_timeout_ms",
        "cascade_proper_noun_threshold",
        "window_ocr_enabled",
        "screen_action_hotkey",
        "vision_provider",
        "vision_model",
        "cleanup_model",
        "show_tray_icon",
        "context_enabled",
        "browser_access_enabled",
        "context_mappings",
        "writing_modes",
        "snippets",
        "output_mode",
        "translation_target_language",
        "selected_action_hotkey",
        "selected_actions_enabled",
        "dictionary_learn_enabled",
        "local_edit_learning_enabled",
        "input_device",
        "input_gain",
        "verbatim_hotkey",
        "translation_hotkey",
        "extra_recording_buffer_ms",
        "audio_feedback_enabled",
        "audio_feedback_volume",
        "vad_enabled",
        "always_on_microphone",
        "clamshell_microphone",
        "autostart_enabled",
        "whats_new_last_seen_version",
        "debug_mode",
        "fuzzy_dictionary_enabled",
        "asr_base_url",
        "asr_api_key",
        "asr_model",
        "asr_provider",
        "cleanup_provider",
        "cleanup_base_url",
        "cleanup_api_key",
        "custom_base_url",
        "custom_asr",
        "custom_llm",
        "custom_cleanup_parameters",
        "ollama_base_url",
        "local_whisper_base_url",
        "provider_keys",
        "provider_api_keys",
    ];
    if let Some(unknown) = object.keys().find(|key| !ALLOWED.contains(&key.as_str())) {
        return Err(format!("unsupported settings field: {unknown}"));
    }
    if object.contains_key("context_mappings") {
        return Err(
            "context mappings must be changed through their explicit save/delete commands".into(),
        );
    }
    if release && object.contains_key("always_on_microphone") {
        return Err("unsupported settings field: always_on_microphone".into());
    }
    if let Some(raw_language) = object.get("language") {
        let lang = raw_language
            .as_str()
            .ok_or_else(|| "unsupported recognition language".to_owned())?;
        if crate::languages::canonical_language_code(lang).is_none() {
            return Err(format!("unsupported recognition language: {lang}"));
        }
    }
    if let Some(raw_preferred) = object.get("preferred_languages") {
        let list = raw_preferred
            .as_array()
            .ok_or_else(|| "preferred_languages must be an array".to_owned())?;
        let mut parsed = Vec::with_capacity(list.len());
        for entry in list {
            let item = entry
                .as_str()
                .ok_or_else(|| "preferred_languages entries must be strings".to_owned())?;
            parsed.push(item.to_owned());
        }
        crate::languages::validate_preferred_languages(&parsed)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn update_settings_patch(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    patch: serde_json::Value,
) -> Result<(), SettingsCommandError> {
    let result: Result<(), String> = async {
        let _guard = state.settings_gate.lock().await;
        let object = patch
            .as_object()
            .ok_or_else(|| "settings patch must be an object".to_owned())?;
        validate_settings_patch_fields(object, cfg!(not(debug_assertions)))?;
        let current = lock_recover(&state.settings).clone();
        let mut merged = serde_json::to_value(&current).map_err(|error| error.to_string())?;
        let merged_object = merged
            .as_object_mut()
            .ok_or_else(|| "settings serialization failed".to_owned())?;
        let requested_custom_base = object
            .get("custom_base_url")
            .and_then(serde_json::Value::as_str);
        let custom_host_changed = requested_custom_base.is_some_and(|next| {
            crate::llm::custom_connection_origin_changed(
                &current.resolved_provider_base(crate::providers::EngineProvider::Custom),
                next,
            )
        });
        let explicit_custom_key = object
            .get("provider_keys")
            .or_else(|| object.get("provider_api_keys"))
            .and_then(serde_json::Value::as_object)
            .and_then(|keys| keys.get("custom"))
            .and_then(|value| {
                value
                    .as_str()
                    .or_else(|| value.get("key").and_then(serde_json::Value::as_str))
            })
            .is_some_and(|key| !key.trim().is_empty());
        for (key, value) in object {
            merged_object.insert(key.clone(), value.clone());
        }
        if object.contains_key("asr_base_url") && !object.contains_key("asr_api_key") {
            let next_url = merged_object
                .get("asr_base_url")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let provider = merged_object
                .get("asr_provider")
                .and_then(serde_json::Value::as_str)
                .and_then(crate::providers::EngineProvider::parse)
                .unwrap_or(current.asr_provider);
            let host_changed = if provider == crate::providers::EngineProvider::Custom {
                crate::llm::custom_connection_origin_changed(&current.asr_base_url, next_url)
            } else {
                crate::asr::asr_host_changed(&current.asr_base_url, next_url)
            };
            if host_changed {
                merged_object.insert(
                    "asr_api_key".into(),
                    serde_json::Value::String(String::new()),
                );
            }
        }
        if object.contains_key("cleanup_base_url") && !object.contains_key("cleanup_api_key") {
            let next_url = merged_object
                .get("cleanup_base_url")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let provider = merged_object
                .get("cleanup_provider")
                .and_then(serde_json::Value::as_str)
                .and_then(crate::providers::EngineProvider::parse)
                .unwrap_or(current.cleanup_provider);
            let host_changed = if provider == crate::providers::EngineProvider::Custom {
                crate::llm::custom_connection_origin_changed(&current.cleanup_base_url, next_url)
            } else {
                crate::llm::chat_host_changed(&current.cleanup_base_url, next_url)
            };
            if host_changed {
                merged_object.insert(
                    "cleanup_api_key".into(),
                    serde_json::Value::String(String::new()),
                );
            }
        }
        if let Some(incoming) = object
            .get("provider_keys")
            .or_else(|| object.get("provider_api_keys"))
        {
            let mut keys = current.provider_api_keys.clone();
            merge_provider_key_patch_values(&mut keys, incoming);
            if custom_host_changed && !explicit_custom_key {
                keys.remove("custom");
            }
            merged_object.insert(
                "provider_api_keys".into(),
                serde_json::to_value(keys).unwrap_or(serde_json::json!({})),
            );
            merged_object.remove("provider_keys");
        }
        if custom_host_changed && !explicit_custom_key {
            merged_object.insert(
                "asr_api_key".into(),
                serde_json::Value::String(String::new()),
            );
            merged_object.insert(
                "cleanup_api_key".into(),
                serde_json::Value::String(String::new()),
            );
            if let Some(keys) = merged_object
                .get_mut("provider_api_keys")
                .and_then(serde_json::Value::as_object_mut)
            {
                keys.remove("custom");
            }
        }
        let mut settings: store::Settings =
            serde_json::from_value(merged).map_err(|error| error.to_string())?;
        let effective_model = settings.resolved_asr_model();
        if object.contains_key("language")
            && !crate::languages::language_supported_by_model(
                settings.asr_provider,
                &effective_model,
                &settings.language,
            )
        {
            return Err(format!(
                "recognition language '{}' is not supported by the selected ASR model",
                settings.language
            ));
        }
        if (object.contains_key("asr_provider") || object.contains_key("asr_model"))
            && !crate::languages::language_supported_by_model(
                settings.asr_provider,
                &effective_model,
                &settings.language,
            )
        {
            settings.language = crate::languages::coerce_language_for_model(
                settings.asr_provider,
                &effective_model,
                &settings.language,
            )
            .to_owned();
        }
        restore_session_custom_credential_after_patch(&current, &mut settings, explicit_custom_key);
        if let Some(groq) = settings.provider_api_keys.get("groq").cloned() {
            if !groq.trim().is_empty() {
                settings.api_key = groq;
            }
        }
        if !settings.session_only_custom_credential {
            if let Some(custom) = settings.provider_api_keys.get("custom").cloned() {
                if !custom.trim().is_empty() {
                    settings.asr_api_key = custom.clone();
                    settings.cleanup_api_key = custom;
                }
            }
        }
        if is_binding_configuration_patch(object) {
            return apply_binding_settings(&app, &state, settings, false).await;
        }
        let require_service_readiness = settings_patch_requires_service_readiness(object);
        let hud_appearance_requested = object.contains_key("hud_appearance");
        let hud_patch_requested = hud_appearance_requested
            || object.contains_key("hud_color_variant")
            || object.contains_key("hud_processing_color_variant")
            || object.contains_key("hud_voice_style")
            || object.contains_key("hud_processing_style")
            || object.contains_key("hud_voice_intensity");
        apply_settings(
            app,
            &state,
            settings,
            ApplySettingsOptions {
                autostart_explicitly_requested: object.contains_key("autostart_enabled"),
                require_service_readiness,
                dictionary_patch_requested: object.contains_key("dictionary"),
                hud_appearance_requested,
                hud_patch_requested,
            },
        )
        .await
    }
    .await;
    result.map_err(SettingsCommandError::from_string)
}

#[tauri::command]
pub(crate) async fn remove_provider_key(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    provider: String,
) -> Result<store::SettingsView, SettingsCommandError> {
    let result: Result<store::SettingsView, String> = async {
        let _guard = state.settings_gate.lock().await;
        let parsed = crate::providers::EngineProvider::parse(&provider)
            .ok_or_else(|| format!("unknown provider: {provider}"))?;
        keychain::set_provider_api_key(parsed, "")
            .map_err(|error| format!("failed to remove provider key securely: {error}"))?;
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?;
        let mut settings = lock_recover(&state.settings).clone();
        settings.provider_api_keys.remove(parsed.as_str());
        settings.credential_read_states.insert(
            parsed.keychain_account().into(),
            store::CredentialReadState::Missing,
        );
        store::clear_provider_key_sidecars(&dir, parsed);
        if parsed.is_groq() {
            settings.api_key.clear();
        }
        if parsed.is_custom() {
            settings.asr_api_key.clear();
            settings.cleanup_api_key.clear();
            settings.session_only_custom_credential = false;
            settings.session_custom_credential_scope = None;
            settings.session_custom_credential_rejected = false;
        }
        if settings.asr_provider == parsed
            && !dictation_has_asr_credential(&settings, &dir.join("models"))
        {
            settings.onboarded = false;
        }
        if settings.cleanup_enabled
            && settings.cleanup_provider == parsed
            && settings.cleanup_credential().trim().is_empty()
        {
            let cleanup_base = settings.resolved_provider_base(parsed);
            let local_keyless = matches!(
                parsed,
                crate::providers::EngineProvider::OnDeviceMlx
                    | crate::providers::EngineProvider::OnDeviceLlamaCpp
            ) || parsed.allows_empty_key()
                && crate::providers::is_loopback_url(&cleanup_base);
            if !local_keyless {
                settings.cleanup_enabled = false;
            }
        }
        store::save_settings(&dir, &settings).map_err(|error| error.to_string())?;
        let configuration_changed = commit_settings_snapshot(&state, settings.clone());
        cancel_invalidated_dictation(&app, &state, configuration_changed).await;
        Ok(store::SettingsView::from(&settings))
    }
    .await;
    result.map_err(SettingsCommandError::from_string)
}

#[tauri::command]
pub(crate) async fn clear_all_data(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    include_learned_rules: Option<bool>,
) -> Result<(), SettingsCommandError> {
    let result: Result<(), String> = async {
        let include_learned_rules = include_learned_rules.unwrap_or(false);
        let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        let settings_gate = state.settings_gate.lock().await;
        if !try_claim_operation(&state, OperationLease::DataClear) {
            return Err(
                "Data clear is unavailable while another dictation operation is active.".into(),
            );
        }
        let clear_guard = match store::begin_clear_all_data(&dir) {
            Ok(guard) => guard,
            Err(error) => {
                release_operation(&state, OperationLease::DataClear);
                return Err(error.to_string());
            }
        };
        dictionary_learn::invalidate_learning_observers(&state.learning_observer_epoch);
        drop(settings_gate);
        let operation_guard = Arc::new(DataClearOperationGuard { app: app.clone() });
        let owner_operation_guard = operation_guard.clone();
        let owner_app = app.clone();
        let clear_dir = dir.clone();
        let result =
            audio::run_retained_spool_clear_job("clear-all-data".into(), dir.clone(), move || {
                if let Some(sink) = owner_app.state::<AppState>().dictation_trace_sink.as_ref() {
                    dictation_trace_sink::clear_all_data_under_guard_with_options(
                        &clear_dir,
                        &clear_guard,
                        sink,
                        include_learned_rules,
                    )
                    .map_err(|error| error.to_string())?;
                } else {
                    store::clear_all_data_under_guard_with_options(
                        &clear_dir,
                        &clear_guard,
                        include_learned_rules,
                    )
                    .map_err(|error| error.to_string())?;
                    delivery_diagnostics::clear(&clear_dir).map_err(|error| error.to_string())?;
                }
                // Reset the in-memory audit on the retained owner after durable clear
                // succeeds, but leave event publication to the command thread.
                dictionary_learning_audit::clear_state(&owner_app);
                drop(owner_operation_guard);
                Ok(())
            })
            .await;
        if result.is_ok() {
            if include_learned_rules {
                let _gate = state.settings_gate.lock().await;
                let mut snapshot = lock_recover(&state.settings).clone();
                if snapshot.remove_all_rule_dictionary_sources() {
                    snapshot.normalize();
                    if let Err(error) = store::save_settings(&dir, &snapshot) {
                        return Err(error.to_string());
                    }
                    commit_learning_settings_snapshot(&state, snapshot.clone());
                    let _ = app.emit("settings://changed", store::SettingsView::from(&snapshot));
                }
            }
            dictionary_learning_audit::notify_current(&app);
            let _ = app.emit("learn_pairs://changed", ());
            let _ = app.emit("style_drafts://changed", ());
            let _ = app.emit("writing_preference_drafts://changed", ());
        }
        drop(operation_guard);
        result
    }
    .await;
    result.map_err(SettingsCommandError::from_string)
}

pub(crate) struct DataClearOperationGuard {
    app: tauri::AppHandle,
}

impl Drop for DataClearOperationGuard {
    fn drop(&mut self) {
        let state = self.app.state::<AppState>();
        release_operation(&state, OperationLease::DataClear);
    }
}
#[tauri::command]
pub(crate) async fn probe_engine_draft(
    state: State<'_, AppState>,
    draft: engine::EngineDraft,
) -> Result<engine::ProbeResult, SettingsCommandError> {
    let result: Result<engine::ProbeResult, String> = async {
        if state.exit_state.load(Ordering::Acquire) != 0 {
            return Err("VoiceFlow is shutting down.".into());
        }
        let stored = lock_recover(&state.settings).clone();
        let probes_native_cleanup = draft.cleanup_provider
            == crate::providers::EngineProvider::OnDeviceMlx
            && draft.probe_stage != engine::ProbeScope::Asr
            && (draft.cleanup_enabled || draft.probe_stage == engine::ProbeScope::Cleanup);
        let result = engine::probe_engine_draft_with_cleanup_demand_and_gguf_runtime(
            &draft,
            &stored,
            &state.models_root,
            Some(&state.llama_cleanup_runtime),
            || probes_native_cleanup.then(|| begin_cleanup_model_demand(&state)),
        )
        .await;
        Ok(result)
    }
    .await;
    result.map_err(SettingsCommandError::from_string)
}

#[tauri::command]
pub(crate) async fn validate_api_key(key: String) -> Result<String, String> {
    Ok(groq::validate_key(&key).await)
}
#[tauri::command]
pub(crate) async fn validate_configured_api_key() -> Result<String, String> {
    let key = keychain::get_api_key().ok_or_else(|| "No API key is configured".to_owned())?;
    Ok(groq::validate_key(&key).await)
}

#[tauri::command]
pub(crate) fn get_whats_new_status(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> whats_new::WhatsNewStatus {
    let settings = lock_recover(&state.settings);
    whats_new::whats_new_status(
        &settings.whats_new_last_seen_version,
        app.path().resource_dir().ok().as_deref(),
    )
}
