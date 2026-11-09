//! Secure storage for the Groq API key using the OS-native credential store.
//!
//! - macOS: Keychain
//! - Windows: Credential Manager
//! - Linux: Secret Service (e.g. GNOME Keyring / KWallet)
//!
//! Credentials are stored under fixed service/account pairs so they never
//! touch `settings.json` in plaintext. A one-time migration moves any legacy
//! plaintext API key from the settings file into the keychain.

#[cfg(not(target_os = "macos"))]
use keyring::Entry;

#[cfg(target_os = "macos")]
use core_foundation::base::TCFType;
#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
use core_foundation::data::CFData;
#[cfg(target_os = "macos")]
use core_foundation::string::CFString;
#[cfg(target_os = "macos")]
use security_framework::base::Error as SecurityError;
#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
use security_framework::item::{ItemAddOptions, ItemAddValue, Location};
#[cfg(all(target_os = "macos", not(debug_assertions)))]
use security_framework::item::{ItemClass, ItemSearchOptions, SearchResult};
#[cfg(target_os = "macos")]
use security_framework::os::macos::keychain::SecKeychain;
#[cfg(target_os = "macos")]
use security_framework::passwords::{
    delete_generic_password_options, generic_password, set_generic_password_options,
    PasswordOptions,
};
#[cfg(target_os = "macos")]
use security_framework_sys::access_control::kSecAttrAccessibleAfterFirstUnlock;
#[cfg(target_os = "macos")]
use security_framework_sys::keychain::SecKeychainSetUserInteractionAllowed;
#[cfg(any(test, all(target_os = "macos", not(debug_assertions))))]
const ERR_SEC_MISSING_ENTITLEMENT: i32 = -34018;
#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
#[cfg(target_os = "macos")]
const ERR_SEC_USER_CANCELED: i32 = -128;

// Never reuse an item that may have been created by an ad-hoc build. Those
// items can carry a code-signature ACL and cause macOS to show a login
// keychain prompt after every Cargo rebuild. The current service is a fresh
// Data Protection Keychain item. Legacy app services are intentionally not
// touched: probing them can resurrect the password dialog that this app must
// never show during startup.
#[cfg(any(not(target_os = "macos"), not(debug_assertions)))]
const SERVICE: &str = "com.voiceflow.desktop.credentials.v3";
#[cfg(all(target_os = "macos", debug_assertions))]
// Keep the dev service versioned separately from older ad-hoc entries. Older
// entries may carry an ACL that requires login-keychain authentication; the
// non-interactive dev path intentionally skips those entries and must use a
// fresh service instead.
const FALLBACK_SERVICE: &str = "com.voiceflow.desktop.credentials.dev.v6";
#[cfg(all(target_os = "macos", not(debug_assertions)))]
const LOGIN_FALLBACK_SERVICE: &str = "com.voiceflow.desktop.credentials.login.v3";
#[cfg(not(target_os = "macos"))]
const PREVIOUS_SERVICE: &str = "com.voiceflow.desktop.credentials.v2";
const ACCOUNT: &str = "groq_api_key";
const ASR_ACCOUNT: &str = "asr_api_key";
const CLEANUP_ACCOUNT: &str = "cleanup_api_key";
#[cfg(all(target_os = "macos", not(debug_assertions)))]
const API_SERVICE: &str = SERVICE;
#[cfg(all(target_os = "macos", not(debug_assertions)))]
const API_FALLBACK_SERVICE: &str = LOGIN_FALLBACK_SERVICE;
#[cfg(all(target_os = "macos", debug_assertions))]
const API_SERVICE: &str = FALLBACK_SERVICE;
#[cfg(all(target_os = "macos", debug_assertions))]
const API_FALLBACK_SERVICE: &str = FALLBACK_SERVICE;
#[cfg(not(target_os = "macos"))]
const API_SERVICE: &str = SERVICE;
#[cfg(not(target_os = "macos"))]
const API_FALLBACK_SERVICE: &str = PREVIOUS_SERVICE;
const HISTORY_KEY_ACCOUNT: &str = "history-key";
const HISTORY_KEY_SERVICE: &str = "com.voiceflow.desktop.history-key.v1";
const HISTORY_KEY_FALLBACK_SERVICE: &str = "com.voiceflow.desktop.history-key.dev.v1";
pub(crate) const INSTALLABLE_TEST_BUNDLE_IDENTIFIER: &str = "com.voiceflow.installable-test";
#[cfg(any(not(target_os = "macos"), not(debug_assertions)))]
const INSTALLABLE_TEST_API_SERVICE: &str = "com.voiceflow.installable-test.credentials.v1";
#[cfg(any(not(target_os = "macos"), not(debug_assertions)))]
const INSTALLABLE_TEST_API_FALLBACK_SERVICE: &str =
    "com.voiceflow.installable-test.credentials.fallback.v1";
#[cfg(all(target_os = "macos", debug_assertions))]
const INSTALLABLE_TEST_API_DEV_SERVICE: &str = "com.voiceflow.installable-test.credentials.dev.v1";
#[cfg(any(test, not(target_os = "macos"), not(debug_assertions)))]
const INSTALLABLE_TEST_GROQ_API_SERVICE: &str =
    "com.voiceflow.installable-test.groq-credentials.v7";
#[cfg(any(test, not(target_os = "macos"), not(debug_assertions)))]
const INSTALLABLE_TEST_GROQ_API_FALLBACK_SERVICE: &str =
    "com.voiceflow.installable-test.groq-credentials.fallback.v7";
#[cfg(all(target_os = "macos", debug_assertions))]
const INSTALLABLE_TEST_GROQ_API_DEV_SERVICE: &str =
    "com.voiceflow.installable-test.groq-credentials.dev.v2";
const INSTALLABLE_TEST_HISTORY_KEY_SERVICE: &str = "com.voiceflow.installable-test.history-key.v1";
const INSTALLABLE_TEST_HISTORY_KEY_FALLBACK_SERVICE: &str =
    "com.voiceflow.installable-test.history-key.fallback.v1";

static KEYCHAIN_OPERATION_GATE: std::sync::OnceLock<std::sync::Mutex<()>> =
    std::sync::OnceLock::new();
const KEYCHAIN_OPERATION_WAITING: u8 = 0;
const KEYCHAIN_OPERATION_STARTED: u8 = 1;
const KEYCHAIN_OPERATION_CANCELLED: u8 = 2;

fn keychain_operation_guard() -> std::sync::MutexGuard<'static, ()> {
    KEYCHAIN_OPERATION_GATE
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn begin_queued_keychain_operation(
    state: &std::sync::atomic::AtomicU8,
) -> Option<std::sync::MutexGuard<'static, ()>> {
    if state.load(std::sync::atomic::Ordering::Acquire) != KEYCHAIN_OPERATION_WAITING {
        return None;
    }
    let guard = keychain_operation_guard();
    state
        .compare_exchange(
            KEYCHAIN_OPERATION_WAITING,
            KEYCHAIN_OPERATION_STARTED,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        )
        .ok()
        .map(|_| guard)
}

#[cfg(any(test, target_os = "macos"))]
struct InteractionPolicyRestore<F: FnMut(bool)> {
    previous: bool,
    restore: F,
}

#[cfg(any(test, target_os = "macos"))]
impl<F: FnMut(bool)> InteractionPolicyRestore<F> {
    fn new(previous: bool, restore: F) -> Self {
        Self { previous, restore }
    }
}

#[cfg(any(test, target_os = "macos"))]
impl<F: FnMut(bool)> Drop for InteractionPolicyRestore<F> {
    fn drop(&mut self) {
        (self.restore)(self.previous);
    }
}

#[cfg(target_os = "macos")]
fn set_keychain_interaction_allowed(allowed: bool) -> Result<(), SecurityError> {
    let status = unsafe { SecKeychainSetUserInteractionAllowed(u8::from(allowed)) };
    if status == 0 {
        Ok(())
    } else {
        Err(SecurityError::from_code(status))
    }
}

#[cfg(target_os = "macos")]
fn restore_keychain_interaction_allowed(allowed: bool) {
    if let Err(error) = set_keychain_interaction_allowed(allowed) {
        log::warn!(
            "could not restore keychain interaction policy ({})",
            error.code()
        );
    }
}

