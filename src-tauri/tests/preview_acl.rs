use tauri::WebviewWindowBuilder;

#[tauri::command(rename = "get_action_preview_bootstrap")]
fn preview_acl_probe_bootstrap() -> &'static str {
    "preview bootstrap probe"
}

#[tauri::command(rename = "get_settings")]
fn preview_acl_probe_settings() -> &'static str {
    "settings probe"
}

#[tauri::command(rename = "get_history")]
fn preview_acl_probe_history() -> &'static str {
    "history probe"
}

#[tauri::command(rename = "read_provider_credential_from_keychain")]
fn preview_acl_probe_keychain() -> &'static str {
    "keychain probe"
}

#[tauri::command(rename = "update_settings_patch")]
fn preview_acl_probe_settings_write() -> &'static str {
    "settings write probe"
}

#[tauri::command(rename = "confirm_selected_action_preview")]
fn preview_acl_probe_confirm_selected() -> &'static str {
    "confirm selected probe"
}

#[tauri::command(rename = "copy_selected_action_preview")]
fn preview_acl_probe_copy_selected() -> &'static str {
    "copy selected probe"
}

#[tauri::command(rename = "cancel_selected_action_preview")]
fn preview_acl_probe_cancel_selected() -> &'static str {
    "cancel selected probe"
}

#[tauri::command(rename = "confirm_screen_action_preview")]
fn preview_acl_probe_confirm_screen() -> &'static str {
    "confirm screen probe"
}

#[tauri::command(rename = "copy_screen_action_preview")]
fn preview_acl_probe_copy_screen() -> &'static str {
    "copy screen probe"
}

#[tauri::command(rename = "cancel_screen_action_preview")]
fn preview_acl_probe_cancel_screen() -> &'static str {
    "cancel screen probe"
}

#[tauri::command(rename = "promote_learn_pair")]
fn preview_acl_probe_promote_learn_pair() -> &'static str {
    "promote learn pair probe"
}

#[test]
fn generated_mock_runtime_allows_preview_ipc_only_on_the_preview_window() {
    let app = tauri::test::mock_builder()
        .invoke_handler(tauri::generate_handler![
            preview_acl_probe_bootstrap,
            preview_acl_probe_settings,
            preview_acl_probe_history,
            preview_acl_probe_keychain,
            preview_acl_probe_settings_write,
            preview_acl_probe_confirm_selected,
            preview_acl_probe_copy_selected,
            preview_acl_probe_cancel_selected,
            preview_acl_probe_confirm_screen,
            preview_acl_probe_copy_screen,
            preview_acl_probe_cancel_screen,
            preview_acl_probe_promote_learn_pair
        ])
        .build(tauri::generate_context!())
        .expect("build the application mock runtime from the generated config");

    let configured_window = |label: &str| {
        let config = app
            .config()
            .app
            .windows
            .iter()
            .find(|config| config.label == label)
            .unwrap_or_else(|| panic!("configured {label} window"));
        WebviewWindowBuilder::from_config(&app, config)
            .expect("build configured window")
            .build()
            .expect("create configured window")
    };
    let preview = configured_window("preview");
    let main = configured_window("main");
    let island = configured_window("island");
    let invoke =
        |window: &tauri::WebviewWindow<tauri::test::MockRuntime>, command: &str, callback: u32| {
            tauri::test::get_ipc_response(
                window,
                tauri::webview::InvokeRequest {
                    cmd: command.into(),
                    callback: tauri::ipc::CallbackFn(callback),
                    error: tauri::ipc::CallbackFn(callback + 1),
                    url: if cfg!(any(windows, target_os = "android")) {
                        "http://tauri.localhost"
                    } else {
                        "tauri://localhost"
                    }
                    .parse()
                    .unwrap(),
                    body: tauri::ipc::InvokeBody::default(),
                    headers: Default::default(),
                    invoke_key: tauri::test::INVOKE_KEY.to_string(),
                },
            )
            .map(|body| body.deserialize::<String>().unwrap())
        };

    assert_eq!(
        invoke(&preview, "get_action_preview_bootstrap", 0).unwrap(),
        "preview bootstrap probe"
    );
    for (callback, command) in [
        "get_settings",
        "get_history",
        "read_provider_credential_from_keychain",
        "update_settings_patch",
    ]
    .into_iter()
    .enumerate()
    {
        let error = invoke(&preview, command, (callback * 2 + 2) as u32).unwrap_err();
        assert!(
            error.to_string().contains("not allowed"),
            "{command} should be denied by the generated preview capability: {error}"
        );
    }
    for (callback, command) in [
        "confirm_selected_action_preview",
        "copy_selected_action_preview",
        "cancel_selected_action_preview",
        "confirm_screen_action_preview",
        "copy_screen_action_preview",
        "cancel_screen_action_preview",
    ]
    .into_iter()
    .enumerate()
    {
        assert!(
            invoke(&preview, command, (callback * 2 + 12) as u32).is_ok(),
            "{command} should be available to the preview capability"
        );
    }
    assert_eq!(invoke(&main, "get_settings", 30).unwrap(), "settings probe");
    let error = invoke(&main, "get_action_preview_bootstrap", 32).unwrap_err();
    assert!(error.to_string().contains("not allowed"));
    assert_eq!(
        invoke(&island, "promote_learn_pair", 34).unwrap(),
        "promote learn pair probe"
    );
    assert_eq!(
        invoke(&main, "promote_learn_pair", 36).unwrap(),
        "promote learn pair probe"
    );
    let preview_promote_err = invoke(&preview, "promote_learn_pair", 38).unwrap_err();
    assert!(preview_promote_err.to_string().contains("not allowed"));
    let island_history_err = invoke(&island, "get_history", 40).unwrap_err();
    assert!(island_history_err.to_string().contains("not allowed"));
}
