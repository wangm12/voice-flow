use crate::{
    commit_settings_snapshot_if_generation, context, keychain, lock_recover, store, AppState,
    OperationLease, Phase,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Manager, State};

#[derive(Clone)]
pub(crate) struct PendingProviderCredentialRead {
    pub(crate) request_id: String,
    pub(crate) settings_generation: u64,
    pub(crate) cancelled: Arc<AtomicBool>,
    pub(crate) provider: crate::providers::EngineProvider,
    pub(crate) purpose: Option<store::CustomCredentialPurpose>,
    pub(crate) custom_origin: Option<String>,
}

pub(crate) const SECURITY_AGENT_BUNDLE_ID: &str = "com.apple.SecurityAgent";
pub(crate) const PROVIDER_CREDENTIAL_READ_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(30);
pub(crate) const PROVIDER_CREDENTIAL_SECURITY_AGENT_GRACE: std::time::Duration =
    std::time::Duration::from_secs(10);
pub(crate) const PROVIDER_CREDENTIAL_FOCUS_RESTORE_GRACE: std::time::Duration =
    std::time::Duration::from_secs(2);
pub(crate) const PROVIDER_CREDENTIAL_FRONTMOST_SETTLE: std::time::Duration =
    std::time::Duration::from_millis(150);

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProviderCredentialReadStatus {
    SessionConfigured,
    Missing,
    Unavailable,
    Cancelled,
    Busy,
    ForegroundRequired,
    Stale,
    Unsupported,
}

#[derive(serde::Serialize)]
pub(crate) struct ProviderCredentialReadResult {
    pub(crate) status: ProviderCredentialReadStatus,
    pub(crate) settings: Option<store::SettingsView>,
}

impl ProviderCredentialReadResult {
    fn status(status: ProviderCredentialReadStatus) -> Self {
        Self {
            status,
            settings: None,
        }
    }
}

pub(crate) fn same_groq_credential_snapshot(
    left: &store::Settings,
    right: &store::Settings,
) -> bool {
    left.api_key == right.api_key
        && left.provider_api_keys.get("groq") == right.provider_api_keys.get("groq")
        && left.credential_baselines.get("groq_api_key")
            == right.credential_baselines.get("groq_api_key")
        && left.unverified_credential_sources.get("settings:api_key")
            == right.unverified_credential_sources.get("settings:api_key")
        && left
            .unverified_credential_sources
            .get("sidecar:groq_api_key")
            == right
                .unverified_credential_sources
                .get("sidecar:groq_api_key")
}

pub(crate) fn same_custom_credential_snapshot(
    left: &store::Settings,
    right: &store::Settings,
    purpose: store::CustomCredentialPurpose,
    captured_origin: &str,
) -> bool {
    let Some(current_scope) = store::custom_credential_recovery_scope(right, purpose) else {
        return false;
    };
    current_scope.origin == captured_origin
        && left.asr_provider == right.asr_provider
        && left.cleanup_provider == right.cleanup_provider
        && left.cleanup_enabled == right.cleanup_enabled
        && left.custom_base_url == right.custom_base_url
        && left.asr_base_url == right.asr_base_url
        && left.cleanup_base_url == right.cleanup_base_url
        && left.session_only_custom_credential == right.session_only_custom_credential
        && left.session_custom_credential_scope == right.session_custom_credential_scope
        && left.session_custom_credential_rejected == right.session_custom_credential_rejected
        && left.credential_read_states.get("provider_custom")
            == right.credential_read_states.get("provider_custom")
        && left.credential_read_states.get("asr_api_key")
            == right.credential_read_states.get("asr_api_key")
        && left.credential_read_states.get("cleanup_api_key")
            == right.credential_read_states.get("cleanup_api_key")
        && left.asr_api_key == right.asr_api_key
        && left.cleanup_api_key == right.cleanup_api_key
        && left.provider_api_keys.get("custom") == right.provider_api_keys.get("custom")
        && left.credential_baselines.get("provider_custom")
            == right.credential_baselines.get("provider_custom")
        && left.credential_baselines.get("asr_api_key")
            == right.credential_baselines.get("asr_api_key")
        && left.credential_baselines.get("cleanup_api_key")
            == right.credential_baselines.get("cleanup_api_key")
        && left
            .unverified_credential_sources
            .get("settings:provider:custom")
            == right
                .unverified_credential_sources
                .get("settings:provider:custom")
        && left
            .unverified_credential_sources
            .get("settings:asr_api_key")
            == right
                .unverified_credential_sources
                .get("settings:asr_api_key")
        && left
            .unverified_credential_sources
            .get("settings:cleanup_api_key")
            == right
                .unverified_credential_sources
                .get("settings:cleanup_api_key")
        && left
            .unverified_credential_sources
            .get("sidecar:provider_custom")
            == right
                .unverified_credential_sources
                .get("sidecar:provider_custom")
        && left
            .unverified_credential_sources
            .get("sidecar:asr_api_key")
            == right
                .unverified_credential_sources
                .get("sidecar:asr_api_key")
        && left
            .unverified_credential_sources
            .get("sidecar:cleanup_api_key")
            == right
                .unverified_credential_sources
                .get("sidecar:cleanup_api_key")
}