#[cfg(target_os = "macos")]
fn keychain_interaction_policy(
    allowed: bool,
) -> Result<InteractionPolicyRestore<fn(bool)>, SecurityError> {
    let previous = SecKeychain::user_interaction_allowed()?;
    set_keychain_interaction_allowed(allowed)?;
    Ok(InteractionPolicyRestore::new(
        previous,
        restore_keychain_interaction_allowed,
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CredentialNamespace {
    Normal,
    InstallableTest,
}

static CREDENTIAL_NAMESPACE: std::sync::OnceLock<CredentialNamespace> = std::sync::OnceLock::new();

/// Bind credential lookups to the application identity supplied by Tauri's
/// resolved runtime configuration. This must run before settings load any
/// API key or create/read the History encryption key.
pub fn configure_runtime_bundle(identifier: &str) -> Result<(), String> {
    let requested = credential_namespace_for_bundle(identifier);
    if let Some(current) = CREDENTIAL_NAMESPACE.get().copied() {
        return bind_credential_namespace(Some(current), requested).map(|_| ());
    }
    match CREDENTIAL_NAMESPACE.set(requested) {
        Ok(()) => Ok(()),
        Err(_) => CREDENTIAL_NAMESPACE
            .get()
            .copied()
            .ok_or_else(|| "credential namespace is not initialized".to_owned())
            .and_then(|current| bind_credential_namespace(Some(current), requested).map(|_| ())),
    }
}

fn bind_credential_namespace(
    current: Option<CredentialNamespace>,
    requested: CredentialNamespace,
) -> Result<CredentialNamespace, String> {
    match current {
        Some(current) if current == requested => Ok(current),
        Some(_) => Err("credential namespace was already bound to another app".into()),
        None => Ok(requested),
    }
}

fn initialized_credential_namespace(
    namespace: Option<CredentialNamespace>,
) -> Result<CredentialNamespace, String> {
    namespace.ok_or_else(|| "credential namespace is not initialized".to_owned())
}

fn credential_namespace_for_bundle(identifier: &str) -> CredentialNamespace {
    if identifier == INSTALLABLE_TEST_BUNDLE_IDENTIFIER {
        CredentialNamespace::InstallableTest
    } else {
        CredentialNamespace::Normal
    }
}

fn api_services_for(namespace: CredentialNamespace) -> (&'static str, &'static str) {
    if namespace == CredentialNamespace::InstallableTest {
        #[cfg(target_os = "macos")]
        {
            #[cfg(debug_assertions)]
            return (
                INSTALLABLE_TEST_API_DEV_SERVICE,
                INSTALLABLE_TEST_API_DEV_SERVICE,
            );
            #[cfg(not(debug_assertions))]
            return (
                INSTALLABLE_TEST_API_SERVICE,
                INSTALLABLE_TEST_API_FALLBACK_SERVICE,
            );
        }
        #[cfg(not(target_os = "macos"))]
        return (
            INSTALLABLE_TEST_API_SERVICE,
            INSTALLABLE_TEST_API_FALLBACK_SERVICE,
        );
    }
    (API_SERVICE, API_FALLBACK_SERVICE)
}

fn api_services() -> Result<(&'static str, &'static str), String> {
    let namespace = initialized_credential_namespace(CREDENTIAL_NAMESPACE.get().copied())?;
    Ok(api_services_for(namespace))
}

fn groq_api_services_for(namespace: CredentialNamespace) -> (&'static str, &'static str) {
    if namespace == CredentialNamespace::InstallableTest {
        #[cfg(all(target_os = "macos", debug_assertions))]
        return (
            INSTALLABLE_TEST_GROQ_API_DEV_SERVICE,
            INSTALLABLE_TEST_GROQ_API_DEV_SERVICE,
        );
        #[cfg(all(target_os = "macos", not(debug_assertions)))]
        return (
            INSTALLABLE_TEST_GROQ_API_SERVICE,
            INSTALLABLE_TEST_GROQ_API_FALLBACK_SERVICE,
        );
        #[cfg(not(target_os = "macos"))]
        return (
            INSTALLABLE_TEST_GROQ_API_SERVICE,
            INSTALLABLE_TEST_GROQ_API_FALLBACK_SERVICE,
        );
    }
    (API_SERVICE, API_FALLBACK_SERVICE)
}

fn groq_api_services() -> Result<(&'static str, &'static str), String> {
    let namespace = initialized_credential_namespace(CREDENTIAL_NAMESPACE.get().copied())?;
    Ok(groq_api_services_for(namespace))
}

fn history_key_services_for(namespace: CredentialNamespace) -> (&'static str, &'static str) {
    if namespace == CredentialNamespace::InstallableTest {
        (
            INSTALLABLE_TEST_HISTORY_KEY_SERVICE,
            INSTALLABLE_TEST_HISTORY_KEY_FALLBACK_SERVICE,
        )
    } else {
        (HISTORY_KEY_SERVICE, HISTORY_KEY_FALLBACK_SERVICE)
    }
}

fn history_key_services() -> Result<(&'static str, &'static str), String> {
    let namespace = initialized_credential_namespace(CREDENTIAL_NAMESPACE.get().copied())?;
    Ok(history_key_services_for(namespace))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyState {
    Configured(String),
    Missing,
    Unavailable(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum InteractiveApiKeyRead {
    Configured(String),
    Missing,
    Cancelled,
    Stale,
    Unavailable,
}

#[cfg(target_os = "macos")]
fn interactive_read_error(error: SecurityError) -> InteractiveApiKeyRead {
    if error.code() == ERR_SEC_USER_CANCELED {
        InteractiveApiKeyRead::Cancelled
    } else {
        InteractiveApiKeyRead::Unavailable
    }
}

#[cfg(target_os = "macos")]
fn read_secret_interactively(
    _service: &str,
    fallback_service: &str,
    account: &str,
) -> InteractiveApiKeyRead {
    #[cfg(debug_assertions)]
    {
        match read_fallback(fallback_service, account, true) {
            Ok(Some(key)) => InteractiveApiKeyRead::Configured(key),
            Ok(None) => InteractiveApiKeyRead::Missing,
            Err(error) => interactive_read_error(error),
        }
    }

    #[cfg(not(debug_assertions))]
    {
        let fallback_after_primary =
            |primary: Option<SecurityError>| match read_fallback(fallback_service, account, true) {
                Ok(Some(key)) => InteractiveApiKeyRead::Configured(key),
                Ok(None) => match primary {
                    Some(error) => interactive_read_error(error),
                    None => InteractiveApiKeyRead::Missing,
                },
                Err(error) => interactive_read_error(error),
            };
        match read_protected(_service, account, true) {
            Ok(Some(key)) => InteractiveApiKeyRead::Configured(key),
            Ok(None) => fallback_after_primary(None),
            Err(error) if error.code() == ERR_SEC_USER_CANCELED => InteractiveApiKeyRead::Cancelled,
            Err(error) if missing_entitlement(error) => fallback_after_primary(None),
            Err(error) => fallback_after_primary(Some(error)),
        }
    }
}

#[cfg(target_os = "macos")]
fn read_groq_secret_interactively(service: &str, fallback_service: &str) -> InteractiveApiKeyRead {
    read_secret_interactively(service, fallback_service, ACCOUNT)
}

fn custom_recovery_accounts(
    include_asr_legacy: bool,
    include_cleanup_legacy: bool,
) -> Vec<&'static str> {
    let mut accounts = vec![crate::providers::EngineProvider::Custom.keychain_account()];
    if include_asr_legacy {
        accounts.push(ASR_ACCOUNT);
    }
    if include_cleanup_legacy {
        accounts.push(CLEANUP_ACCOUNT);
    }
    accounts
}

#[cfg(any(test, target_os = "macos"))]
fn resolve_custom_recovery_accounts(
    accounts: &[&str],
    mut read: impl FnMut(&str) -> InteractiveApiKeyRead,
) -> InteractiveApiKeyRead {
    for account in accounts {
        match read(account) {
            InteractiveApiKeyRead::Configured(key) => {
                return InteractiveApiKeyRead::Configured(key)
            }
            InteractiveApiKeyRead::Missing => {}
            other => return other,
        }
    }
    InteractiveApiKeyRead::Missing
}

/// Read only the fixed Custom account and legacy slots that the caller has
/// proven belong to currently active Custom routes at the same origin.
/// This is memory-only; the existing service namespace and ACLs are unchanged.
pub fn read_custom_api_key_with_user_interaction<F>(
    include_asr_legacy: bool,
    include_cleanup_legacy: bool,
    validate_after_gate: F,
) -> InteractiveApiKeyRead
where
    F: FnOnce() -> Result<(), InteractiveApiKeyRead> + Send + 'static,
{
    if keychain_disabled() {
        return InteractiveApiKeyRead::Unavailable;
    }
    let (service, fallback) = match api_services() {
        Ok(services) => services,
        Err(_) => return InteractiveApiKeyRead::Unavailable,
    };
    let accounts = custom_recovery_accounts(include_asr_legacy, include_cleanup_legacy);
    guarded_interactive_read(validate_after_gate, || {
        #[cfg(target_os = "macos")]
        {
            resolve_custom_recovery_accounts(&accounts, |account| {
                read_secret_interactively(service, fallback, account)
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (service, fallback, accounts);
            InteractiveApiKeyRead::Unavailable
        }
    })
    .unwrap_or_else(|status| status)
}

/// Tests must never touch the real OS keychain by default (it can block on an
/// interactive authorization prompt). Set `VOICEFLOW_TEST_KEYCHAIN_ENABLED=1`
/// only for the ignored manual round-trip test.
#[inline]
fn keychain_disabled() -> bool {
    std::env::var_os("VOICEFLOW_TEST_KEYCHAIN_DISABLED").is_some()
        || (cfg!(test) && std::env::var_os("VOICEFLOW_TEST_KEYCHAIN_ENABLED").is_none())
}

#[cfg(not(target_os = "macos"))]
fn entry(service: &str, account: &str) -> Result<Entry, keyring::Error> {
    Entry::new(service, account)
}

#[cfg(not(target_os = "macos"))]
fn read_keyring(service: &str, account: &str) -> Result<Option<String>, keyring::Error> {
    match entry(service, account)?.get_password() {
        Ok(key) if !key.is_empty() => Ok(Some(key)),
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(not(target_os = "macos"))]
fn read(service: &str, account: &str) -> Result<Option<String>, keyring::Error> {
    read_keyring(service, account)
}

#[cfg(all(target_os = "macos", not(debug_assertions)))]
fn protected_options(service: &str, account: &str) -> PasswordOptions {
    let mut options = PasswordOptions::new_generic_password(service, account);
    // The modern data-protection keychain does not attach a per-build legacy
    // ACL to the item. This is important for `tauri dev`, where Cargo creates
    // a new ad-hoc code signature after a rebuild and the legacy API prompts
    // for the login keychain password again.
    options.use_protected_keychain();
    // Do not add kSecUseAuthenticationUI to SecItemAdd/SecItemUpdate. That
    // query key is valid for item lookup, but macOS rejects it as an invalid
    // write parameter. `set_protected` disables Security.framework UI around
    // the whole write instead, so a stale authenticated item still fails
    // softly without opening a login-keychain prompt.
    options
}

#[cfg(all(target_os = "macos", not(debug_assertions)))]
fn read_protected(
    service: &str,
    account: &str,
    allow_user_interaction: bool,
) -> Result<Option<String>, SecurityError> {
    // Background callers pass false; the foreground Settings recovery action
    // opts in only while holding the shared process-wide operation gate.
    let _interaction_policy = keychain_interaction_policy(allow_user_interaction)?;

    // User interaction remains disabled for this background lookup. Do not
    // skip authenticated items here: that filter also hides credentials this
    // app just wrote to the login keychain.
    let mut options = ItemSearchOptions::new();
    options
        .class(ItemClass::generic_password())
        .service(service)
        .account(account)
        .load_data(true)
        .ignore_legacy_keychains();

    match options.search() {
        Ok(results) => Ok(results.into_iter().find_map(|result| match result {
            SearchResult::Data(bytes) => {
                String::from_utf8(bytes).ok().filter(|key| !key.is_empty())
            }
            _ => None,
        })),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "macos")]
fn read_fallback(
    service: &str,
    account: &str,
    allow_user_interaction: bool,
) -> Result<Option<String>, SecurityError> {
    let _interaction_policy = keychain_interaction_policy(allow_user_interaction)?;

    // Use the same PasswordOptions query used by set_fallback. ItemSearchOptions
    // does not reliably locate login-keychain items created through
    // set_generic_password_options on current macOS releases.
    match generic_password(PasswordOptions::new_generic_password(service, account)) {
        Ok(bytes) => Ok(String::from_utf8(bytes).ok().filter(|key| !key.is_empty())),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(all(target_os = "macos", not(debug_assertions)))]
fn delete_protected(service: &str, account: &str) -> Result<(), SecurityError> {
    let _interaction_policy = keychain_interaction_policy(false)?;

    let mut options = ItemSearchOptions::new();
    options
        .class(ItemClass::generic_password())
        .service(service)
        .account(account)
        .skip_authenticated_items(true)
        .ignore_legacy_keychains();

    match options.delete() {
        Ok(()) => Ok(()),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()), // errSecItemNotFound
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "macos")]
fn delete_fallback(service: &str, account: &str) -> Result<(), SecurityError> {
    let _interaction_policy = keychain_interaction_policy(false)?;

    match delete_generic_password_options(PasswordOptions::new_generic_password(service, account)) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()), // errSecItemNotFound
        Err(error) => Err(error),
    }
}

#[cfg(all(target_os = "macos", not(debug_assertions)))]
fn set_protected(service: &str, account: &str, key: &str) -> Result<(), SecurityError> {
    // A credential-store operation must never be allowed to open an
    // interactive login-keychain prompt. If an item needs authentication,
    // fail softly and let the settings UI ask the user to configure the key
    // again instead.
    let _interaction_policy = keychain_interaction_policy(false)?;
    set_generic_password_options(key.as_bytes(), protected_options(service, account))
}

#[cfg(target_os = "macos")]
fn set_fallback(service: &str, account: &str, key: &str) -> Result<(), SecurityError> {
    // This path is used by unsigned/ad-hoc `tauri dev` builds, which cannot
    // address the Data Protection Keychain because they have no stable team
    // identity entitlement. It still uses SecItem APIs and never enables UI.
    // Replace the old item instead of updating it: the default login-keychain
    // item carries an ACL tied to the ad-hoc code signature, which changes on
    // every Cargo rebuild. The explicit accessibility class keeps the item
    // protected by the unlocked user keychain without that per-build ACL.
    if let Err(error) = delete_fallback(service, account) {
        log::warn!("could not replace login keychain item before write: {error}");
    }
    let _interaction_policy = keychain_interaction_policy(false)?;
    let mut options = PasswordOptions::new_generic_password(service, account);
    #[allow(deprecated)]
    unsafe {
        options.query.push((
            // `security-framework-sys` does not export kSecAttrAccessible;
            // "pdmn" is its documented Keychain attribute key.
            CFString::from("pdmn"),
            CFString::wrap_under_get_rule(kSecAttrAccessibleAfterFirstUnlock).into_CFType(),
        ));
    }
    set_generic_password_options(key.as_bytes(), options)
}

#[cfg(all(target_os = "macos", not(debug_assertions)))]
fn missing_entitlement(error: SecurityError) -> bool {
    missing_entitlement_code(error.code())
        || error
            .to_string()
            .to_ascii_lowercase()
            .contains("entitlement")
}

#[cfg(any(test, all(target_os = "macos", not(debug_assertions))))]
fn missing_entitlement_code(code: i32) -> bool {
    code == ERR_SEC_MISSING_ENTITLEMENT
}

/// Run a keychain operation on a worker thread with a timeout. The OS
/// credential store can block (e.g. authorization prompts, or when the process
/// is launched outside a full GUI session), and we must never let that stall
/// app startup. On timeout we return `None` / a soft error and continue.
fn with_timeout_for<T, F>(timeout: std::time::Duration, op: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    let operation_state =
        std::sync::Arc::new(std::sync::atomic::AtomicU8::new(KEYCHAIN_OPERATION_WAITING));
    let worker_state = std::sync::Arc::clone(&operation_state);
    std::thread::spawn(move || {
        if let Some(_operation_guard) = begin_queued_keychain_operation(&worker_state) {
            let _ = tx.send(op());
        }
    });
    match rx.recv_timeout(timeout) {
        Ok(result) => Some(result),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            let _ = operation_state.compare_exchange(
                KEYCHAIN_OPERATION_WAITING,
                KEYCHAIN_OPERATION_CANCELLED,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            );
            None
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => None,
    }
}

fn with_timeout<T, F>(op: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    with_timeout_for(std::time::Duration::from_millis(800), op)
}

fn with_write_timeout<T, F>(op: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    with_timeout_for(std::time::Duration::from_secs(5), op)
}

#[cfg(any(
    test,
    all(
        feature = "research-harness",
        target_os = "macos",
        not(debug_assertions)
    )
))]
enum WorkerWait<T> {
    Completed(T),
    TimedOut,
    Disconnected,
}

#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
fn with_worker_timeout<T, F>(timeout: std::time::Duration, op: F) -> WorkerWait<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    let operation_state =
        std::sync::Arc::new(std::sync::atomic::AtomicU8::new(KEYCHAIN_OPERATION_WAITING));
    let worker_state = std::sync::Arc::clone(&operation_state);
    std::thread::spawn(move || {
        if let Some(_operation_guard) = begin_queued_keychain_operation(&worker_state) {
            let _ = tx.send(op());
        }
    });
    match rx.recv_timeout(timeout) {
        Ok(value) => WorkerWait::Completed(value),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            let _ = operation_state.compare_exchange(
                KEYCHAIN_OPERATION_WAITING,
                KEYCHAIN_OPERATION_CANCELLED,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            );
            WorkerWait::TimedOut
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => WorkerWait::Disconnected,
    }
}

#[cfg(any(test, not(target_os = "macos"), not(debug_assertions)))]
fn preserve_primary_read_failure<T, E: std::fmt::Display>(
    primary_error: E,
    fallback: Result<Option<T>, E>,
) -> Result<Option<T>, String> {
    match fallback {
        Ok(Some(value)) => Ok(Some(value)),
        Ok(None) => Err(primary_error.to_string()),
        Err(fallback_error) => Err(format!("{primary_error}; fallback: {fallback_error}")),
    }
}

#[allow(clippy::needless_return)]
fn read_stored_secret_with_interaction(
    service: &str,
    fallback_service: &str,
    account: &str,
    allow_user_interaction: bool,
) -> Result<Option<String>, String> {
    #[cfg(not(target_os = "macos"))]
    let _ = allow_user_interaction;
    if keychain_disabled() {
        return Ok(None);
    }
    #[cfg(target_os = "macos")]
    {
        #[cfg(debug_assertions)]
        let _ = service;
        // `tauri dev` is ad-hoc signed and cannot use the Data Protection
        // Keychain's application identity. Keep its credential in a separate
        // service instead of probing a production item and turning a normal
        // development launch into a missing-entitlement error.
        #[cfg(debug_assertions)]
        {
            return read_fallback(fallback_service, account, allow_user_interaction)
                .map_err(|error| error.to_string());
        }

        #[cfg(not(debug_assertions))]
        {
            let primary = read_protected(service, account, allow_user_interaction);
            return match primary {
                Ok(Some(key)) => Ok(Some(key)),
                Ok(None) => read_fallback(fallback_service, account, allow_user_interaction)
                    .map_err(|error| error.to_string()),
                Err(primary_error) if missing_entitlement(primary_error) => {
                    read_fallback(fallback_service, account, allow_user_interaction)
                        .map_err(|error| error.to_string())
                }
                Err(primary_error) => preserve_primary_read_failure(
                    primary_error,
                    read_fallback(fallback_service, account, allow_user_interaction),
                ),
            };
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        match read(service, account) {
            Ok(Some(key)) => Ok(Some(key)),
            Ok(None) => read(fallback_service, account).map_err(|error| error.to_string()),
            Err(primary_error) => {
                preserve_primary_read_failure(primary_error, read(fallback_service, account))
            }
        }
    }
}

fn read_stored_secret(
    service: &str,
    fallback_service: &str,
    account: &str,
) -> Result<Option<String>, String> {
    read_stored_secret_with_interaction(service, fallback_service, account, false)
}

fn read_stored_api_key() -> Result<Option<String>, String> {
    let (service, fallback) = groq_api_services()?;
    read_stored_secret(service, fallback, ACCOUNT)
}

fn read_stored_asr_api_key() -> Result<Option<String>, String> {
    let (service, fallback) = api_services()?;
    read_stored_secret(service, fallback, ASR_ACCOUNT)
}

fn read_stored_cleanup_api_key() -> Result<Option<String>, String> {
    let (service, fallback) = api_services()?;
    read_stored_secret(service, fallback, CLEANUP_ACCOUNT)
}

fn is_keychain_auth_failure(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("passphrase") || lower.contains("errsecauthfailed") || lower.contains("(-25293)")
}

fn keychain_read_error_state(error: String) -> ApiKeyState {
    if is_keychain_auth_failure(&error) {
        log::warn!(
            "keychain credential unreadable ({error}); preserving legacy sources without migration"
        );
        ApiKeyState::Unavailable(error)
    } else {
        ApiKeyState::Unavailable(error)
    }
}

fn secret_state<F>(read: F) -> ApiKeyState
where
    F: FnOnce() -> Result<Option<String>, String> + Send + 'static,
{
    if keychain_disabled() {
        return ApiKeyState::Missing;
    }
    match with_timeout(read) {
        Some(Ok(Some(key))) => ApiKeyState::Configured(key),
        Some(Ok(None)) => ApiKeyState::Missing,
        Some(Err(error)) => keychain_read_error_state(error),
        None => ApiKeyState::Unavailable("keychain read timed out".into()),
    }
}

/// Read the API key while preserving the difference between an empty store
/// and a credential-store failure. In particular, a timeout must not be
/// persisted as "the user has no API key" during startup.
pub fn get_api_key_state() -> ApiKeyState {
    secret_state(read_stored_api_key)
}

/// Perform one user-authorized read of the active Groq credential. The caller
/// must already have verified a visible, focused Settings window. This keeps
/// the exact active namespace and serializes the temporary process-wide
/// Keychain interaction policy with all background operations.
fn guarded_interactive_read<T>(
    validate_after_gate: impl FnOnce() -> Result<(), InteractiveApiKeyRead>,
    read: impl FnOnce() -> T,
) -> Result<T, InteractiveApiKeyRead> {
    let _operation_guard = keychain_operation_guard();
    validate_after_gate()?;
    Ok(read())
}

pub fn read_groq_api_key_with_user_interaction<F>(validate_after_gate: F) -> InteractiveApiKeyRead
where
    F: FnOnce() -> Result<(), InteractiveApiKeyRead> + Send + 'static,
{
    if keychain_disabled() {
        return InteractiveApiKeyRead::Unavailable;
    }
    let (_service, fallback) = match groq_api_services() {
        Ok(services) => services,
        Err(_) => return InteractiveApiKeyRead::Unavailable,
    };
    guarded_interactive_read(validate_after_gate, || {
        #[cfg(target_os = "macos")]
        {
            read_groq_secret_interactively(_service, fallback)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (_service, fallback);
            InteractiveApiKeyRead::Unavailable
        }
    })
    .unwrap_or_else(|status| status)
}

/// Read the API key from the OS credential store.
/// Returns `None` if no key has been stored yet or the store is unavailable.
pub fn get_api_key() -> Option<String> {
    match get_api_key_state() {
        ApiKeyState::Configured(key) => Some(key),
        ApiKeyState::Missing | ApiKeyState::Unavailable(_) => None,
    }
}

/// Store the API key in the OS credential store.
/// An empty key deletes the stored credential.
fn set_secret(
    service: &str,
    fallback_service: &str,
    account: &str,
    key: &str,
) -> Result<(), String> {
    if keychain_disabled() {
        return Ok(());
    }
    let key = key.to_string();
    let service = service.to_string();
    let fallback_service = fallback_service.to_string();
    let account = account.to_string();
    #[cfg(all(target_os = "macos", debug_assertions))]
    let _ = &service;
    let result = with_write_timeout(move || -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            if key.is_empty() {
                #[cfg(debug_assertions)]
                {
                    delete_fallback(&fallback_service, &account).map_err(|error| error.to_string())
                }

                #[cfg(not(debug_assertions))]
                {
                    match delete_protected(&service, &account) {
                        Ok(()) => {
                            let _ = delete_fallback(&fallback_service, &account);
                            Ok(())
                        }
                        Err(error) => {
                            log::warn!(
                                "protected keychain delete unavailable; using login keychain fallback: {error}"
                            );
                            delete_fallback(&fallback_service, &account)
                                .map_err(|error| error.to_string())
                        }
                    }
                }
            } else {
                #[cfg(debug_assertions)]
                {
                    set_fallback(&fallback_service, &account, &key)
                        .map_err(|error| error.to_string())?;
                }

                #[cfg(not(debug_assertions))]
                {
                    match set_protected(&service, &account, &key) {
                        Ok(()) => {}
                        Err(error) => {
                            log::warn!(
                                "protected keychain write unavailable; using login keychain fallback: {error}"
                            );
                            set_fallback(&fallback_service, &account, &key)
                                .map_err(|error| error.to_string())?;
                        }
                    }
                }

                match read_stored_secret(&service, &fallback_service, &account) {
                    Ok(Some(stored)) if stored == key => Ok(()),
                    Ok(Some(_)) => Err("credential write verification failed".into()),
                    Ok(None) => {
                        Err("credential write succeeded but no credential was readable".into())
                    }
                    Err(error) => Err(format!("credential write verification failed: {error}")),
                }
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            let current_entry = entry(&service, &account).map_err(|e| e.to_string())?;
            if key.is_empty() {
                // Best-effort delete; NoEntry just means nothing was stored.
                match current_entry.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => {
                        if let Ok(previous) = entry(&fallback_service, &account) {
                            let _ = previous.delete_credential();
                        }
                        return Ok(());
                    }
                    Err(error) => return Err(error.to_string()),
                }
            }
            current_entry
                .set_password(&key)
                .map_err(|e| e.to_string())?;
            if let Ok(previous) = entry(&fallback_service, &account) {
                let _ = previous.delete_credential();
            }
            match read_stored_secret(&service, &fallback_service, &account) {
                Ok(Some(stored)) if stored == key => Ok(()),
                Ok(Some(_)) => Err("credential write verification failed".into()),
                Ok(None) => Err("credential write succeeded but no credential was readable".into()),
                Err(error) => Err(format!("credential write verification failed: {error}")),
            }
        }
    });
    match result {
        Some(res) => res,
        None => Err("keychain write timed out".into()),
    }
}

/// Store the API key in the OS credential store.
/// An empty key deletes the stored credential.
pub fn set_api_key(key: &str) -> Result<(), String> {
    if keychain_disabled() {
        return Ok(());
    }
    let (service, fallback) = groq_api_services()?;
    set_secret(service, fallback, ACCOUNT, key)
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(test, feature = "research-harness"))]
pub(crate) enum TestGroqKeyImportResult {
    Stored,
    AlreadyConfigured,
    Conflict,
    InvalidKey,
    WrongNamespace,
    Disabled,
    Unavailable,
    VerificationUnknown,
    TimedOut,
    Unsupported,
}

