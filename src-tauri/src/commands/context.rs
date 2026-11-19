use crate::{
    apply_context_override_configuration, context, dictionary_learn, lock_recover,
    persist_context_state, refresh_context_preview_snapshot, settings_error::SettingsCommandError,
    AppState,
};
use tauri::State;

#[tauri::command]
pub(crate) fn get_context_snapshot(state: State<'_, AppState>) -> context::ContextSnapshot {
    lock_recover(&state.context).snapshot.clone()
}

pub(crate) fn visible_context_mappings(
    mappings: &[context::AppMapping],
) -> Vec<context::AppMapping> {
    mappings
        .iter()
        .filter(|mapping| !mapping.deleted)
        .cloned()
        .collect()
}

pub(crate) fn upsert_context_mapping(
    mappings: &mut Vec<context::AppMapping>,
    mut mapping: context::AppMapping,
    replace_style_examples: bool,
) -> Result<(), String> {
    if mapping.deleted {
        return Err("deleted mapping identities cannot be saved".into());
    }
    if mapping.writing_task_id.is_some() && !mapping.has_task_anchor() {
        return Err("task identity requires an explicit App or site anchor".into());
    }
    if mappings
        .iter()
        .any(|item| item.id == mapping.id && item.deleted)
    {
        return Err("this mapping was deleted; create a new mapping identity".into());
    }
    let matching_indices = mappings
        .iter()
        .enumerate()
        .filter_map(|(index, item)| (item.id == mapping.id).then_some(index))
        .collect::<Vec<_>>();
    if matching_indices.len() > 1 {
        return Err("mapping identity is ambiguous; reload mappings before saving".into());
    }
    if let Some(index) = matching_indices.first().copied() {
        context::merge_saved_preferences(&mappings[index], &mut mapping)?;
        let existing = &mappings[index];
        let mut merged = context::merge_saved_mapping(existing, mapping, replace_style_examples);
        let preference_scope_changed = existing.declared_identity_changed(&merged)
            || existing.writing_task_id != merged.writing_task_id
            || existing.dictionary_learn_enabled != merged.dictionary_learn_enabled;
        if preference_scope_changed && merged.preference_revision <= existing.preference_revision {
            merged.preference_revision = existing.preference_revision.wrapping_add(1).max(1);
        }
        let selector_scope_changed = existing.bundle_id != merged.bundle_id
            || existing.executable != merged.executable
            || existing.browser_host != merged.browser_host
            || existing.browser_path_prefix != merged.browser_path_prefix
            || existing.focused_field != merged.focused_field;
        if selector_scope_changed
            || existing.style_example_input != merged.style_example_input
            || existing.style_example_output != merged.style_example_output
            || existing.style_example_pairs != merged.style_example_pairs
        {
            merged.style_examples_approved = false;
        }
        mappings[index] = merged;
    } else {
        if mapping.preference_revision != 0 {
            return Err("new mappings must start at preference revision zero".into());
        }
        // A renderer cannot manufacture backend review provenance when it
        // introduces a mapping. New values are explicit manual settings.
        for preference in &mut mapping.writing_preferences {
            preference.source = context::WritingPreferenceSource::Manual;
            preference.version = 1;
        }
        mappings.push(mapping);
    }
    Ok(())
}

pub(crate) fn tombstone_context_mapping(mappings: &mut [context::AppMapping], id: &str) -> bool {
    let mut changed = false;
    for mapping in mappings
        .iter_mut()
        .filter(|mapping| mapping.id == id && !mapping.deleted)
    {
        let next_revision = mapping.preference_revision.wrapping_add(1).max(1);
        mapping.scrub_as_deleted_tombstone();
        mapping.preference_revision = next_revision;
        changed = true;
    }
    changed
}

