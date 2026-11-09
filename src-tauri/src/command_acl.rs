// Shared build-time and test-time source of registered application IPC command names.
pub(crate) const COMMANDS: &[&str] = &[
    "stop_dictation_from_hud",
    "cancel_dictation_from_hud",
    "get_settings",
    "read_provider_credential_from_keychain",
    "cancel_provider_credential_read",
    "get_guide_acknowledgements",
    "acknowledge_guide",
    "get_dictation_phase",
    "get_delivery_settlement_snapshot",
    "get_whats_new_status",
    "preview_whats_new",
    "get_autostart_status",
    "get_latency_metrics",
    "preview_writing_mode",
    "cancel_writing_preview",
    "suggest_dictionary_entries",
    "add_dictionary_entries",
    "remove_dictionary_word",
    "list_learn_pairs",
    "add_manual_scoped_learn_pair",
    "get_dictionary_learning_audit",
    "list_learned_term_usage",
    "promote_learn_pair",
    "ignore_learn_pair",
    "undo_learn_pair",
    "pin_dictionary_term",
    "list_pinned_terms",
    "list_style_drafts",
    "confirm_style_draft",
    "dismiss_style_draft",
    "read_dictionary_file",
    "get_context_snapshot",
    "get_context_mappings",
    "get_available_applications",
    "get_manual_learn_applications",
    "get_application_from_path",
    "get_context_override",
    "save_context_mapping",
    "delete_context_mapping",
    "set_context_enabled",
    "set_context_override",
    "request_browser_access",
    "update_settings_patch",
    "remove_provider_key",
    "get_history",
    "export_history",
    "export_gold_corpus",
    "clear_all_data",
    "retry_dictation",
    "probe_engine_draft",
    "get_local_cleanup_status",
    "get_lmstudio_cleanup_status",
    "get_cleanup_model_store_status",
    "import_cleanup_model",
    "download_cleanup_model",
    "cancel_cleanup_model_store_operation",
    "delete_cleanup_model_store",
    "list_on_device_models",
    "download_on_device_model",
    "cancel_on_device_download",
    "cancel_on_device_model_load",
    "delete_on_device_model",
    "validate_api_key",
    "validate_configured_api_key",
    "repaste_history",
    "save_history_revision",
    "get_history_revisions",
    "reclean_history",
    "undo_last_delivery",
    "confirm_selected_action_preview",
    "copy_selected_action_preview",
    "cancel_selected_action_preview",
    "confirm_screen_action_preview",
    "copy_screen_action_preview",
    "cancel_screen_action_preview",
    "delete_history",
    "check_permissions",
    "get_audio_input_devices",
    "start_microphone_check",
    "stop_microphone_check",
    "request_microphone_permission",
    "open_privacy_settings",
    "hide_island_if_idle",
    "set_island_learn_interactive",
    "set_island_details_visible",
    "open_hud_destination",
    "open_recovery_destination",
    "restore_audio_recovery_to_history",
    "dismiss_audio_recovery",
    "copy_history_recovery",
    "dismiss_history_recovery",
    "set_hotkeys_suspended",
    "set_onboarding_test_mode",
    "set_onboarding_selected_text",
    "get_onboarding_configuration_generation",
    "prepare_onboarding_external",
    "copy_onboarding_external_fixture",
    "run_onboarding_external",
    "cancel_onboarding_external",
    "confirm_onboarding_external",
    "show_permission_guide",
    "close_permission_guide",
];