// Diagnostic enums and import/status helpers are available to unit tests and
// explicit research-harness builds. Runtime Keychain access is compiled only
// for macOS release builds with that feature enabled.
#[cfg_attr(
    not(all(
        feature = "research-harness",
        target_os = "macos",
        not(debug_assertions)
    )),
    allow(dead_code)
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(test, feature = "research-harness"))]
pub(crate) enum TestCredentialPhase {
    Admission,
    InteractionPolicy,
    Add,
    Readback,
    Wait,
    Complete,
}

#[cfg(any(test, feature = "research-harness"))]
impl TestCredentialPhase {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Admission => "admission",
            Self::InteractionPolicy => "interaction_policy",
            Self::Add => "add",
            Self::Readback => "readback",
            Self::Wait => "wait",
            Self::Complete => "complete",
        }
    }
}

#[cfg_attr(
    not(all(
        feature = "research-harness",
        target_os = "macos",
        not(debug_assertions)
    )),
    allow(dead_code)
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(test, feature = "research-harness"))]
pub(crate) enum TestCredentialAddOutcome {
    NotStarted,
    Added,
    Duplicate,
    Error,
    Unknown,
}

#[cfg(any(test, feature = "research-harness"))]
impl TestCredentialAddOutcome {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::Added => "added",
            Self::Duplicate => "duplicate",
            Self::Error => "error",
            Self::Unknown => "unknown",
        }
    }
}

