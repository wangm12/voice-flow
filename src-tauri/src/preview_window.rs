//! Small, capability-scoped window for selected-text and screen-action previews.

use std::future::Future;
use std::time::{Duration, Instant};
use tauri::{Manager, WebviewWindow, WebviewWindowBuilder, Window, WindowEvent};

pub(crate) const LABEL: &str = "preview";
const WAKE_SCRIPT: &str = "window.dispatchEvent(new Event('voiceflow-preview-ready'))";
const DESTROY_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) async fn open(app: &tauri::AppHandle, transaction_id: &str) -> Result<(), String> {
    let state = app.state::<crate::AppState>();
    with_lifecycle(&state.preview_window_lifecycle, async {
        wait_for_destroyed_window(&state).await?;
        if app.get_webview_window(LABEL).is_none() {
            let stale_owner = { crate::lock_recover(&state.preview_window_owner).clone() };
            if let Some(stale_owner) = stale_owner {
                clear_owner_if_matches(&state, &stale_owner);
            }
        }

        let selected_owns_transaction = crate::lock_recover(&state.selected_preview)
            .as_ref()
            .is_some_and(|preview| preview.session.identity.transaction_id == transaction_id);
        let screen_owns_transaction = crate::lock_recover(&state.screen_preview)
            .as_ref()
            .is_some_and(|preview| preview.session.identity.transaction_id == transaction_id);
        if !selected_owns_transaction && !screen_owns_transaction {
            return Err("preview transaction is no longer current".into());
        }
        let target_pid = crate::lock_recover(&state.selected_preview)
            .as_ref()
            .filter(|preview| preview.session.identity.transaction_id == transaction_id)
            .map(|preview| preview.session.target_guard.pid)
            .or_else(|| {
                crate::lock_recover(&state.screen_preview)
                    .as_ref()
                    .filter(|preview| preview.session.identity.transaction_id == transaction_id)
                    .map(|preview| preview.session.target_guard.pid)
            });

        {
            let mut owner = crate::lock_recover(&state.preview_window_owner);
            if owner
                .as_deref()
                .is_some_and(|current| current != transaction_id)
            {
                return Err("another preview transaction still owns the window".into());
            }
            *owner = Some(transaction_id.to_owned());
        }
        *crate::lock_recover(&state.preview_window_target_pid) =
            target_pid.map(|pid| (transaction_id.to_owned(), pid));

        let window = match app.get_webview_window(LABEL) {
            Some(window) => Ok(window),
            None => {
                let config = app
                    .config()
                    .app
                    .windows
                    .iter()
                    .find(|config| config.label == LABEL)
                    .ok_or_else(|| "preview window config is missing".to_owned())?;
                WebviewWindowBuilder::from_config(app, config)
                    .map_err(|error| error.to_string())?
                    .on_page_load(|window, payload| {
                        if window.label() == LABEL
                            && payload.event() == tauri::webview::PageLoadEvent::Finished
                        {
                            let _ = window.eval(WAKE_SCRIPT);
                        }
                    })
                    .build()
                    .map_err(|error| error.to_string())
            }
        };
        let window = match window {
            Ok(window) => window,
            Err(error) => {
                clear_owner_if_matches(&state, transaction_id);
                return Err(error);
            }
        };

        configure_collection_behavior(&window);
        let result = window
            .show()
            .and_then(|()| window.set_focus())
            .map_err(|error| error.to_string());
        if let Err(error) = result {
            clear_owner_if_matches(&state, transaction_id);
            let _ = request_destroy_and_wait(&state, &window).await;
            return Err(error);
        }
        // Initial bootstrap plus this fixed DOM signal cover both first load
        // and a reused window without granting access to Tauri's global event API.
        let _ = window.eval(WAKE_SCRIPT);
        Ok(())
    })
    .await
}

pub(crate) async fn destroy_if_owner(app: &tauri::AppHandle, transaction_id: &str) {
    let _ = destroy_if_owner_then(app, transaction_id, || {}).await;
}

/// Destroys this transaction's preview and runs focus restoration before a
/// later preview can acquire the shared native window.
pub(crate) async fn destroy_if_owner_then(
    app: &tauri::AppHandle,
    transaction_id: &str,
    after_destroyed: impl FnOnce(),
) -> bool {
    let state = app.state::<crate::AppState>();
    with_lifecycle(&state.preview_window_lifecycle, async {
        let close = async {
            if let Some(window) = app.get_webview_window(LABEL) {
                request_destroy_and_wait(&state, &window).await
            } else {
                Ok(())
            }
        };
        match destroy_owned_transaction(&state.preview_window_owner, transaction_id, close, || {
            clear_target_pid_if_matches(&state, transaction_id);
            after_destroyed();
        })
        .await
        {
            Ok(destroyed) => destroyed,
            Err(error) => {
                log::warn!("failed to destroy preview window: {error}");
                false
            }
        }
    })
    .await
}