#[tauri::command]
pub(crate) fn get_context_mappings(state: State<'_, AppState>) -> Vec<context::AppMapping> {
    visible_context_mappings(&lock_recover(&state.context).mappings)
}
#[tauri::command]
pub(crate) fn get_available_applications() -> Vec<context::ApplicationOption> {
    context::available_applications()
}
#[tauri::command]
pub(crate) fn get_manual_learn_applications() -> Vec<context::ApplicationOption> {
    dictionary_learn::manual_learn_applications()
}
#[tauri::command]
pub(crate) fn get_application_from_path(
    path: String,
) -> Result<context::ApplicationOption, SettingsCommandError> {
    context::application_from_path(&path).map_err(SettingsCommandError::from_string)
}
#[tauri::command]
pub(crate) fn get_context_override(state: State<'_, AppState>) -> Option<context::ContextFamily> {
    lock_recover(&state.context).manual_override
}
#[tauri::command]
pub(crate) async fn save_context_mapping(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    mut mapping: context::AppMapping,
    replace_style_examples: Option<bool>,
) -> Result<Vec<context::AppMapping>, SettingsCommandError> {
    let result: Result<Vec<context::AppMapping>, String> = async {
        if mapping.deleted {
            return Err("deleted mapping identities cannot be saved".into());
        }
        if let Some(host) = &mapping.browser_host {
            mapping.browser_host = context::normalize_host(host);
        }
        if let Some(path) = &mapping.browser_path_prefix {
            mapping.browser_path_prefix = context::normalize_path_prefix(path);
        }
        mapping.validate()?;
        let _guard = state.settings_gate.lock().await;
        let (enabled, browser_access_enabled, mut mappings) = {
            let current = lock_recover(&state.context);
            (
                current.enabled,
                current.browser_access_enabled,
                current.mappings.clone(),
            )
        };
        upsert_context_mapping(
            &mut mappings,
            mapping,
            replace_style_examples.unwrap_or(false),
        )?;
        let visible_mappings = visible_context_mappings(&mappings);
        persist_context_state(
            &app,
            &state,
            enabled,
            browser_access_enabled,
            mappings.clone(),
        )
        .await?;
        Ok(visible_mappings)
    }
    .await;
    result.map_err(SettingsCommandError::from_string)
}
#[tauri::command]
pub(crate) async fn delete_context_mapping(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<context::AppMapping>, SettingsCommandError> {
    let result: Result<Vec<context::AppMapping>, String> = async {
        let _guard = state.settings_gate.lock().await;
        let (enabled, browser_access_enabled, mut mappings) = {
            let current = lock_recover(&state.context);
            (
                current.enabled,
                current.browser_access_enabled,
                current.mappings.clone(),
            )
        };
        tombstone_context_mapping(&mut mappings, &id);
        let visible_mappings = visible_context_mappings(&mappings);
        persist_context_state(
            &app,
            &state,
            enabled,
            browser_access_enabled,
            mappings.clone(),
        )
        .await?;
        Ok(visible_mappings)
    }
    .await;
    result.map_err(SettingsCommandError::from_string)
}
#[tauri::command]
pub(crate) async fn set_context_enabled(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), SettingsCommandError> {
    let result: Result<(), String> = async {
        let _guard = state.settings_gate.lock().await;
        let (browser_access_enabled, mappings) = {
            let current = lock_recover(&state.context);
            (current.browser_access_enabled, current.mappings.clone())
        };
        persist_context_state(&app, &state, enabled, browser_access_enabled, mappings).await
    }
    .await;
    result.map_err(SettingsCommandError::from_string)
}
#[tauri::command]
pub(crate) async fn set_context_override(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    family: Option<context::ContextFamily>,
) -> Result<context::ContextSnapshot, SettingsCommandError> {
    let result: Result<context::ContextSnapshot, String> = async {
        let _guard = state.settings_gate.lock().await;
        {
            let mut current = lock_recover(&state.context);
            apply_context_override_configuration(&mut current, family, false);
        }
        refresh_context_preview_snapshot(&app, &state).await;
        Ok(lock_recover(&state.context).snapshot.clone())
    }
    .await;
    result.map_err(SettingsCommandError::from_string)
}
#[tauri::command]
pub(crate) async fn request_browser_access(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, SettingsCommandError> {
    let result: Result<String, String> = async {
        let _guard = state.settings_gate.lock().await;
        let (enabled, mappings) = {
            let current = lock_recover(&state.context);
            (current.enabled, current.mappings.clone())
        };
        persist_context_state(&app, &state, enabled, true, mappings).await?;
        // The OS automation prompt can only be triggered by querying a browser.
        // Settings is usually the frontmost app when this button is clicked, so
        // enable the user's intent here and let the next browser-focused refresh
        // perform the actual, scoped query.
        Ok("pending".into())
    }
    .await;
    result.map_err(SettingsCommandError::from_string)
}