#[cfg_attr(
    not(all(
        feature = "research-harness",
        target_os = "macos",
        not(debug_assertions)
    )),
    allow(dead_code)
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(test, feature = "research-harness"))]
pub(crate) enum TestCredentialReadOutcome {
    NotStarted,
    Matched,
    Conflict,
    NotFound,
    Error,
    NotCompleted,
    ReadableNonempty,
    Unavailable,
}

#[cfg(any(test, feature = "research-harness"))]
impl TestCredentialReadOutcome {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::Matched => "matched",
            Self::Conflict => "conflict",
            Self::NotFound => "not_found",
            Self::Error => "error",
            Self::NotCompleted => "not_completed",
            Self::ReadableNonempty => "readable_nonempty",
            Self::Unavailable => "unavailable",
        }
    }
}

#[cfg_attr(
    not(all(
        feature = "research-harness",
        target_os = "macos",
        not(debug_assertions)
    )),
    allow(dead_code)
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(test, feature = "research-harness"))]
pub(crate) enum TestCredentialWaitOutcome {
    NotStarted,
    Completed,
    TimedOut,
    Disconnected,
}

#[cfg(any(test, feature = "research-harness"))]
impl TestCredentialWaitOutcome {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::Completed => "completed",
            Self::TimedOut => "timeout",
            Self::Disconnected => "disconnected",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(test, feature = "research-harness"))]
