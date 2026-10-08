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
#[cfg(target_os = "macos")]
use core_foundation::string::CFString;
#[cfg(target_os = "macos")]
use security_framework::base::Error as SecurityError;
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
#[cfg(all(target_os = "macos", not(debug_assertions)))]
const ERR_SEC_MISSING_ENTITLEMENT: i32 = -34018;
#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

// Never reuse an item that may have been created by an ad-hoc build. Those
// items can carry a code-signature ACL and cause macOS to show a login
// keychain prompt after every Cargo rebuild. The current service is a fresh
// Data Protection Keychain item. Legacy app services are intentionally not
// touched: probing them can resurrect the password dialog that this app must
// never show during startup.
#[cfg(any(not(target_os = "macos"), not(debug_assertions)))]
const SERVICE: &str = "com.voiceflow.desktop.credentials.v3";
#[cfg(target_os = "macos")]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyState {
    Configured(String),
    Missing,
    Unavailable(String),
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
fn read_protected(service: &str, account: &str) -> Result<Option<String>, SecurityError> {
    // Keep startup and background settings reconciliation completely
    // non-interactive. `skip_authenticated_items` is encoded into the query
    // too, but the thread-local Security.framework guard is the final
    // protection against a login-keychain password sheet appearing here.
    let _interaction_lock = SecKeychain::disable_user_interaction()?;

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
fn read_fallback(service: &str, account: &str) -> Result<Option<String>, SecurityError> {
    let _interaction_lock = SecKeychain::disable_user_interaction()?;

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
    let _interaction_lock = SecKeychain::disable_user_interaction()?;

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
    let _interaction_lock = SecKeychain::disable_user_interaction()?;

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
    let _interaction_lock = SecKeychain::disable_user_interaction()?;
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
    let _interaction_lock = SecKeychain::disable_user_interaction()?;
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
    error.code() == ERR_SEC_MISSING_ENTITLEMENT
        || error
            .to_string()
            .to_ascii_lowercase()
            .contains("entitlement")
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
    std::thread::spawn(move || {
        let _ = tx.send(op());
    });
    rx.recv_timeout(timeout).ok()
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

#[allow(clippy::needless_return)]
fn read_stored_secret(
    service: &str,
    fallback_service: &str,
    account: &str,
) -> Result<Option<String>, String> {
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
            return read_fallback(fallback_service, account).map_err(|error| error.to_string());
        }

        #[cfg(not(debug_assertions))]
        {
            let primary = read_protected(service, account);
            return match primary {
                Ok(Some(key)) => Ok(Some(key)),
                Ok(None) => {
                    read_fallback(fallback_service, account).map_err(|error| error.to_string())
                }
                Err(primary_error) if missing_entitlement(primary_error) => {
                    read_fallback(fallback_service, account).map_err(|error| error.to_string())
                }
                Err(primary_error) => {
                    read_fallback(fallback_service, account).map_err(|fallback_error| {
                        format!("{primary_error}; fallback: {fallback_error}")
                    })
                }
            };
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        match read(service, account) {
            Ok(Some(key)) => Ok(Some(key)),
            Ok(None) => read(fallback_service, account).map_err(|error| error.to_string()),
            Err(primary_error) => read(fallback_service, account)
                .map_err(|fallback_error| format!("{primary_error}; fallback: {fallback_error}")),
        }
    }
}

fn read_stored_api_key() -> Result<Option<String>, String> {
    read_stored_secret(API_SERVICE, API_FALLBACK_SERVICE, ACCOUNT)
}

fn read_stored_asr_api_key() -> Result<Option<String>, String> {
    read_stored_secret(API_SERVICE, API_FALLBACK_SERVICE, ASR_ACCOUNT)
}

fn read_stored_cleanup_api_key() -> Result<Option<String>, String> {
    read_stored_secret(API_SERVICE, API_FALLBACK_SERVICE, CLEANUP_ACCOUNT)
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
                    return match delete_protected(&service, &account) {
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
                    };
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
    set_secret(API_SERVICE, API_FALLBACK_SERVICE, ACCOUNT, key)
}

pub fn get_asr_api_key_state() -> ApiKeyState {
    secret_state(read_stored_asr_api_key)
}

#[allow(dead_code)]
pub fn get_asr_api_key() -> Option<String> {
    match get_asr_api_key_state() {
        ApiKeyState::Configured(key) => Some(key),
        ApiKeyState::Missing | ApiKeyState::Unavailable(_) => None,
    }
}

pub fn set_asr_api_key(key: &str) -> Result<(), String> {
    set_secret(API_SERVICE, API_FALLBACK_SERVICE, ASR_ACCOUNT, key)
}

pub fn get_cleanup_api_key_state() -> ApiKeyState {
    secret_state(read_stored_cleanup_api_key)
}

pub fn set_cleanup_api_key(key: &str) -> Result<(), String> {
    set_secret(API_SERVICE, API_FALLBACK_SERVICE, CLEANUP_ACCOUNT, key)
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
            read_stored_secret(
                API_SERVICE,
                API_FALLBACK_SERVICE,
                provider.keychain_account(),
            )
        });
        if !matches!(custom, ApiKeyState::Missing) {
            return custom;
        }
        return custom_provider_state(custom, get_asr_api_key_state(), get_cleanup_api_key_state());
    }
    let account = provider.keychain_account();
    secret_state(move || read_stored_secret(API_SERVICE, API_FALLBACK_SERVICE, account))
}