pub(crate) fn same_required_provider_credential_snapshot(
    provider: crate::providers::EngineProvider,
    left: &store::Settings,
    right: &store::Settings,
) -> bool {
    let provider_id = provider.as_str();
    let account = provider.keychain_account();
    let settings_source = format!("settings:provider:{provider_id}");
    let sidecar_source = format!("sidecar:{account}");
    left.asr_provider == right.asr_provider
        && left.cleanup_provider == right.cleanup_provider
        && left.cleanup_enabled == right.cleanup_enabled
        && left.provider_api_keys.get(provider_id) == right.provider_api_keys.get(provider_id)
        && left.credential_baselines.get(account) == right.credential_baselines.get(account)
        && left.credential_read_states.get(account) == right.credential_read_states.get(account)
        && left.unverified_credential_sources.get(&settings_source)
            == right.unverified_credential_sources.get(&settings_source)
        && left.unverified_credential_sources.get(&sidecar_source)
            == right.unverified_credential_sources.get(&sidecar_source)
}

pub(crate) fn provider_has_required_cloud_key(provider: crate::providers::EngineProvider) -> bool {
    provider.is_groq() || (!provider.is_custom() && !provider.allows_empty_key())
}

pub(crate) fn same_provider_credential_snapshot(
    provider: crate::providers::EngineProvider,
    purpose: Option<store::CustomCredentialPurpose>,
    custom_origin: Option<&str>,
    left: &store::Settings,
    right: &store::Settings,
) -> bool {
    if provider.is_groq() {
        return purpose.is_none() && same_groq_credential_snapshot(left, right);
    }
    if provider.is_custom() {
        return purpose.zip(custom_origin).is_some_and(|(purpose, origin)| {
            same_custom_credential_snapshot(left, right, purpose, origin)
        });
    }
    purpose.is_none()
        && provider_has_required_cloud_key(provider)
        && same_required_provider_credential_snapshot(provider, left, right)
}

pub(crate) fn provider_credential_read_admission(
    provider: crate::providers::EngineProvider,
    window_label: &str,
    visible: bool,
    focused: bool,
) -> Result<(), ProviderCredentialReadStatus> {
    if !provider_has_required_cloud_key(provider) {
        return Err(ProviderCredentialReadStatus::Unsupported);
    }
    if window_label != "main" || !visible || !focused {
        return Err(ProviderCredentialReadStatus::ForegroundRequired);
    }
    Ok(())
}

pub(crate) fn provider_credential_read_request_admission(
    provider: crate::providers::EngineProvider,
    purpose: Option<store::CustomCredentialPurpose>,
    settings: &store::Settings,
    window_label: &str,
    visible: bool,
    focused: bool,
    operation_lease_idle: bool,
) -> Result<Option<store::CustomCredentialRecoveryScope>, ProviderCredentialReadStatus> {
    if window_label != "main" || !visible || !focused {
        return Err(ProviderCredentialReadStatus::ForegroundRequired);
    }
    if !operation_lease_idle {
        return Err(ProviderCredentialReadStatus::Busy);
    }
    if provider.is_groq() {
        if purpose.is_some() {
            return Err(ProviderCredentialReadStatus::Unsupported);
        }
        provider_credential_read_admission(provider, window_label, visible, focused)?;
        return Ok(None);
    }
    if provider.is_custom() {
        let Some(purpose) = purpose else {
            return Err(ProviderCredentialReadStatus::Unsupported);
        };
        return store::custom_credential_recovery_scope(settings, purpose)
            .map(Some)
            .ok_or(ProviderCredentialReadStatus::Unsupported);
    }
    if provider_has_required_cloud_key(provider) {
        return if purpose.is_none() {
            Ok(None)
        } else {
            Err(ProviderCredentialReadStatus::Unsupported)
        };
    }
    Err(ProviderCredentialReadStatus::Unsupported)
}