pub(crate) struct TestCredentialAttemptDiagnostic {
    pub(crate) add: TestCredentialAddOutcome,
    pub(crate) read: TestCredentialReadOutcome,
    pub(crate) interaction_os_status: Option<i32>,
    pub(crate) operation_os_status: Option<i32>,
}

#[cfg(any(test, feature = "research-harness"))]
impl Default for TestCredentialAttemptDiagnostic {
    fn default() -> Self {
        Self {
            add: TestCredentialAddOutcome::NotStarted,
            read: TestCredentialReadOutcome::NotStarted,
            interaction_os_status: None,
            operation_os_status: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(test, feature = "research-harness"))]
pub(crate) struct TestGroqKeyImportDiagnostic {
    pub(crate) phase: TestCredentialPhase,
    pub(crate) location: &'static str,
    pub(crate) primary: TestCredentialAttemptDiagnostic,
    pub(crate) fallback: TestCredentialAttemptDiagnostic,
    pub(crate) wait: TestCredentialWaitOutcome,
    pub(crate) elapsed_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
pub(crate) struct TestGroqKeyImportOutcome {
    pub(crate) result: TestGroqKeyImportResult,
    pub(crate) diagnostic: TestGroqKeyImportDiagnostic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(test, feature = "research-harness"))]
pub(crate) struct TestCredentialReadStatus {
    pub(crate) outcome: TestCredentialReadOutcome,
    pub(crate) phase: TestCredentialPhase,
    pub(crate) os_status: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(test, feature = "research-harness"))]
pub(crate) struct TestGroqKeyStatus {
    pub(crate) primary: TestCredentialReadStatus,
    pub(crate) fallback: TestCredentialReadStatus,
    pub(crate) wait: TestCredentialWaitOutcome,
    pub(crate) elapsed_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
pub(crate) enum TestGroqKeyStatusResult {
    Completed(TestGroqKeyStatus),
    WrongNamespace,
    Disabled,
    TimedOut,
    Disconnected,
}

#[cfg(any(
    test,
    all(
        feature = "research-harness",
        target_os = "macos",
        not(debug_assertions)
    )
))]
fn valid_test_groq_import_key(key: &[u8]) -> bool {
    !key.is_empty() && key.len() <= 512 && key.iter().all(u8::is_ascii_graphic)
}

#[cfg(test)]
fn compare_test_groq_key(existing: Option<&[u8]>, requested: &[u8]) -> TestGroqKeyImportResult {
    match existing {
        Some(existing) if existing == requested => TestGroqKeyImportResult::AlreadyConfigured,
        Some(_) => TestGroqKeyImportResult::Conflict,
        None => TestGroqKeyImportResult::VerificationUnknown,
    }
}

#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
enum DetailedTestKeyRead {
    Found(String),
    NotFound,
    Error {
        phase: TestCredentialPhase,
        os_status: i32,
    },
}

#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
struct TestKeyAddFailure {
    phase: TestCredentialPhase,
    os_status: i32,
}

#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
fn add_password_once(
    service: &str,
    account: &str,
    key: &[u8],
    location: Location,
) -> Result<(), TestKeyAddFailure> {
    let _interaction_policy =
        keychain_interaction_policy(false).map_err(|error| TestKeyAddFailure {
            phase: TestCredentialPhase::InteractionPolicy,
            os_status: error.code(),
        })?;
    let item = ItemAddValue::Data {
        class: ItemClass::generic_password(),
        data: CFData::from_buffer(key),
    };
    let mut options = ItemAddOptions::new(item);
    options
        .set_service(service)
        .set_account_name(account)
        .set_location(location);
    options.add().map_err(|error| TestKeyAddFailure {
        phase: TestCredentialPhase::Add,
        os_status: error.code(),
    })
}

#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
fn read_protected_for_test_diagnostic(service: &str, account: &str) -> DetailedTestKeyRead {
    let _interaction_policy = match keychain_interaction_policy(false) {
        Ok(lock) => lock,
        Err(error) => {
            return DetailedTestKeyRead::Error {
                phase: TestCredentialPhase::InteractionPolicy,
                os_status: error.code(),
            }
        }
    };
    let mut options = ItemSearchOptions::new();
    options
        .class(ItemClass::generic_password())
        .service(service)
        .account(account)
        .load_data(true)
        .ignore_legacy_keychains();
    match options.search() {
        Ok(results) => results
            .into_iter()
            .find_map(|result| match result {
                SearchResult::Data(bytes) => String::from_utf8(bytes)
                    .ok()
                    .filter(|value| !value.is_empty()),
                _ => None,
            })
            .map(DetailedTestKeyRead::Found)
            .unwrap_or(DetailedTestKeyRead::NotFound),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => DetailedTestKeyRead::NotFound,
        Err(error) => DetailedTestKeyRead::Error {
            phase: TestCredentialPhase::Readback,
            os_status: error.code(),
        },
    }
}

#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
fn read_fallback_for_test_diagnostic(service: &str, account: &str) -> DetailedTestKeyRead {
    let _interaction_policy = match keychain_interaction_policy(false) {
        Ok(lock) => lock,
        Err(error) => {
            return DetailedTestKeyRead::Error {
                phase: TestCredentialPhase::InteractionPolicy,
                os_status: error.code(),
            }
        }
    };
    match generic_password(PasswordOptions::new_generic_password(service, account)) {
        Ok(bytes) => String::from_utf8(bytes)
            .ok()
            .filter(|value| !value.is_empty())
            .map(DetailedTestKeyRead::Found)
            .unwrap_or(DetailedTestKeyRead::NotFound),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => DetailedTestKeyRead::NotFound,
        Err(error) => DetailedTestKeyRead::Error {
            phase: TestCredentialPhase::Readback,
            os_status: error.code(),
        },
    }
}

#[cfg(any(
    test,
    all(
        feature = "research-harness",
        target_os = "macos",
        not(debug_assertions)
    )
))]
fn resolve_test_key_readback(
    existing: Result<Option<Vec<u8>>, i32>,
    key: &[u8],
    just_added: bool,
) -> (
    TestGroqKeyImportResult,
    TestCredentialReadOutcome,
    Option<i32>,
) {
    match existing {
        Ok(Some(existing)) if existing.as_slice() == key => {
            let result = if just_added {
                TestGroqKeyImportResult::Stored
            } else {
                TestGroqKeyImportResult::AlreadyConfigured
            };
            (result, TestCredentialReadOutcome::Matched, None)
        }
        Ok(Some(_)) => (
            TestGroqKeyImportResult::Conflict,
            TestCredentialReadOutcome::Conflict,
            None,
        ),
        Ok(None) => (
            TestGroqKeyImportResult::VerificationUnknown,
            TestCredentialReadOutcome::NotFound,
            None,
        ),
        Err(status) => (
            TestGroqKeyImportResult::VerificationUnknown,
            TestCredentialReadOutcome::Error,
            Some(status),
        ),
    }
}

#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
fn update_readback_diagnostic(
    attempt: &mut TestCredentialAttemptDiagnostic,
    read: DetailedTestKeyRead,
) -> (Result<Option<Vec<u8>>, i32>, Option<TestCredentialPhase>) {
    match read {
        DetailedTestKeyRead::Found(value) => {
            attempt.read = TestCredentialReadOutcome::Matched;
            (
                Ok(Some(value.into_bytes())),
                Some(TestCredentialPhase::Readback),
            )
        }
        DetailedTestKeyRead::NotFound => {
            attempt.read = TestCredentialReadOutcome::NotFound;
            (Ok(None), Some(TestCredentialPhase::Readback))
        }
        DetailedTestKeyRead::Error { phase, os_status } => {
            attempt.read = TestCredentialReadOutcome::Error;
            if phase == TestCredentialPhase::InteractionPolicy {
                attempt.interaction_os_status = Some(os_status);
            } else {
                attempt.operation_os_status = Some(os_status);
            }
            (Err(os_status), Some(phase))
        }
    }
}

#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
fn attempt_test_key_location(
    service: &str,
    key: &[u8],
    location: Location,
    fallback: bool,
    diagnostic: &mut TestGroqKeyImportDiagnostic,
) -> Result<TestGroqKeyImportResult, TestKeyAddFailure> {
    let added = match add_password_once(service, ACCOUNT, key, location) {
        Ok(()) => true,
        Err(error) if error.os_status == security_framework_sys::base::errSecDuplicateItem => false,
        Err(error) => return Err(error),
    };
    let attempt_diagnostic = if fallback {
        &mut diagnostic.fallback
    } else {
        &mut diagnostic.primary
    };
    attempt_diagnostic.add = if added {
        TestCredentialAddOutcome::Added
    } else {
        TestCredentialAddOutcome::Duplicate
    };
    let read = if fallback {
        read_fallback_for_test_diagnostic(service, ACCOUNT)
    } else {
        read_protected_for_test_diagnostic(service, ACCOUNT)
    };
    let (existing, phase) = update_readback_diagnostic(attempt_diagnostic, read);
    let (result, read_outcome, os_status) = resolve_test_key_readback(existing, key, added);
    attempt_diagnostic.read = read_outcome;
    if let Some(status) = os_status {
        attempt_diagnostic.operation_os_status = Some(status);
    }
    diagnostic.phase = phase.unwrap_or(TestCredentialPhase::Readback);
    diagnostic.location = if fallback { "fallback" } else { "primary" };
    Ok(result)
}