async fn request_destroy_and_wait(
    state: &crate::AppState,
    window: &WebviewWindow,
) -> Result<(), String> {
    state
        .preview_window_close_pending
        .store(true, std::sync::atomic::Ordering::Release);
    let mut destroyed = Box::pin(state.preview_window_destroyed.notified());
    destroyed.as_mut().enable();
    if let Err(error) = window.destroy() {
        state
            .preview_window_close_pending
            .store(false, std::sync::atomic::Ordering::Release);
        return Err(error.to_string());
    }
    if tokio::time::timeout(DESTROY_TIMEOUT, &mut destroyed)
        .await
        .is_err()
    {
        return Err("timed out waiting for preview window destruction".into());
    }
    Ok(())
}

async fn wait_for_destroyed_window(state: &crate::AppState) -> Result<(), String> {
    let deadline = Instant::now() + DESTROY_TIMEOUT;
    while state
        .preview_window_close_pending
        .load(std::sync::atomic::Ordering::Acquire)
    {
        let mut destroyed = Box::pin(state.preview_window_destroyed.notified());
        destroyed.as_mut().enable();
        if !state
            .preview_window_close_pending
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero()
            || tokio::time::timeout(remaining, &mut destroyed)
                .await
                .is_err()
        {
            return Err("the previous preview window is still closing".into());
        }
    }
    Ok(())
}

async fn with_lifecycle<T>(gate: &tokio::sync::Mutex<()>, future: impl Future<Output = T>) -> T {
    let _guard = gate.lock().await;
    future.await
}

fn clear_owner_if_matches(state: &crate::AppState, transaction_id: &str) {
    let mut owner = crate::lock_recover(&state.preview_window_owner);
    if take_owner_if_matches(&mut owner, transaction_id) {
        clear_target_pid_if_matches(state, transaction_id);
    }
}

fn clear_target_pid_if_matches(state: &crate::AppState, transaction_id: &str) {
    let mut target = crate::lock_recover(&state.preview_window_target_pid);
    if target
        .as_ref()
        .is_some_and(|(owner, _)| owner == transaction_id)
    {
        target.take();
    }
}

fn take_owner_if_matches(owner: &mut Option<String>, transaction_id: &str) -> bool {
    if owner.as_deref() == Some(transaction_id) {
        owner.take();
        true
    } else {
        false
    }
}

async fn destroy_owned_transaction(
    owner: &std::sync::Mutex<Option<String>>,
    transaction_id: &str,
    close_window: impl Future<Output = Result<(), String>>,
    after_destroyed: impl FnOnce(),
) -> Result<bool, String> {
    if crate::lock_recover(owner).as_deref() != Some(transaction_id) {
        return Ok(false);
    }
    close_window.await?;
    let released = {
        let mut owner = crate::lock_recover(owner);
        take_owner_if_matches(&mut owner, transaction_id)
    };
    if released {
        after_destroyed();
    }
    Ok(released)
}

pub(crate) fn handle_destroyed(window: &Window, event: &WindowEvent) {
    if window.label() == LABEL && matches!(event, WindowEvent::Destroyed) {
        if let Some(state) = window.app_handle().try_state::<crate::AppState>() {
            state
                .preview_window_close_pending
                .store(false, std::sync::atomic::Ordering::Release);
            state.preview_window_destroyed.notify_one();
        }
    }
}

/// Returns true when the preview close request was claimed for transaction
/// cancellation. With no retained preview owner, the OS close proceeds.
pub(crate) fn handle_close_requested(window: &Window, event: &WindowEvent) -> bool {
    if window.label() != LABEL {
        return false;
    }
    let WindowEvent::CloseRequested { api, .. } = event else {
        return false;
    };
    let app = window.app_handle().clone();
    let Some(state) = app.try_state::<crate::AppState>() else {
        return false;
    };
    let owner = crate::lock_recover(&state.preview_window_owner).clone();
    let Some(transaction_id) = owner else {
        // A hidden, unowned window may close normally; do not recursively
        // prevent-close and call close() again.
        return false;
    };
    api.prevent_close();
    tauri::async_runtime::spawn(async move {
        crate::cancel_preview_from_native_close(&app, &transaction_id).await;
        destroy_if_owner(&app, &transaction_id).await;
    });
    true
}