pub(crate) fn provider_credential_read_request_id_is_valid(request_id: &str) -> bool {
    let bytes = request_id.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

pub(crate) fn pending_provider_credential_read_matches(
    pending: Option<&PendingProviderCredentialRead>,
    request_id: &str,
    cancelled: &Arc<AtomicBool>,
    settings_generation: u64,
) -> bool {
    !cancelled.load(Ordering::Acquire)
        && pending.is_some_and(|pending| {
            pending.request_id == request_id
                && pending.settings_generation == settings_generation
                && Arc::ptr_eq(&pending.cancelled, cancelled)
                && !pending.cancelled.load(Ordering::Acquire)
        })
}

pub(crate) fn provider_credential_read_is_current(
    state: &AppState,
    request_id: &str,
    cancelled: &Arc<AtomicBool>,
    settings_generation: u64,
) -> bool {
    if cancelled.load(Ordering::Acquire)
        || state.settings_generation.load(Ordering::Acquire) != settings_generation
    {
        return false;
    }
    pending_provider_credential_read_matches(
        lock_recover(&state.pending_provider_credential_read).as_ref(),
        request_id,
        cancelled,
        settings_generation,
    )
}

pub(crate) fn provider_credential_read_gate_admission(
    state: &AppState,
    window: &tauri::WebviewWindow,
    capture: &PendingProviderCredentialRead,
    initial_settings: &store::Settings,
) -> Result<(), keychain::InteractiveApiKeyRead> {
    if capture.cancelled.load(Ordering::Acquire) {
        return Err(keychain::InteractiveApiKeyRead::Cancelled);
    }
    if !provider_credential_read_is_current(
        state,
        &capture.request_id,
        &capture.cancelled,
        capture.settings_generation,
    ) || state.exit_state.load(Ordering::Acquire) != 0
        || window.label() != "main"
        || !window.is_visible().unwrap_or(false)
        || !window.is_focused().unwrap_or(false)
        || lock_recover(&state.manager).phase != Phase::Idle
        || *lock_recover(&state.operation_lease) != OperationLease::Idle
    {
        return Err(keychain::InteractiveApiKeyRead::Stale);
    }

    let current = lock_recover(&state.settings);
    if state.settings_generation.load(Ordering::Acquire) != capture.settings_generation
        || !same_provider_credential_snapshot(
            capture.provider,
            capture.purpose,
            capture.custom_origin.as_deref(),
            initial_settings,
            &current,
        )
    {
        return Err(keychain::InteractiveApiKeyRead::Stale);
    }
    Ok(())
}

pub(crate) fn cancel_pending_provider_credential_read(
    pending: &Mutex<Option<PendingProviderCredentialRead>>,
    request_id: Option<&str>,
) -> bool {
    let pending = lock_recover(pending);
    let Some(active) = pending.as_ref().filter(|active| match request_id {
        Some(request_id) => active.request_id.as_str() == request_id,
        None => true,
    }) else {
        return false;
    };
    active.cancelled.store(true, Ordering::Release);
    true
}

pub(crate) fn provider_credential_blur_is_security_agent(
    frontmost_bundle_id: Option<&str>,
) -> bool {
    frontmost_bundle_id == Some(SECURITY_AGENT_BUNDLE_ID)
}

pub(crate) fn provider_credential_focus_is_confirmed(
    window_focused: bool,
    frontmost_pid: i32,
) -> bool {
    window_focused && frontmost_pid == std::process::id() as i32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProviderCredentialBlurTransition {
    AppForegroundSettle,
    SecurityAgentPrompt,
    AppFocusSettle,
    Returned,
    UnexpectedFrontmost,
}

pub(crate) fn provider_credential_blur_transition(
    window_focused: bool,
    frontmost_pid: i32,
    frontmost_bundle_id: Option<&str>,
    security_agent_seen: bool,
) -> ProviderCredentialBlurTransition {
    let app_pid = std::process::id() as i32;
    if provider_credential_focus_is_confirmed(window_focused, frontmost_pid) {
        ProviderCredentialBlurTransition::Returned
    } else if frontmost_pid == app_pid {
        if security_agent_seen {
            ProviderCredentialBlurTransition::AppFocusSettle
        } else {
            ProviderCredentialBlurTransition::AppForegroundSettle
        }
    } else if provider_credential_blur_is_security_agent(frontmost_bundle_id) {
        ProviderCredentialBlurTransition::SecurityAgentPrompt
    } else {
        ProviderCredentialBlurTransition::UnexpectedFrontmost
    }
}

pub(crate) fn cancel_provider_credential_read_capture(
    app: &tauri::AppHandle,
    capture: &PendingProviderCredentialRead,
) {
    if let Some(state) = app.try_state::<AppState>() {
        cancel_pending_provider_credential_read(
            &state.pending_provider_credential_read,
            Some(&capture.request_id),
        );
    }
}

/// Keep a read alive across focus loss only while macOS's SecurityAgent owns
/// the foreground prompt. Any other app (or unknown frontmost owner) cancels
/// immediately; SecurityAgent gets a short bounded response window.
pub(crate) async fn watch_provider_credential_read_blur(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    capture: PendingProviderCredentialRead,
) {
    let deadline = tokio::time::Instant::now() + PROVIDER_CREDENTIAL_SECURITY_AGENT_GRACE;
    let initial_frontmost_settle_deadline =
        tokio::time::Instant::now() + PROVIDER_CREDENTIAL_FRONTMOST_SETTLE;
    let mut security_agent_seen = false;
    let mut focus_restore_deadline = None;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        if !provider_credential_read_is_current(
            &state,
            &capture.request_id,
            &capture.cancelled,
            capture.settings_generation,
        ) {
            return;
        }
        if state.exit_state.load(Ordering::Acquire) != 0
            || !window.is_visible().unwrap_or(false)
            || lock_recover(&state.manager).phase != Phase::Idle
            || *lock_recover(&state.operation_lease) != OperationLease::Idle
        {
            cancel_provider_credential_read_capture(&app, &capture);
            return;
        }
        let (frontmost_pid, bundle_id) = context::frontmost_application_key();
        let now = tokio::time::Instant::now();
        match provider_credential_blur_transition(
            window.is_focused().unwrap_or(false),
            frontmost_pid,
            bundle_id.as_deref(),
            security_agent_seen,
        ) {
            ProviderCredentialBlurTransition::Returned => return,
            ProviderCredentialBlurTransition::AppForegroundSettle => {
                if now >= initial_frontmost_settle_deadline {
                    cancel_provider_credential_read_capture(&app, &capture);
                    return;
                }
            }
            ProviderCredentialBlurTransition::SecurityAgentPrompt => {
                security_agent_seen = true;
                focus_restore_deadline = None;
                if now >= deadline {
                    cancel_provider_credential_read_capture(&app, &capture);
                    return;
                }
            }
            ProviderCredentialBlurTransition::AppFocusSettle => {
                let restore_deadline = *focus_restore_deadline
                    .get_or_insert_with(|| now + PROVIDER_CREDENTIAL_FOCUS_RESTORE_GRACE);
                if now >= restore_deadline {
                    cancel_provider_credential_read_capture(&app, &capture);
                    return;
                }
            }
            ProviderCredentialBlurTransition::UnexpectedFrontmost => {
                cancel_provider_credential_read_capture(&app, &capture);
                return;
            }
        }
    }
}

pub(crate) async fn provider_credential_focus_returned(
    state: &AppState,
    window: &tauri::WebviewWindow,
    capture: &PendingProviderCredentialRead,
) -> bool {
    let deadline = tokio::time::Instant::now() + PROVIDER_CREDENTIAL_FOCUS_RESTORE_GRACE;
    loop {
        if !provider_credential_read_is_current(
            state,
            &capture.request_id,
            &capture.cancelled,
            capture.settings_generation,
        ) || state.exit_state.load(Ordering::Acquire) != 0
            || !window.is_visible().unwrap_or(false)
            || lock_recover(&state.manager).phase != Phase::Idle
            || *lock_recover(&state.operation_lease) != OperationLease::Idle
        {
            return false;
        }
        let (frontmost_pid, bundle_id) = context::frontmost_application_key();
        if provider_credential_focus_is_confirmed(
            window.is_focused().unwrap_or(false),
            frontmost_pid,
        ) {
            return true;
        }
        if frontmost_pid != std::process::id() as i32
            && !provider_credential_blur_is_security_agent(bundle_id.as_deref())
        {
            return false;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

pub(crate) fn clear_provider_credential_read(state: &AppState, request_id: &str) {
    let mut pending = lock_recover(&state.pending_provider_credential_read);
    if pending
        .as_ref()
        .is_some_and(|active| active.request_id == request_id)
    {
        pending.take();
    }
}

#[tauri::command]
pub(crate) async fn read_provider_credential_from_keychain(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    provider: crate::providers::EngineProvider,
    request_id: String,
    purpose: Option<store::CustomCredentialPurpose>,
) -> Result<ProviderCredentialReadResult, String> {
    if !provider_credential_read_request_id_is_valid(&request_id) {
        return Ok(ProviderCredentialReadResult::status(
            ProviderCredentialReadStatus::Stale,
        ));
    }
    let (initial_settings, settings_generation) = {
        let settings = lock_recover(&state.settings);
        (
            settings.clone(),
            state.settings_generation.load(Ordering::Acquire),
        )
    };
    let custom_scope = match provider_credential_read_request_admission(
        provider,
        purpose,
        &initial_settings,
        window.label(),
        window.is_visible().unwrap_or(false),
        window.is_focused().unwrap_or(false),
        *lock_recover(&state.operation_lease) == OperationLease::Idle,
    ) {
        Ok(scope) => scope,
        Err(status) => return Ok(ProviderCredentialReadResult::status(status)),
    };
    if state.exit_state.load(Ordering::Acquire) != 0
        || lock_recover(&state.manager).phase != Phase::Idle
        || *lock_recover(&state.operation_lease) != OperationLease::Idle
    {
        return Ok(ProviderCredentialReadResult::status(
            ProviderCredentialReadStatus::Busy,
        ));
    }
    let custom_origin = custom_scope.as_ref().map(|scope| scope.origin.clone());
    let cancelled = Arc::new(AtomicBool::new(false));
    let pending_capture;
    {
        let mut pending = lock_recover(&state.pending_provider_credential_read);
        if pending.is_some() {
            return Ok(ProviderCredentialReadResult::status(
                ProviderCredentialReadStatus::Busy,
            ));
        }
        pending_capture = PendingProviderCredentialRead {
            request_id: request_id.clone(),
            settings_generation,
            cancelled: Arc::clone(&cancelled),
            provider,
            purpose,
            custom_origin: custom_origin.clone(),
        };
        *pending = Some(pending_capture.clone());
    }

    let outcome = async {
        if !provider_credential_read_is_current(
            &state,
            &request_id,
            &cancelled,
            settings_generation,
        ) {
            return ProviderCredentialReadResult::status(ProviderCredentialReadStatus::Cancelled);
        }
        let gate_app = window.app_handle().clone();
        let gate_window = window.clone();
        let gate_capture = pending_capture.clone();
        let gate_settings = initial_settings.clone();
        let gate_provider = gate_capture.provider;
        let custom_scope_for_read = custom_scope.clone();
        let read = tokio::time::timeout(
            PROVIDER_CREDENTIAL_READ_TIMEOUT,
            tauri::async_runtime::spawn_blocking(move || {
                let validate_after_gate = move || {
                    let state = gate_app.state::<AppState>();
                    provider_credential_read_gate_admission(
                        &state,
                        &gate_window,
                        &gate_capture,
                        &gate_settings,
                    )
                };
                if gate_provider.is_groq() {
                    keychain::read_groq_api_key_with_user_interaction(validate_after_gate)
                } else if gate_provider.is_custom() {
                    let Some(scope) = custom_scope_for_read else {
                        return keychain::InteractiveApiKeyRead::Stale;
                    };
                    keychain::read_custom_api_key_with_user_interaction(
                        scope.include_asr_legacy,
                        scope.include_cleanup_legacy,
                        validate_after_gate,
                    )
                } else if provider_has_required_cloud_key(gate_provider) {
                    keychain::read_provider_api_key_with_user_interaction(
                        gate_provider,
                        validate_after_gate,
                    )
                } else {
                    keychain::InteractiveApiKeyRead::Unavailable
                }
            }),
        )
        .await;
        let read = match read {
            Ok(read) => read,
            Err(_) => {
                cancelled.store(true, Ordering::Release);
                return ProviderCredentialReadResult::status(
                    ProviderCredentialReadStatus::Cancelled,
                );
            }
        };
        let key = match read {
            Ok(keychain::InteractiveApiKeyRead::Configured(key)) => key,
            Ok(keychain::InteractiveApiKeyRead::Missing) => {
                return ProviderCredentialReadResult::status(ProviderCredentialReadStatus::Missing)
            }
            Ok(keychain::InteractiveApiKeyRead::Cancelled) => {
                return ProviderCredentialReadResult::status(
                    ProviderCredentialReadStatus::Cancelled,
                )
            }
            Ok(keychain::InteractiveApiKeyRead::Stale) => {
                return ProviderCredentialReadResult::status(ProviderCredentialReadStatus::Stale)
            }
            Ok(keychain::InteractiveApiKeyRead::Unavailable) | Err(_) => {
                return ProviderCredentialReadResult::status(
                    ProviderCredentialReadStatus::Unavailable,
                )
            }
        };
        if !provider_credential_focus_returned(&state, &window, &pending_capture).await {
            return ProviderCredentialReadResult::status(ProviderCredentialReadStatus::Stale);
        }

        let _settings_gate = state.settings_gate.lock().await;
        let window_visible = window.is_visible().unwrap_or(false);
        let window_focused = window.is_focused().unwrap_or(false);
        let (frontmost_pid, _) = context::frontmost_application_key();
        let pending = lock_recover(&state.pending_provider_credential_read);
        let request_still_current = pending_provider_credential_read_matches(
            pending.as_ref(),
            &request_id,
            &cancelled,
            settings_generation,
        ) && pending.as_ref().is_some_and(|active| {
            active.provider == provider
                && active.purpose == purpose
                && active.custom_origin == custom_origin
        });
        if !request_still_current
            || state.settings_generation.load(Ordering::Acquire) != settings_generation
            || !window_visible
            || !provider_credential_focus_is_confirmed(window_focused, frontmost_pid)
            || state.exit_state.load(Ordering::Acquire) != 0
            || lock_recover(&state.manager).phase != Phase::Idle
            || *lock_recover(&state.operation_lease) != OperationLease::Idle
        {
            return ProviderCredentialReadResult::status(ProviderCredentialReadStatus::Stale);
        }
        let current = lock_recover(&state.settings).clone();
        if !same_provider_credential_snapshot(
            provider,
            purpose,
            custom_origin.as_deref(),
            &initial_settings,
            &current,
        ) {
            return ProviderCredentialReadResult::status(ProviderCredentialReadStatus::Stale);
        }
        let mut next = current;
        if provider.is_groq() {
            store::adopt_interactive_groq_credential(&mut next, key);
        } else if provider.is_custom() {
            let Some(scope) = custom_scope.clone() else {
                return ProviderCredentialReadResult::status(
                    ProviderCredentialReadStatus::Unsupported,
                );
            };
            store::adopt_interactive_custom_credential(&mut next, key, scope);
        } else if provider_has_required_cloud_key(provider) {
            store::adopt_interactive_provider_credential(&mut next, provider, key);
        } else {
            return ProviderCredentialReadResult::status(ProviderCredentialReadStatus::Unsupported);
        }
        let Some(_) =
            commit_settings_snapshot_if_generation(&state, next, Some(settings_generation))
        else {
            return ProviderCredentialReadResult::status(ProviderCredentialReadStatus::Stale);
        };
        drop(pending);
        ProviderCredentialReadResult {
            status: ProviderCredentialReadStatus::SessionConfigured,
            settings: Some(store::SettingsView::from(&*lock_recover(&state.settings))),
        }
    }
    .await;
    clear_provider_credential_read(&state, &request_id);
    Ok(outcome)
}

#[tauri::command]
pub(crate) fn cancel_provider_credential_read(
    state: State<'_, AppState>,
    request_id: String,
) -> bool {
    if !provider_credential_read_request_id_is_valid(&request_id) {
        return false;
    }
    cancel_pending_provider_credential_read(
        &state.pending_provider_credential_read,
        Some(&request_id),
    )
}
