use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
#[cfg(test)]
use tokio::sync::{Mutex as TestMutex, MutexGuard as TestMutexGuard};
use tokio_util::sync::CancellationToken;

static STRICT_OFFLINE: AtomicBool = AtomicBool::new(false);
static CLOUD_REQUESTS: OnceLock<Mutex<CancellationToken>> = OnceLock::new();

pub const STRICT_OFFLINE_MESSAGE: &str =
    "Strict offline mode blocks cloud requests. Use local ASR or disable strict offline mode.";

pub fn set_strict_offline(enabled: bool) {
    let previous = STRICT_OFFLINE.swap(enabled, Ordering::AcqRel);
    if previous == enabled {
        return;
    }
    let token = CLOUD_REQUESTS.get_or_init(|| Mutex::new(CancellationToken::new()));
    let mut token = token
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if enabled {
        token.cancel();
    } else {
        *token = CancellationToken::new();
    }
}

pub fn is_strict_offline() -> bool {
    STRICT_OFFLINE.load(Ordering::Acquire)
}

pub fn ensure_cloud_allowed() -> Result<(), &'static str> {
    if is_strict_offline() {
        Err(STRICT_OFFLINE_MESSAGE)
    } else {
        Ok(())
    }
}

/// A shared cancellation token for work that may contact a cloud service.
/// Enabling strict offline mode cancels the current token synchronously, so
/// active HTTP/WebSocket consumers can drop their in-flight futures.
pub fn cloud_request_token() -> CancellationToken {
    CLOUD_REQUESTS
        .get_or_init(|| Mutex::new(CancellationToken::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Cancel in-flight cloud requests during app shutdown. The process is exiting,
/// so the token intentionally remains cancelled and cannot authorize new work.
pub fn cancel_cloud_requests() {
    CLOUD_REQUESTS
        .get_or_init(|| Mutex::new(CancellationToken::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .cancel();
}

#[cfg(test)]
static STRICT_OFFLINE_TEST_LOCK: OnceLock<TestMutex<()>> = OnceLock::new();

/// Serializes tests that mutate the process-wide cloud request policy.
#[cfg(test)]
pub(crate) async fn strict_offline_test_guard() -> TestMutexGuard<'static, ()> {
    STRICT_OFFLINE_TEST_LOCK
        .get_or_init(|| TestMutex::new(()))
        .lock()
        .await
}

/// Acquires the same test lock from a synchronous test outside a Tokio runtime.
#[cfg(test)]
pub(crate) fn strict_offline_test_guard_blocking() -> TestMutexGuard<'static, ()> {
    STRICT_OFFLINE_TEST_LOCK
        .get_or_init(|| TestMutex::new(()))
        .blocking_lock()
}