#[cfg(test)]
mod tests {
    use super::COMMANDS;
    use std::collections::BTreeSet;
    const MAIN: &[&str] = &[
        "get_settings",
        "read_provider_credential_from_keychain",
        "cancel_provider_credential_read",
        "get_guide_acknowledgements",
        "acknowledge_guide",
        "get_dictation_phase",
        "get_whats_new_status",
        "preview_whats_new",
        "get_autostart_status",
        "get_latency_metrics",
        "preview_writing_mode",
        "cancel_writing_preview",
        "suggest_dictionary_entries",
        "add_dictionary_entries",
        "remove_dictionary_word",
        "list_learn_pairs",
        "add_manual_scoped_learn_pair",
        "get_dictionary_learning_audit",
        "list_learned_term_usage",
        "promote_learn_pair",
        "ignore_learn_pair",
        "undo_learn_pair",
        "pin_dictionary_term",
        "list_pinned_terms",
        "list_style_drafts",
        "confirm_style_draft",
        "dismiss_style_draft",
        "read_dictionary_file",
        "get_context_snapshot",
        "get_context_mappings",
        "get_available_applications",
        "get_manual_learn_applications",
        "get_application_from_path",
        "get_context_override",
        "save_context_mapping",
        "delete_context_mapping",
        "set_context_enabled",
        "set_context_override",
        "request_browser_access",
        "update_settings_patch",
        "remove_provider_key",
        "get_history",
        "export_history",
        "export_gold_corpus",
        "clear_all_data",
        "retry_dictation",
        "probe_engine_draft",
        "get_local_cleanup_status",
        "get_lmstudio_cleanup_status",
        "get_cleanup_model_store_status",
        "import_cleanup_model",
        "download_cleanup_model",
        "cancel_cleanup_model_store_operation",
        "delete_cleanup_model_store",
        "list_on_device_models",
        "download_on_device_model",
        "cancel_on_device_download",
        "cancel_on_device_model_load",
        "delete_on_device_model",
        "validate_api_key",
        "validate_configured_api_key",
        "repaste_history",
        "save_history_revision",
        "get_history_revisions",
        "reclean_history",
        "undo_last_delivery",
        "confirm_selected_action_preview",
        "copy_selected_action_preview",
        "cancel_selected_action_preview",
        "confirm_screen_action_preview",
        "copy_screen_action_preview",
        "cancel_screen_action_preview",
        "delete_history",
        "check_permissions",
        "get_audio_input_devices",
        "start_microphone_check",
        "stop_microphone_check",
        "request_microphone_permission",
        "open_privacy_settings",
        "set_hotkeys_suspended",
        "set_onboarding_test_mode",
        "set_onboarding_selected_text",
        "get_onboarding_configuration_generation",
        "prepare_onboarding_external",
        "copy_onboarding_external_fixture",
        "run_onboarding_external",
        "cancel_onboarding_external",
        "confirm_onboarding_external",
        "show_permission_guide",
        "close_permission_guide",
    ];
    const ISLAND: &[&str] = &[
        "stop_dictation_from_hud",
        "cancel_dictation_from_hud",
        "get_settings",
        "get_delivery_settlement_snapshot",
        "undo_learn_pair",
        "hide_island_if_idle",
        "set_island_learn_interactive",
        "set_island_details_visible",
        "open_hud_destination",
        "open_recovery_destination",
        "restore_audio_recovery_to_history",
        "dismiss_audio_recovery",
        "copy_history_recovery",
        "dismiss_history_recovery",
    ];
    #[test]
    fn registry_matches_handler_source() {
        let source = include_str!("lib.rs");
        let marker = ".invoke_handler(tauri::generate_handler![";
        let start = source.find(marker).expect("handler block") + marker.len();
        assert!(!source[start..].contains(marker));
        let tail = &source[start..];
        let close = tail.find("])").expect("handler close");
        let body = &tail[..close];
        let mut names = Vec::new();
        for (index, raw) in body.split(',').enumerate() {
            let item = raw.trim();
            if item.is_empty() {
                assert_eq!(
                    index + 1,
                    body.split(',').count(),
                    "empty interior handler item"
                );
                continue;
            }
            let parts: Vec<_> = item.split("::").collect();
            assert!(
                (1..=2).contains(&parts.len()),
                "unsupported handler path: {item}"
            );
            assert!(
                parts.iter().all(|p| !p.is_empty()
                    && p.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')),
                "unsupported handler path: {item}"
            );
            names.push(*parts.last().unwrap());
        }
        assert_eq!(names.len(), names.iter().collect::<BTreeSet<_>>().len());
        assert_eq!(
            names.iter().copied().collect::<BTreeSet<_>>(),
            COMMANDS.iter().copied().collect()
        );
        assert_eq!(names, COMMANDS);
        assert!(!names.contains(&"set_settings"));
    }
    fn json(p: &str) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(p).expect("read ACL file"))
            .expect("valid ACL JSON")
    }
    fn perms(v: &serde_json::Value) -> BTreeSet<String> {
        {
            let a = v["permissions"].as_array().expect("permissions array");
            let out: BTreeSet<String> = a
                .iter()
                .map(|x| x.as_str().expect("permission string").to_owned())
                .collect();
            assert_eq!(a.len(), out.len(), "duplicate permission");
            out
        }
    }
    fn source_local(v: &serde_json::Value) -> bool {
        match v.get("local") {
            None => true,
            Some(value) => value.as_bool().expect("local must be a boolean"),
        }
    }
    #[test]
    fn source_capabilities_are_exact_and_window_bound() {
        let main = json("capabilities/default.json");
        let island = json("capabilities/island.json");
        let conf = json("tauri.conf.json");
        assert_eq!(main["identifier"], "default");
        assert_eq!(island["identifier"], "island");
        assert!(source_local(&main));
        assert!(source_local(&island));
        assert!(main.get("remote").is_none());
        assert!(island.get("remote").is_none());
        assert!(main.get("webviews").is_none());
        assert!(island.get("webviews").is_none());
        assert_eq!(main["windows"], serde_json::json!(["main"]));
        assert_eq!(island["windows"], serde_json::json!(["island"]));
        let labels: BTreeSet<_> = conf["app"]["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["label"].as_str().unwrap())
            .collect();
        assert_eq!(labels, ["main", "island"].into_iter().collect());
        let mp = perms(&main);
        let ip = perms(&island);
        let expected_m: BTreeSet<String> = MAIN
            .iter()
            .map(|x| format!("allow-{}", x.replace('_', "-")))
            .chain(
                [
                    "core:event:allow-listen",
                    "core:event:allow-unlisten",
                    "core:event:allow-emit",
                    "core:window:allow-start-dragging",
                    "core:window:allow-internal-toggle-maximize",
                    "dialog:allow-open",
                ]
                .into_iter()
                .map(str::to_owned),
            )
            .collect();
        let expected_i: BTreeSet<String> = ISLAND
            .iter()
            .map(|x| format!("allow-{}", x.replace('_', "-")))
            .chain(
                [
                    "core:event:allow-listen",
                    "core:event:allow-unlisten",
                    "core:event:allow-emit",
                ]
                .into_iter()
                .map(str::to_owned),
            )
            .collect();
        assert_eq!(mp, expected_m);
        assert_eq!(ip, expected_i);
        let ms: BTreeSet<_> = MAIN.iter().copied().collect();
        let is: BTreeSet<_> = ISLAND.iter().copied().collect();
        assert_eq!(ms.len(), 90);
        assert_eq!(is.len(), 14);
        assert_eq!(
            ms.intersection(&is).copied().collect::<BTreeSet<_>>(),
            ["get_settings", "undo_learn_pair"].into_iter().collect()
        );
        assert_eq!(
            ms.union(&is).copied().collect::<BTreeSet<_>>(),
            COMMANDS.iter().copied().collect()
        );
        assert_eq!(ms.union(&is).count(), 102);
        for c in [
            "clear_all_data",
            "update_settings_patch",
            "export_history",
            "export_gold_corpus",
        ] {
            assert!(!is.contains(c));
        }
        assert!(mp.iter().chain(ip.iter()).all(|p| !p.contains('*')));
    }
    #[test]
    fn generated_acl_outputs_bind_registered_commands() {
        let acl = json("gen/schemas/acl-manifests.json");
        let app = acl["__app-acl__"]["permissions"]
            .as_object()
            .expect("generated app permissions");
        assert_eq!(app.len(), 204);
        for c in COMMANDS {
            let allow = &app[&format!("allow-{}", c.replace('_', "-"))]["commands"];
            let deny = &app[&format!("deny-{}", c.replace('_', "-"))]["commands"];
            assert_eq!(allow["allow"], serde_json::json!([*c]));
            assert_eq!(allow["deny"], serde_json::json!([]));
            assert_eq!(deny["deny"], serde_json::json!([*c]));
            assert_eq!(deny["allow"], serde_json::json!([]));
        }
        let resolved = json("gen/schemas/capabilities.json");
        let resolved_keys: BTreeSet<_> = resolved
            .as_object()
            .expect("generated capabilities object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(resolved_keys, ["default", "island"].into_iter().collect());
        for (id, file) in [
            ("default", "capabilities/default.json"),
            ("island", "capabilities/island.json"),
        ] {
            let src = json(file);
            let generated = &resolved[id];
            assert_eq!(perms(generated), perms(&src));
            assert!(source_local(&src));
            assert_eq!(
                generated.get("local").and_then(serde_json::Value::as_bool),
                Some(true),
                "generated capability {id} local"
            );
            for field in ["identifier", "windows", "remote", "webviews", "permissions"] {
                assert_eq!(
                    generated.get(field),
                    src.get(field),
                    "generated capability {id} field {field}"
                );
            }
        }
    }
}