#[allow(deprecated)]
fn configure_collection_behavior(window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        let window = window.clone();
        let native_window = window.clone();
        let _ = window.run_on_main_thread(move || {
            use objc::{msg_send, sel, sel_impl};
            use tauri_nspanel::cocoa::appkit::NSWindowCollectionBehavior as CollectionBehavior;

            let Ok(pointer) = native_window.ns_window() else {
                return;
            };
            let native_window = pointer as *mut objc::runtime::Object;
            unsafe {
                let behavior: usize = msg_send![native_window, collectionBehavior];
                let conflicting = CollectionBehavior::NSWindowCollectionBehaviorMoveToActiveSpace
                    | CollectionBehavior::NSWindowCollectionBehaviorFullScreenPrimary
                    | CollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary;
                // FullScreenNone is bit 9 in the public enum, though cocoa 0.26
                // does not expose a named constant for it.
                let clear_mask = conflicting.bits() as usize | (1 << 9);
                let set_mask = CollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces.bits()
                    as usize
                    | CollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary.bits()
                        as usize;
                let behavior = (behavior & !clear_mask) | set_mask;
                let _: () = msg_send![native_window, setCollectionBehavior: behavior];
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

#[cfg(test)]
mod tests {
    use super::{destroy_owned_transaction, take_owner_if_matches, with_lifecycle};
    use std::sync::{Arc, Mutex};
    use tokio::sync::Notify;

    #[test]
    fn stale_terminal_close_cannot_clear_a_newer_preview_owner() {
        let mut owner = Some("new-transaction".to_owned());

        assert!(!take_owner_if_matches(&mut owner, "old-transaction"));
        assert_eq!(owner.as_deref(), Some("new-transaction"));
        assert!(take_owner_if_matches(&mut owner, "new-transaction"));
        assert_eq!(owner, None);
    }

    #[tokio::test]
    async fn open_waits_for_old_native_destroy_before_reusing_window_owner() {
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        let owner = Arc::new(Mutex::new(Some("old".to_owned())));
        let close_entered = Arc::new(Notify::new());
        let release_close = Arc::new(Notify::new());
        let native_actions = Arc::new(Mutex::new(Vec::new()));

        let close = {
            let gate = Arc::clone(&gate);
            let owner = Arc::clone(&owner);
            let close_entered = Arc::clone(&close_entered);
            let release_close = Arc::clone(&release_close);
            let native_actions = Arc::clone(&native_actions);
            tokio::spawn(async move {
                with_lifecycle(&gate, async {
                    let _ = destroy_owned_transaction(
                        &owner,
                        "old",
                        async {
                            close_entered.notify_one();
                            release_close.notified().await;
                            native_actions.lock().unwrap().push("destroy old");
                            Ok(())
                        },
                        || native_actions.lock().unwrap().push("restore old"),
                    )
                    .await;
                })
                .await;
            })
        };
        close_entered.notified().await;

        let opened = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let open = {
            let gate = Arc::clone(&gate);
            let owner = Arc::clone(&owner);
            let opened = Arc::clone(&opened);
            let native_actions = Arc::clone(&native_actions);
            tokio::spawn(async move {
                with_lifecycle(&gate, async {
                    *owner.lock().unwrap() = Some("new".to_owned());
                    native_actions.lock().unwrap().push("open new");
                    opened.store(true, std::sync::atomic::Ordering::Release);
                })
                .await;
            })
        };
        tokio::task::yield_now().await;
        assert!(!opened.load(std::sync::atomic::Ordering::Acquire));
        release_close.notify_one();
        close.await.unwrap();
        open.await.unwrap();

        assert_eq!(
            *native_actions.lock().unwrap(),
            vec!["destroy old", "restore old", "open new"]
        );
        assert_eq!(owner.lock().unwrap().as_deref(), Some("new"));
    }

    #[tokio::test]
    async fn stale_close_neither_destroys_nor_restores_over_a_new_owner() {
        let owner = std::sync::Mutex::new(Some("new".to_owned()));
        let destroyed = std::sync::atomic::AtomicBool::new(false);
        let restored = std::sync::atomic::AtomicBool::new(false);
        let result = destroy_owned_transaction(
            &owner,
            "old",
            async {
                destroyed.store(true, std::sync::atomic::Ordering::Release);
                Ok(())
            },
            || restored.store(true, std::sync::atomic::Ordering::Release),
        )
        .await
        .unwrap();

        assert!(!result);
        assert!(!destroyed.load(std::sync::atomic::Ordering::Acquire));
        assert!(!restored.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(owner.lock().unwrap().as_deref(), Some("new"));
    }
}