#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
fn record_add_failure(
    diagnostic: &mut TestGroqKeyImportDiagnostic,
    fallback: bool,
    failure: &TestKeyAddFailure,
) {
    let attempt = if fallback {
        &mut diagnostic.fallback
    } else {
        &mut diagnostic.primary
    };
    attempt.add = if failure.os_status == security_framework_sys::base::errSecDuplicateItem {
        TestCredentialAddOutcome::Duplicate
    } else {
        TestCredentialAddOutcome::Error
    };
    if failure.phase == TestCredentialPhase::InteractionPolicy {
        attempt.interaction_os_status = Some(failure.os_status);
    } else {
        attempt.operation_os_status = Some(failure.os_status);
    }
    diagnostic.phase = failure.phase;
    diagnostic.location = if fallback { "fallback" } else { "primary" };
}

#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
fn import_test_groq_key_once_sync(key: &[u8]) -> TestGroqKeyImportOutcome {
    let started = std::time::Instant::now();
    let (service, fallback_service) = groq_api_services_for(CredentialNamespace::InstallableTest);
    let mut diagnostic = TestGroqKeyImportDiagnostic {
        phase: TestCredentialPhase::Add,
        location: "primary",
        primary: TestCredentialAttemptDiagnostic::default(),
        fallback: TestCredentialAttemptDiagnostic::default(),
        wait: TestCredentialWaitOutcome::Completed,
        elapsed_ms: 0,
    };
    let primary = attempt_test_key_location(
        service,
        key,
        Location::DataProtectionKeychain,
        false,
        &mut diagnostic,
    );
    let result = match primary {
        Ok(result) => result,
        Err(error) if error.os_status == ERR_SEC_MISSING_ENTITLEMENT => {
            record_add_failure(&mut diagnostic, false, &error);
            match attempt_test_key_location(
                fallback_service,
                key,
                Location::DefaultFileKeychain,
                true,
                &mut diagnostic,
            ) {
                Ok(result) => result,
                Err(fallback_error) => {
                    record_add_failure(&mut diagnostic, true, &fallback_error);
                    TestGroqKeyImportResult::Unavailable
                }
            }
        }
        Err(error) => {
            record_add_failure(&mut diagnostic, false, &error);
            TestGroqKeyImportResult::Unavailable
        }
    };
    if matches!(
        result,
        TestGroqKeyImportResult::Stored
            | TestGroqKeyImportResult::AlreadyConfigured
            | TestGroqKeyImportResult::Conflict
    ) {
        diagnostic.phase = TestCredentialPhase::Complete;
    }
    diagnostic.elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    TestGroqKeyImportOutcome { result, diagnostic }
}

#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
fn status_for_test_key_read(read: DetailedTestKeyRead) -> TestCredentialReadStatus {
    match read {
        DetailedTestKeyRead::Found(value) => {
            let _ = value;
            TestCredentialReadStatus {
                outcome: TestCredentialReadOutcome::ReadableNonempty,
                phase: TestCredentialPhase::Readback,
                os_status: None,
            }
        }
        DetailedTestKeyRead::NotFound => TestCredentialReadStatus {
            outcome: TestCredentialReadOutcome::NotFound,
            phase: TestCredentialPhase::Readback,
            os_status: None,
        },
        DetailedTestKeyRead::Error { phase, os_status } => TestCredentialReadStatus {
            outcome: TestCredentialReadOutcome::Unavailable,
            phase,
            os_status: Some(os_status),
        },
    }
}

/// Inspect only the two fixed installable-Test Groq credential locations.
/// This is read-only and never compares or returns credential material.
#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
pub(crate) fn inspect_test_groq_key_status() -> TestGroqKeyStatusResult {
    if CREDENTIAL_NAMESPACE.get().copied() != Some(CredentialNamespace::InstallableTest) {
        return TestGroqKeyStatusResult::WrongNamespace;
    }
    if keychain_disabled() {
        return TestGroqKeyStatusResult::Disabled;
    }
    #[cfg(all(
        feature = "research-harness",
        target_os = "macos",
        not(debug_assertions)
    ))]
    {
        let started = std::time::Instant::now();
        let (service, fallback) = groq_api_services_for(CredentialNamespace::InstallableTest);
        match with_worker_timeout(std::time::Duration::from_secs(5), move || {
            (
                status_for_test_key_read(read_protected_for_test_diagnostic(service, ACCOUNT)),
                status_for_test_key_read(read_fallback_for_test_diagnostic(fallback, ACCOUNT)),
            )
        }) {
            WorkerWait::Completed((primary, fallback)) => {
                TestGroqKeyStatusResult::Completed(TestGroqKeyStatus {
                    primary,
                    fallback,
                    wait: TestCredentialWaitOutcome::Completed,
                    elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                })
            }
            WorkerWait::TimedOut => TestGroqKeyStatusResult::TimedOut,
            WorkerWait::Disconnected => TestGroqKeyStatusResult::Disconnected,
        }
    }
}

#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
fn import_diagnostic_for_result(
    result: TestGroqKeyImportResult,
    phase: TestCredentialPhase,
    wait: TestCredentialWaitOutcome,
    elapsed_ms: u64,
) -> TestGroqKeyImportOutcome {
    TestGroqKeyImportOutcome {
        result,
        diagnostic: TestGroqKeyImportDiagnostic {
            phase,
            location: "none",
            primary: TestCredentialAttemptDiagnostic::default(),
            fallback: TestCredentialAttemptDiagnostic::default(),
            wait,
            elapsed_ms,
        },
    }
}

/// Create the installable Test app's Groq credential once. A retry with the
/// same value succeeds without changing it; a different existing value is
/// never replaced. Ordinary, non-Test app credentials are not eligible.
#[cfg(all(
    feature = "research-harness",
    target_os = "macos",
    not(debug_assertions)
))]
pub(crate) fn import_test_groq_key_once_detailed(key: &[u8]) -> TestGroqKeyImportOutcome {
    let started = std::time::Instant::now();
    if CREDENTIAL_NAMESPACE.get().copied() != Some(CredentialNamespace::InstallableTest) {
        return import_diagnostic_for_result(
            TestGroqKeyImportResult::WrongNamespace,
            TestCredentialPhase::Admission,
            TestCredentialWaitOutcome::NotStarted,
            started.elapsed().as_millis() as u64,
        );
    }
    if keychain_disabled() {
        return import_diagnostic_for_result(
            TestGroqKeyImportResult::Disabled,
            TestCredentialPhase::Admission,
            TestCredentialWaitOutcome::NotStarted,
            started.elapsed().as_millis() as u64,
        );
    }
    if !valid_test_groq_import_key(key) {
        return import_diagnostic_for_result(
            TestGroqKeyImportResult::InvalidKey,
            TestCredentialPhase::Admission,
            TestCredentialWaitOutcome::NotStarted,
            started.elapsed().as_millis() as u64,
        );
    }

    #[cfg(all(
        feature = "research-harness",
        target_os = "macos",
        not(debug_assertions)
    ))]
    {
        let mut key = key.to_vec();
        match with_worker_timeout(std::time::Duration::from_secs(5), move || {
            let result = import_test_groq_key_once_sync(&key);
            key.fill(0);
            result
        }) {
            WorkerWait::Completed(mut outcome) => {
                outcome.diagnostic.wait = TestCredentialWaitOutcome::Completed;
                outcome.diagnostic.elapsed_ms = started.elapsed().as_millis() as u64;
                outcome
            }
            WorkerWait::TimedOut => {
                let mut outcome = import_diagnostic_for_result(
                    TestGroqKeyImportResult::TimedOut,
                    TestCredentialPhase::Wait,
                    TestCredentialWaitOutcome::TimedOut,
                    started.elapsed().as_millis() as u64,
                );
                outcome.diagnostic.primary.add = TestCredentialAddOutcome::Unknown;
                outcome.diagnostic.primary.read = TestCredentialReadOutcome::NotCompleted;
                outcome.diagnostic.fallback.add = TestCredentialAddOutcome::Unknown;
                outcome.diagnostic.fallback.read = TestCredentialReadOutcome::NotCompleted;
                outcome
            }
            WorkerWait::Disconnected => {
                let mut outcome = import_diagnostic_for_result(
                    TestGroqKeyImportResult::VerificationUnknown,
                    TestCredentialPhase::Wait,
                    TestCredentialWaitOutcome::Disconnected,
                    started.elapsed().as_millis() as u64,
                );
                outcome.diagnostic.primary.add = TestCredentialAddOutcome::Unknown;
                outcome.diagnostic.primary.read = TestCredentialReadOutcome::NotCompleted;
                outcome.diagnostic.fallback.add = TestCredentialAddOutcome::Unknown;
                outcome.diagnostic.fallback.read = TestCredentialReadOutcome::NotCompleted;
                outcome
            }
        }
    }

    #[cfg(not(all(target_os = "macos", not(debug_assertions))))]
    {
        import_diagnostic_for_result(
            TestGroqKeyImportResult::Unsupported,
            TestCredentialPhase::Admission,
            TestCredentialWaitOutcome::NotStarted,
            started.elapsed().as_millis() as u64,
        )
    }
}

pub fn get_asr_api_key_state() -> ApiKeyState {
    secret_state(read_stored_asr_api_key)
}

pub fn set_asr_api_key(key: &str) -> Result<(), String> {
    if keychain_disabled() {
        return Ok(());
    }
    let (service, fallback) = api_services()?;
    set_secret(service, fallback, ASR_ACCOUNT, key)
}

