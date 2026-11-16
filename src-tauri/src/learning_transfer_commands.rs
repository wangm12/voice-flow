//! Native file-picker IPC for explicit, preview-first learning transfer.

use crate::{history_commands, learning_transfer, lock_recover, store, AppState};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager, State};

fn cancelled_export() -> learning_transfer::LearningTransferExportResult {
    learning_transfer::LearningTransferExportResult {
        status: "cancelled",
        global_word_count: 0,
        pair_count: 0,
        preference_count: 0,
        skipped_count: 0,
    }
}

fn canonical_picked_input(path: &Path) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        format!("learning_transfer_invalid_file: cannot inspect selected file: {error}")
    })?;
    if metadata.file_type().is_symlink() {
        return Err(
            "learning_transfer_invalid_file: selected file cannot be a symbolic link".into(),
        );
    }
    std::fs::canonicalize(path).map_err(|error| {
        format!("learning_transfer_invalid_file: cannot resolve selected file: {error}")
    })
}

fn canonical_export_destination(path: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| "learning_transfer_export_failed: invalid destination path".to_owned())?;
    let name = path.file_name().ok_or_else(|| {
        "learning_transfer_export_failed: invalid destination filename".to_owned()
    })?;
    let parent = std::fs::canonicalize(parent).map_err(|error| {
        format!("learning_transfer_export_failed: cannot resolve destination folder: {error}")
    })?;
    Ok(parent.join(name))
}

fn ensure_export_is_user_selected_data(
    destination: &Path,
    app: &tauri::AppHandle,
) -> Result<(), String> {
    if !destination
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        return Err("learning_transfer_export_failed: choose a .json destination".into());
    }
    let data_dir = std::fs::canonicalize(app_data_dir(app)?).map_err(|error| {
        format!("learning_transfer_export_failed: cannot verify application data folder: {error}")
    })?;
    if destination.starts_with(data_dir) {
        return Err(
            "learning_transfer_export_failed: destination is inside VoiceFlow application data"
                .into(),
        );
    }
    Ok(())
}

fn app_data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|error| error.to_string())
}

fn read_pair_snapshot(dir: &Path) -> Result<Vec<store::LearnPairRecord>, String> {
    let path = dir.join("history.sqlite");
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("cannot inspect local learning store: {error}")),
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            store::list_learn_pairs(dir).map_err(|error| error.to_string())
        }
        Ok(_) => Err("local learning store is not a regular file".into()),
    }
}