pub fn set_provider_api_key(
    provider: crate::providers::EngineProvider,
    key: &str,
) -> Result<(), String> {
    if provider.is_groq() {
        return set_api_key(key);
    }
    set_secret(
        API_SERVICE,
        API_FALLBACK_SERVICE,
        provider.keychain_account(),
        key,
    )
}

fn resolve_legacy_secret(
    plaintext: &str,
    read: fn() -> ApiKeyState,
    write: fn(&str) -> Result<(), String>,
) -> ApiKeyState {
    match read() {
        configured @ ApiKeyState::Configured(_) => configured,
        unavailable @ ApiKeyState::Unavailable(_) => unavailable,
        ApiKeyState::Missing if plaintext.is_empty() => ApiKeyState::Missing,
        ApiKeyState::Missing => match write(plaintext) {
            Err(error) => ApiKeyState::Unavailable(error),
            Ok(()) => match read() {
                ApiKeyState::Configured(stored) if stored == plaintext => {
                    ApiKeyState::Configured(stored)
                }
                ApiKeyState::Configured(_) | ApiKeyState::Missing => ApiKeyState::Unavailable(
                    "credential_storage: legacy credential write was not verified".into(),
                ),
                ApiKeyState::Unavailable(error) => ApiKeyState::Unavailable(error),
            },
        },
    }
}

/// Read the optional application-layer history encryption key.
pub fn get_history_key() -> Result<Option<Vec<u8>>, String> {
    if keychain_disabled() {
        return Ok(None);
    }
    let result = with_timeout(|| {
        read_stored_secret(
            HISTORY_KEY_SERVICE,
            HISTORY_KEY_FALLBACK_SERVICE,
            HISTORY_KEY_ACCOUNT,
        )
    });
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
        let value = encode_history_key(key);
        set_secret(
            HISTORY_KEY_SERVICE,
            HISTORY_KEY_FALLBACK_SERVICE,
            HISTORY_KEY_ACCOUNT,
            &value,
        )
    } else {
        set_secret(
            HISTORY_KEY_SERVICE,
            HISTORY_KEY_FALLBACK_SERVICE,
            HISTORY_KEY_ACCOUNT,
            "",
        )
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

/// Migrate a legacy plaintext key (from `settings.json`) into the keychain.
/// Returns the key that should now be held in memory. The caller is
/// responsible for clearing the plaintext field and re-saving settings.
#[allow(dead_code)]
pub fn migrate_plaintext(plaintext: &str) -> Option<String> {
    match resolve_api_key(plaintext) {
        ApiKeyState::Configured(key) => Some(key),
        ApiKeyState::Missing | ApiKeyState::Unavailable(_) => None,
    }
}

/// Resolve a legacy plaintext key or an existing OS credential without
/// converting an unavailable keychain into a destructive settings update.
pub fn resolve_api_key(plaintext: &str) -> ApiKeyState {
    resolve_legacy_secret(plaintext, get_api_key_state, set_api_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    // This test touches the real OS credential store. Run it manually with
    // `VOICEFLOW_TEST_KEYCHAIN_ENABLED=1 cargo test keychain::tests::round_trip
    // -- --ignored`; it is disabled by default even when explicitly selected.
    #[test]
    #[ignore]
    fn round_trip() {
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
    fn history_key_encoding_is_binary_safe_and_exactly_32_bytes() {
        let key = (0_u8..32).collect::<Vec<_>>();
        let encoded = encode_history_key(&key);
        assert_eq!(encoded.len(), 64);
        assert_eq!(decode_history_key(&encoded).unwrap(), key);
        assert!(decode_history_key("00").is_err());
        assert!(decode_history_key(&"zz".repeat(32)).is_err());
    }
}