pub fn get_cleanup_api_key_state() -> ApiKeyState {
    secret_state(read_stored_cleanup_api_key)
}

pub fn set_cleanup_api_key(key: &str) -> Result<(), String> {
    if keychain_disabled() {
        return Ok(());
    }
    let (service, fallback) = api_services()?;
    set_secret(service, fallback, CLEANUP_ACCOUNT, key)
}

fn custom_provider_state(
    custom: ApiKeyState,
    asr: ApiKeyState,
    cleanup: ApiKeyState,
) -> ApiKeyState {
    match custom {
        configured @ ApiKeyState::Configured(_) => configured,
        unavailable @ ApiKeyState::Unavailable(_) => unavailable,
        ApiKeyState::Missing => match asr {
            configured @ ApiKeyState::Configured(_) => configured,
            unavailable @ ApiKeyState::Unavailable(_) => unavailable,
            ApiKeyState::Missing => cleanup,
        },
    }
}

pub fn get_provider_api_key_state(provider: crate::providers::EngineProvider) -> ApiKeyState {
    if provider.is_groq() {
        return get_api_key_state();
    }
    if provider.is_custom() {
        let custom = secret_state(move || {
            let (service, fallback) = api_services()?;
            read_stored_secret(service, fallback, provider.keychain_account())
        });
        if !matches!(custom, ApiKeyState::Missing) {
            return custom;
        }
        return custom_provider_state(custom, get_asr_api_key_state(), get_cleanup_api_key_state());
    }
    let account = provider.keychain_account();
    secret_state(move || {
        let (service, fallback) = api_services()?;
        read_stored_secret(service, fallback, account)
    })
}

pub fn set_provider_api_key(
    provider: crate::providers::EngineProvider,
    key: &str,
) -> Result<(), String> {
    if provider.is_groq() {
        return set_api_key(key);
    }
    if keychain_disabled() {
        return Ok(());
    }
    let (service, fallback) = api_services()?;
    set_secret(service, fallback, provider.keychain_account(), key)
}

/// Read the optional application-layer history encryption key.
pub fn get_history_key() -> Result<Option<Vec<u8>>, String> {
    if keychain_disabled() {
        return Ok(None);
    }
    let (service, fallback) = history_key_services()?;
    let result = with_timeout(move || read_stored_secret(service, fallback, HISTORY_KEY_ACCOUNT));
    match result {
        Some(Ok(Some(value))) => decode_history_key(&value).map(Some),
        Some(Ok(None)) => Ok(None),
        Some(Err(error)) => Err(error),
        None => Err("keychain read timed out".into()),
    }
}

/// Store or remove the optional application-layer history encryption key.
pub fn set_history_key(key: Option<&[u8]>) -> Result<(), String> {
    if let Some(key) = key {
        if key.len() != 32 {
            return Err("history key must be exactly 32 bytes".into());
        }
    }
    if keychain_disabled() {
        return Ok(());
    }
    if let Some(key) = key {
        let value = encode_history_key(key);
        let (service, fallback) = history_key_services()?;
        set_secret(service, fallback, HISTORY_KEY_ACCOUNT, &value)
    } else {
        let (service, fallback) = history_key_services()?;
        set_secret(service, fallback, HISTORY_KEY_ACCOUNT, "")
    }
}