#[tauri::command]
pub(crate) async fn export_learning_transfer(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<learning_transfer::LearningTransferExportResult, String> {
    if state.exit_state.load(Ordering::Acquire) != 0 {
        return Err("VoiceFlow is shutting down.".into());
    }

    use tauri_plugin_dialog::DialogExt;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("VoiceFlow Learning Transfer", &["json"])
        .set_file_name("voiceflow-learning-transfer.json")
        .save_file(move |selected| {
            let selected = selected
                .map(|path| {
                    path.into_path()
                        .map_err(|error| format!("learning_transfer_export_failed: {error}"))
                })
                .transpose();
            let _ = sender.send(selected);
        });
    let Some(selected) = receiver.await.map_err(|_| {
        "learning_transfer_export_failed: native save picker did not return".to_owned()
    })??
    else {
        return Ok(cancelled_export());
    };
    let destination = canonical_export_destination(&selected)?;
    ensure_export_is_user_selected_data(&destination, &app)?;

    let _settings_gate = state.settings_gate.lock().await;
    if state.exit_state.load(Ordering::Acquire) != 0 {
        return Err("VoiceFlow is shutting down.".into());
    }
    let dir = app_data_dir(&app)?;
    let settings = lock_recover(&state.settings).clone();
    let (file, skipped_count) = tauri::async_runtime::spawn_blocking(move || {
        let pairs = read_pair_snapshot(&dir).map_err(|error| {
            format!("learning_transfer_export_failed: cannot read approved rules: {error}")
        })?;
        let (file, skipped_count) =
            learning_transfer::build_export_file(&settings, &settings.context_mappings, &pairs);
        let bytes = learning_transfer::make_transfer_file_bytes(&file)?;
        learning_transfer::parse_transfer_file(&bytes).map_err(|error| {
            format!("learning_transfer_export_failed: generated file failed validation: {error}")
        })?;
        learning_transfer::write_export_file(&destination, &bytes)?;
        Ok::<_, String>((file, skipped_count))
    })
    .await
    .map_err(|error| format!("learning_transfer_export_failed: {error}"))??;
    Ok(learning_transfer::LearningTransferExportResult {
        status: "exported",
        global_word_count: file.global_words.len(),
        pair_count: file.word_pairs.len(),
        preference_count: file.writing_preferences.len(),
        skipped_count,
    })
}

#[tauri::command]
pub(crate) async fn preview_learning_transfer(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<learning_transfer::LearningTransferPreviewResponse, String> {
    if state.exit_state.load(Ordering::Acquire) != 0 {
        return Err("VoiceFlow is shutting down.".into());
    }

    use tauri_plugin_dialog::DialogExt;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("VoiceFlow Learning Transfer", &["json"])
        .pick_file(move |selected| {
            let selected = selected
                .map(|path| {
                    path.into_path()
                        .map_err(|error| format!("learning_transfer_invalid_file: {error}"))
                })
                .transpose();
            let _ = sender.send(selected);
        });
    let Some(selected) = receiver.await.map_err(|_| {
        "learning_transfer_invalid_file: native open picker did not return".to_owned()
    })??
    else {
        return Ok(learning_transfer::cancelled_preview_response().into());
    };
    let path = canonical_picked_input(&selected)?;
    let _settings_gate = state.settings_gate.lock().await;
    let dir = app_data_dir(&app)?;
    let settings = lock_recover(&state.settings).clone();
    tauri::async_runtime::spawn_blocking(move || {
        let rows = read_pair_snapshot(&dir).map_err(|error| {
            format!("learning_transfer_invalid_file: cannot read local approved rules: {error}")
        })?;
        let loaded = learning_transfer::load_transfer_file(&path)?;
        learning_transfer::build_preview(path, loaded, &settings, &rows)
            .map(learning_transfer::LearningTransferPreviewResponse::from)
    })
    .await
    .map_err(|error| format!("learning_transfer_invalid_file: {error}"))?
}

fn pair_database_changed(outcomes: &[store::LearnPairTransferOutcome]) -> bool {
    outcomes.iter().any(|outcome| {
        matches!(
            outcome,
            store::LearnPairTransferOutcome::Inserted
                | store::LearnPairTransferOutcome::Replaced { .. }
        )
    })
}

fn dictionary_reconciliation_needed(outcomes: &[store::LearnPairTransferOutcome]) -> bool {
    outcomes
        .iter()
        .any(|outcome| matches!(outcome, store::LearnPairTransferOutcome::Replaced { .. }))
}

fn mark_non_pair_settings_failed(
    result: &mut learning_transfer::LearningTransferApplyResult,
    pair_item_ids: &[String],
) {
    let pair_ids = pair_item_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    for item in &mut result.results {
        if !pair_ids.contains(item.item_id.as_str())
            && matches!(
                item.status,
                learning_transfer::TransferItemResultStatus::Imported
            )
        {
            item.status = learning_transfer::TransferItemResultStatus::Failed;
            item.reason = Some("learning_transfer_settings_not_saved".into());
        }
    }
    result.status = "partial";
}

fn mark_pair_items_failed(
    result: &mut learning_transfer::LearningTransferApplyResult,
    pair_item_ids: &[String],
    reason: &str,
) {
    for item in &mut result.results {
        if pair_item_ids.iter().any(|pair_id| pair_id == &item.item_id)
            && matches!(
                item.status,
                learning_transfer::TransferItemResultStatus::Blocked
                    | learning_transfer::TransferItemResultStatus::Failed
            )
        {
            item.status = learning_transfer::TransferItemResultStatus::Failed;
            let is_mapping_creation_failure =
                item.reason.as_deref() == Some("learning_transfer_mapping_created_pair_failed");
            if reason == "learning_transfer_settings_not_saved" || !is_mapping_creation_failure {
                item.reason = Some(reason.to_owned());
            }
        }
    }
    result.status = "partial";
}

#[tauri::command]
pub(crate) async fn apply_learning_transfer(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    preview_id: String,
    choices: Vec<learning_transfer::LearningTransferChoice>,
) -> Result<learning_transfer::LearningTransferApplyResult, String> {
    if state.exit_state.load(Ordering::Acquire) != 0 {
        return Err("VoiceFlow is shutting down.".into());
    }
    // Clear All takes the settings gate before claiming its operation. Claiming
    // this mutation first prevents a clear from passing its owner check while
    // an import waits for the gate.
    let _operation = history_commands::claim_history_mutation(&app, &state)?;
    let _settings_gate = state.settings_gate.lock().await;
    if state.exit_state.load(Ordering::Acquire) != 0 {
        return Err("VoiceFlow is shutting down.".into());
    }

    let dir = app_data_dir(&app)?;
    let settings = lock_recover(&state.settings).clone();
    let rows = read_pair_snapshot(&dir).map_err(|error| {
        format!("learning_transfer_stale_preview: cannot read local rules: {error}")
    })?;
    let input = learning_transfer::LearningTransferApplyInput {
        preview_id,
        choices,
    };
    let prepared = learning_transfer::prepare_apply(input, &settings, &rows, &dir)?;
    let pair_intents = prepared.pair_intents.clone();
    let pair_item_ids = prepared.pair_item_ids.clone();
    let settings_changed = prepared.settings_changed;

    // Save mappings/preferences/global words before writing promoted rows that
    // may reference their local mapping IDs. A Settings failure must not leave
    // a live rule whose target mapping was never persisted.
    let mut pending_sync_error = None;
    if settings_changed {
        match crate::persist_learning_transfer_import(
            &app,
            &state,
            prepared.next_settings.clone(),
            false,
        )
        .await
        {
            Ok(sync_error) => pending_sync_error = sync_error,
            Err(error) => {
                let outcomes = vec![store::LearnPairTransferOutcome::Blocked; pair_intents.len()];
                let mut result = learning_transfer::finish_apply(prepared, &outcomes)?;
                mark_non_pair_settings_failed(&mut result, &pair_item_ids);
                mark_pair_items_failed(
                    &mut result,
                    &pair_item_ids,
                    "learning_transfer_settings_not_saved",
                );
                log::warn!("learning transfer Settings save failed before pair import: {error}");
                return Ok(result);
            }
        }
    }

    let outcomes = match store::apply_learn_pair_transfers(&dir, &pair_intents) {
        Ok(outcomes) => outcomes,
        Err(error) => {
            let outcomes = vec![store::LearnPairTransferOutcome::Blocked; pair_intents.len()];
            let mut result = learning_transfer::finish_apply(prepared, &outcomes)?;
            mark_pair_items_failed(
                &mut result,
                &pair_item_ids,
                "learning_transfer_pair_store_failed",
            );
            if settings_changed {
                result.status = "partial";
            }
            log::warn!("learning transfer pair store failed after Settings save: {error}");
            return Ok(result);
        }
    };
    let database_changed = pair_database_changed(&outcomes);
    let dictionary_reconciliation_needed = dictionary_reconciliation_needed(&outcomes);
    let mut result = learning_transfer::finish_apply(prepared, &outcomes)?;

    if dictionary_reconciliation_needed {
        // Replaced rows may stage Rule-source removals. Reconcile those from
        // the current snapshot so the saved transfer settings are not replayed.
        let current_settings = lock_recover(&state.settings).clone();
        match crate::persist_learning_transfer_import(&app, &state, current_settings, true).await {
            Ok(Some(sync_error)) => pending_sync_error = Some(sync_error),
            Ok(None) => {}
            Err(error) => pending_sync_error = Some(error),
        }
    }
    if pending_sync_error.is_some() {
        for item in &mut result.results {
            if pair_item_ids.iter().any(|pair_id| pair_id == &item.item_id)
                && matches!(
                    item.status,
                    learning_transfer::TransferItemResultStatus::Imported
                )
            {
                item.reason = Some("learning_transfer_dictionary_sync_pending".into());
            }
        }
        result.status = "partial";
        log::warn!("learning transfer dictionary projection needs retry");
    }
    if result.results.iter().any(|item| {
        matches!(
            item.status,
            learning_transfer::TransferItemResultStatus::Blocked
                | learning_transfer::TransferItemResultStatus::Failed
        )
    }) {
        result.status = "partial";
    }

    if database_changed {
        let _ = app.emit("learn_pairs://changed", ());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::canonical_export_destination;
    use std::path::Path;

    #[test]
    fn export_destination_keeps_the_native_selected_filename() {
        let selected = Path::new("/tmp/voiceflow-learning-transfer.json");
        let canonical = canonical_export_destination(selected).unwrap();
        assert_eq!(
            canonical.file_name().unwrap(),
            "voiceflow-learning-transfer.json"
        );
    }
}