fn encode_history_key(key: &[u8]) -> String {
    let mut encoded = String::with_capacity(key.len() * 2);
    for byte in key {
        use std::fmt::Write;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn decode_history_key(value: &str) -> Result<Vec<u8>, String> {
    if value.len() != 64 || !value.is_ascii() {
        return Err("stored history key must be 32-byte hex".into());
    }
    (0..value.len())
        .step_by(2)
        .map(|offset| {
            u8::from_str_radix(&value[offset..offset + 2], 16)
                .map_err(|_| "stored history key must be 32-byte hex".to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    #[test]
    fn keychain_interaction_policy_restores_prior_true_and_false_values() {
        for prior in [false, true] {
            let allowed = Arc::new(std::sync::atomic::AtomicBool::new(!prior));
            {
                let allowed = Arc::clone(&allowed);
                let _restore = InteractionPolicyRestore::new(prior, move |value| {
                    allowed.store(value, Ordering::SeqCst);
                });
            }
            assert_eq!(allowed.load(Ordering::SeqCst), prior);
        }
    }

    #[test]
    fn foreground_and_background_keychain_operations_share_one_gate() {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let first = std::thread::spawn(move || {
            let _guard = keychain_operation_guard();
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();

        let (second_tx, second_rx) = std::sync::mpsc::channel();
        let second = std::thread::spawn(move || {
            let _guard = keychain_operation_guard();
            second_tx.send(()).unwrap();
        });
        assert!(second_rx
            .recv_timeout(std::time::Duration::from_millis(30))
            .is_err());
        release_tx.send(()).unwrap();
        second_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        first.join().unwrap();
        second.join().unwrap();
    }

    #[test]
    fn queued_interactive_read_revalidates_before_the_native_query() {
        let foreground = keychain_operation_guard();
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let query_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker_query_count = Arc::clone(&query_count);
        let worker = std::thread::spawn(move || {
            guarded_interactive_read(
                || {
                    if worker_cancelled.load(Ordering::Acquire) {
                        Err(InteractiveApiKeyRead::Cancelled)
                    } else {
                        Ok(())
                    }
                },
                || {
                    worker_query_count.fetch_add(1, Ordering::AcqRel);
                    InteractiveApiKeyRead::Missing
                },
            )
        });

        cancelled.store(true, Ordering::Release);
        drop(foreground);

        assert_eq!(
            worker.join().unwrap(),
            Err(InteractiveApiKeyRead::Cancelled)
        );
        assert_eq!(query_count.load(Ordering::Acquire), 0);
    }

    #[test]
    fn timed_out_keychain_worker_does_not_start_after_foreground_operation() {
        let foreground = keychain_operation_guard();
        let state = Arc::new(std::sync::atomic::AtomicU8::new(KEYCHAIN_OPERATION_WAITING));
        let worker_state = Arc::clone(&state);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let guard = begin_queued_keychain_operation(&worker_state);
            started_tx.send(guard.is_some()).unwrap();
            drop(guard);
        });

        state
            .compare_exchange(
                KEYCHAIN_OPERATION_WAITING,
                KEYCHAIN_OPERATION_CANCELLED,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .unwrap();
        drop(foreground);

        assert!(!started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn installable_test_credentials_use_only_their_exact_bundle_namespace() {
        assert_eq!(
            credential_namespace_for_bundle(INSTALLABLE_TEST_BUNDLE_IDENTIFIER),
            CredentialNamespace::InstallableTest
        );
        assert_eq!(
            credential_namespace_for_bundle("com.voiceflow.desktop"),
            CredentialNamespace::Normal
        );
        assert_eq!(
            credential_namespace_for_bundle("com.voiceflow.installable-test.lookalike"),
            CredentialNamespace::Normal
        );

        let normal_api = api_services_for(CredentialNamespace::Normal);
        let test_api = api_services_for(CredentialNamespace::InstallableTest);
        let normal_groq = groq_api_services_for(CredentialNamespace::Normal);
        let test_groq = groq_api_services_for(CredentialNamespace::InstallableTest);
        let normal_history = history_key_services_for(CredentialNamespace::Normal);
        let test_history = history_key_services_for(CredentialNamespace::InstallableTest);
        assert_eq!(
            INSTALLABLE_TEST_GROQ_API_SERVICE,
            "com.voiceflow.installable-test.groq-credentials.v7"
        );
        assert_eq!(
            INSTALLABLE_TEST_GROQ_API_FALLBACK_SERVICE,
            "com.voiceflow.installable-test.groq-credentials.fallback.v7"
        );
        #[cfg(all(target_os = "macos", debug_assertions))]
        {
            assert_eq!(
                INSTALLABLE_TEST_GROQ_API_DEV_SERVICE,
                "com.voiceflow.installable-test.groq-credentials.dev.v2"
            );
            assert_eq!(
                test_groq,
                (
                    INSTALLABLE_TEST_GROQ_API_DEV_SERVICE,
                    INSTALLABLE_TEST_GROQ_API_DEV_SERVICE
                )
            );
        }
        #[cfg(not(all(target_os = "macos", debug_assertions)))]
        assert_eq!(
            test_groq,
            (
                INSTALLABLE_TEST_GROQ_API_SERVICE,
                INSTALLABLE_TEST_GROQ_API_FALLBACK_SERVICE
            )
        );
        assert_eq!(normal_api, (API_SERVICE, API_FALLBACK_SERVICE));
        assert_eq!(
            normal_history,
            (HISTORY_KEY_SERVICE, HISTORY_KEY_FALLBACK_SERVICE)
        );
        assert_eq!(normal_groq, normal_api);
        assert_ne!(test_groq, test_api);
        assert!(test_groq.0.contains("groq-credentials"));
        assert!(test_groq.1.contains("groq-credentials"));
        assert!(test_api.0.starts_with("com.voiceflow.installable-test."));
        assert!(test_api.1.starts_with("com.voiceflow.installable-test."));
        assert!(test_history
            .0
            .starts_with("com.voiceflow.installable-test."));
        assert!(test_history
            .1
            .starts_with("com.voiceflow.installable-test."));
        assert_ne!(normal_api.0, test_api.0);
        assert_ne!(normal_api.0, test_api.1);
        assert_ne!(normal_api.1, test_api.0);
        assert_ne!(normal_api.1, test_api.1);
        assert_ne!(normal_history.0, test_history.0);
        assert_ne!(normal_history.0, test_history.1);
        assert_ne!(normal_history.1, test_history.0);
        assert_ne!(normal_history.1, test_history.1);
    }

    #[test]
    fn namespace_binding_is_idempotent_conflict_checked_and_unbound_is_unavailable() {
        assert_eq!(
            bind_credential_namespace(None, CredentialNamespace::InstallableTest),
            Ok(CredentialNamespace::InstallableTest)
        );
        assert_eq!(
            bind_credential_namespace(
                Some(CredentialNamespace::InstallableTest),
                CredentialNamespace::InstallableTest
            ),
            Ok(CredentialNamespace::InstallableTest)
        );
        assert!(bind_credential_namespace(
            Some(CredentialNamespace::InstallableTest),
            CredentialNamespace::Normal
        )
        .is_err());
        assert!(initialized_credential_namespace(None).is_err());
        assert_eq!(
            initialized_credential_namespace(Some(CredentialNamespace::Normal)),
            Ok(CredentialNamespace::Normal)
        );
    }

    #[test]
    fn primary_read_error_is_not_reclassified_as_missing_by_empty_fallback() {
        assert_eq!(
            preserve_primary_read_failure::<String, _>("primary unavailable", Ok(None)),
            Err("primary unavailable".into())
        );
        assert_eq!(
            preserve_primary_read_failure("primary unavailable", Ok(Some("usable".to_owned()))),
            Ok(Some("usable".to_owned()))
        );
        assert_eq!(
            preserve_primary_read_failure::<String, _>(
                "primary unavailable",
                Err("fallback unavailable")
            ),
            Err("primary unavailable; fallback: fallback unavailable".into())
        );
    }

    #[test]
    fn explicit_missing_entitlement_code_remains_the_legacy_fallback_case() {
        assert!(missing_entitlement_code(ERR_SEC_MISSING_ENTITLEMENT));
        assert!(!missing_entitlement_code(-25293));
    }

    #[test]
    fn import_readback_keeps_match_conflict_missing_and_os_error_distinct() {
        assert_eq!(
            resolve_test_key_readback(Ok(Some(b"gsk-test".to_vec())), b"gsk-test", true),
            (
                TestGroqKeyImportResult::Stored,
                TestCredentialReadOutcome::Matched,
                None
            )
        );
        assert_eq!(
            resolve_test_key_readback(Ok(Some(b"gsk-test".to_vec())), b"gsk-test", false),
            (
                TestGroqKeyImportResult::AlreadyConfigured,
                TestCredentialReadOutcome::Matched,
                None
            )
        );
        assert_eq!(
            resolve_test_key_readback(Ok(Some(b"gsk-other".to_vec())), b"gsk-test", false),
            (
                TestGroqKeyImportResult::Conflict,
                TestCredentialReadOutcome::Conflict,
                None
            )
        );
        assert_eq!(
            resolve_test_key_readback(Ok(None), b"gsk-test", true),
            (
                TestGroqKeyImportResult::VerificationUnknown,
                TestCredentialReadOutcome::NotFound,
                None
            )
        );
        assert_eq!(
            resolve_test_key_readback(Err(-25293), b"gsk-test", true),
            (
                TestGroqKeyImportResult::VerificationUnknown,
                TestCredentialReadOutcome::Error,
                Some(-25293)
            )
        );
    }

    #[test]
    fn one_shot_worker_wait_states_remain_distinct() {
        fn classify<T>(wait: &WorkerWait<T>) -> TestCredentialWaitOutcome {
            match wait {
                WorkerWait::Completed(_) => TestCredentialWaitOutcome::Completed,
                WorkerWait::TimedOut => TestCredentialWaitOutcome::TimedOut,
                WorkerWait::Disconnected => TestCredentialWaitOutcome::Disconnected,
            }
        }
        assert_eq!(
            classify(&WorkerWait::Completed(())),
            TestCredentialWaitOutcome::Completed
        );
        assert_eq!(
            classify::<()>(&WorkerWait::TimedOut),
            TestCredentialWaitOutcome::TimedOut
        );
        assert_eq!(
            classify::<()>(&WorkerWait::Disconnected),
            TestCredentialWaitOutcome::Disconnected
        );
    }

    #[test]
    fn installable_test_groq_import_is_bounded_and_never_overwrites_conflicts() {
        assert!(valid_test_groq_import_key(b"gsk-test-value"));
        assert!(!valid_test_groq_import_key(b""));
        assert!(!valid_test_groq_import_key(&vec![b'x'; 513]));
        assert!(!valid_test_groq_import_key(b"key with space"));
        assert!(!valid_test_groq_import_key(b"key\0suffix"));
        assert_eq!(
            compare_test_groq_key(None, b"gsk-test-value"),
            TestGroqKeyImportResult::VerificationUnknown
        );
        assert_eq!(
            compare_test_groq_key(Some(b"gsk-test-value"), b"gsk-test-value"),
            TestGroqKeyImportResult::AlreadyConfigured
        );
        assert_eq!(
            compare_test_groq_key(Some(b"gsk-old-value"), b"gsk-new-value"),
            TestGroqKeyImportResult::Conflict
        );
    }

    // This test touches the real OS credential store. Run it manually with
    // `VOICEFLOW_TEST_KEYCHAIN_ENABLED=1 cargo test keychain::tests::round_trip
    // -- --ignored`; it is disabled by default even when explicitly selected.
    #[test]
    #[ignore]
    fn round_trip() {
        configure_runtime_bundle("com.voiceflow.desktop").unwrap();
        let key = format!("test_key_{}", std::process::id());
        set_api_key(&key).unwrap();
        let stored = get_api_key();
        set_api_key("").unwrap();
        let cleared = get_api_key();

        assert_eq!(stored.as_deref(), Some(key.as_str()));
        assert_eq!(cleared, None);
    }

    #[test]
    fn keychain_auth_failure_is_unavailable_so_legacy_data_is_not_migrated_over_it() {
        assert!(matches!(
            keychain_read_error_state(
                "The user name or passphrase you entered is not correct.".into()
            ),
            ApiKeyState::Unavailable(_)
        ));
        assert!(matches!(
            keychain_read_error_state("keychain read timed out".into()),
            ApiKeyState::Unavailable(_)
        ));
    }

    #[test]
    fn custom_provider_does_not_fall_back_when_canonical_read_is_unavailable() {
        assert_eq!(
            custom_provider_state(
                ApiKeyState::Unavailable("synthetic read failure".into()),
                ApiKeyState::Configured("stale alias".into()),
                ApiKeyState::Missing,
            ),
            ApiKeyState::Unavailable("synthetic read failure".into())
        );
        assert_eq!(
            custom_provider_state(
                ApiKeyState::Missing,
                ApiKeyState::Configured("legacy ASR alias".into()),
                ApiKeyState::Configured("legacy cleanup alias".into()),
            ),
            ApiKeyState::Configured("legacy ASR alias".into())
        );
    }

    #[test]
    fn foreground_custom_recovery_aliases_are_fixed_and_route_scoped() {
        let custom = crate::providers::EngineProvider::Custom.keychain_account();
        assert_eq!(custom_recovery_accounts(false, false), vec![custom]);
        assert_eq!(
            custom_recovery_accounts(true, false),
            vec![custom, ASR_ACCOUNT]
        );
        assert_eq!(
            custom_recovery_accounts(false, true),
            vec![custom, CLEANUP_ACCOUNT]
        );
        assert_eq!(
            custom_recovery_accounts(true, true),
            vec![custom, ASR_ACCOUNT, CLEANUP_ACCOUNT]
        );
    }

    #[test]
    fn foreground_custom_recovery_stops_on_unavailable_and_uses_only_ordered_aliases() {
        let accounts = custom_recovery_accounts(true, true);
        let mut seen = Vec::new();
        let unavailable = resolve_custom_recovery_accounts(&accounts, |account| {
            seen.push(account.to_owned());
            if account == crate::providers::EngineProvider::Custom.keychain_account() {
                InteractiveApiKeyRead::Unavailable
            } else {
                InteractiveApiKeyRead::Configured("synthetic-forbidden-fallback".into())
            }
        });
        assert_eq!(unavailable, InteractiveApiKeyRead::Unavailable);
        assert_eq!(
            seen,
            vec![crate::providers::EngineProvider::Custom
                .keychain_account()
                .to_owned()]
        );

        seen.clear();
        let asr_alias = resolve_custom_recovery_accounts(&accounts, |account| {
            seen.push(account.to_owned());
            if account == crate::providers::EngineProvider::Custom.keychain_account() {
                InteractiveApiKeyRead::Missing
            } else if account == ASR_ACCOUNT {
                InteractiveApiKeyRead::Configured("synthetic-asr-alias".into())
            } else {
                InteractiveApiKeyRead::Configured("synthetic-cleanup-alias".into())
            }
        });
        assert_eq!(
            asr_alias,
            InteractiveApiKeyRead::Configured("synthetic-asr-alias".into())
        );
        assert_eq!(
            seen,
            vec![
                crate::providers::EngineProvider::Custom
                    .keychain_account()
                    .to_owned(),
                ASR_ACCOUNT.to_owned(),
            ]
        );
    }

    #[test]
    fn history_key_encoding_is_binary_safe_and_exactly_32_bytes() {
        let key = (0_u8..32).collect::<Vec<_>>();
        let encoded = encode_history_key(&key);
        assert_eq!(encoded.len(), 64);
        assert_eq!(decode_history_key(&encoded).unwrap(), key);
        assert!(decode_history_key("00").is_err());
        assert!(decode_history_key(&"zz".repeat(32)).is_err());
    }
}
