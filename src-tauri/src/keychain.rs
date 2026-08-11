//! Secure storage for the Groq API key using the OS-native credential store.
//!
//! - macOS: Keychain
//! - Windows: Credential Manager
//! - Linux: Secret Service (e.g. GNOME Keyring / KWallet)
//!
//! The key is stored under a fixed service/account so it never touches
//! `settings.json` in plaintext. A one-time migration moves any legacy
//! plaintext key from the settings file into the keychain.

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
const FALLBACK_SERVICE: &str = "com.voiceflow.desktop.credentials.dev.v5";
#[cfg(not(target_os = "macos"))]
const PREVIOUS_SERVICE: &str = "com.voiceflow.desktop.credentials.v2";
const ACCOUNT: &str = "groq_api_key";

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
fn entry(service: &str) -> Result<Entry, keyring::Error> {
    Entry::new(service, ACCOUNT)
}

#[cfg(not(target_os = "macos"))]
fn read_keyring(service: &str) -> Result<Option<String>, keyring::Error> {
    match entry(service)?.get_password() {
        Ok(key) if !key.is_empty() => Ok(Some(key)),
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(not(target_os = "macos"))]
fn read(service: &str) -> Result<Option<String>, keyring::Error> {
    read_keyring(service)
}

#[cfg(all(target_os = "macos", not(debug_assertions)))]
fn protected_options(service: &str) -> PasswordOptions {
    let mut options = PasswordOptions::new_generic_password(service, ACCOUNT);
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
fn read_protected(service: &str) -> Result<Option<String>, SecurityError> {
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
        .account(ACCOUNT)
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
fn read_fallback(service: &str) -> Result<Option<String>, SecurityError> {
    let _interaction_lock = SecKeychain::disable_user_interaction()?;

    // Use the same PasswordOptions query used by set_fallback. ItemSearchOptions
    // does not reliably locate login-keychain items created through
    // set_generic_password_options on current macOS releases.
    match generic_password(PasswordOptions::new_generic_password(service, ACCOUNT)) {
        Ok(bytes) => Ok(String::from_utf8(bytes).ok().filter(|key| !key.is_empty())),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(all(target_os = "macos", not(debug_assertions)))]
fn delete_protected(service: &str) -> Result<(), SecurityError> {
    let _interaction_lock = SecKeychain::disable_user_interaction()?;

    let mut options = ItemSearchOptions::new();
    options
        .class(ItemClass::generic_password())
        .service(service)
        .account(ACCOUNT)
        .skip_authenticated_items(true)
        .ignore_legacy_keychains();

    match options.delete() {
        Ok(()) => Ok(()),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()), // errSecItemNotFound
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "macos")]
fn delete_fallback(service: &str) -> Result<(), SecurityError> {
    let _interaction_lock = SecKeychain::disable_user_interaction()?;

    match delete_generic_password_options(PasswordOptions::new_generic_password(service, ACCOUNT)) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()), // errSecItemNotFound
        Err(error) => Err(error),
    }
}

#[cfg(all(target_os = "macos", not(debug_assertions)))]
fn set_protected(service: &str, key: &str) -> Result<(), SecurityError> {
    // A credential-store operation must never be allowed to open an
    // interactive login-keychain prompt. If an item needs authentication,
    // fail softly and let the settings UI ask the user to configure the key
    // again instead.
    let _interaction_lock = SecKeychain::disable_user_interaction()?;
    set_generic_password_options(key.as_bytes(), protected_options(service))
}

#[cfg(target_os = "macos")]
fn set_fallback(service: &str, key: &str) -> Result<(), SecurityError> {
    // This path is used by unsigned/ad-hoc `tauri dev` builds, which cannot
    // address the Data Protection Keychain because they have no stable team
    // identity entitlement. It still uses SecItem APIs and never enables UI.
    // Replace the old item instead of updating it: the default login-keychain
    // item carries an ACL tied to the ad-hoc code signature, which changes on
    // every Cargo rebuild. The explicit accessibility class keeps the item
    // protected by the unlocked user keychain without that per-build ACL.
    delete_fallback(service)?;
    let _interaction_lock = SecKeychain::disable_user_interaction()?;
    let mut options = PasswordOptions::new_generic_password(service, ACCOUNT);
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
fn with_timeout<T, F>(op: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(op());
    });
    rx.recv_timeout(std::time::Duration::from_millis(800)).ok()
}

#[allow(clippy::needless_return)]
fn read_stored_api_key() -> Result<Option<String>, String> {
    if keychain_disabled() {
        return Ok(None);
    }
    #[cfg(target_os = "macos")]
    {
        // `tauri dev` is ad-hoc signed and cannot use the Data Protection
        // Keychain's application identity. Keep its credential in a separate
        // service instead of probing a production item and turning a normal
        // development launch into a missing-entitlement error.
        #[cfg(debug_assertions)]
        {
            return read_fallback(FALLBACK_SERVICE).map_err(|error| error.to_string());
        }

        #[cfg(not(debug_assertions))]
        {
            let primary = read_protected(SERVICE);
            return match primary {
                Ok(Some(key)) => Ok(Some(key)),
                Ok(None) => read_fallback(FALLBACK_SERVICE).map_err(|error| error.to_string()),
                Err(primary_error) if missing_entitlement(primary_error) => {
                    read_fallback(FALLBACK_SERVICE).map_err(|error| error.to_string())
                }
                Err(primary_error) => read_fallback(FALLBACK_SERVICE).map_err(|fallback_error| {
                    format!("{primary_error}; fallback: {fallback_error}")
                }),
            };
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        match read(SERVICE) {
            Ok(Some(key)) => Ok(Some(key)),
            Ok(None) => read(PREVIOUS_SERVICE).map_err(|error| error.to_string()),
            Err(primary_error) => read(PREVIOUS_SERVICE)
                .map_err(|fallback_error| format!("{primary_error}; fallback: {fallback_error}")),
        }
    }
}

/// Read the API key while preserving the difference between an empty store
/// and a credential-store failure. In particular, a timeout must not be
/// persisted as "the user has no API key" during startup.
pub fn get_api_key_state() -> ApiKeyState {
    if keychain_disabled() {
        return ApiKeyState::Missing;
    }
    let result = with_timeout(read_stored_api_key);
    match result {
        Some(Ok(Some(key))) => ApiKeyState::Configured(key),
        Some(Ok(None)) => ApiKeyState::Missing,
        Some(Err(error)) => ApiKeyState::Unavailable(error),
        None => ApiKeyState::Unavailable("keychain read timed out".into()),
    }
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
pub fn set_api_key(key: &str) -> Result<(), String> {
    if keychain_disabled() {
        return Ok(());
    }
    let key = key.to_string();
    let result = with_timeout(move || -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            if key.is_empty() {
                #[cfg(debug_assertions)]
                {
                    delete_fallback(FALLBACK_SERVICE).map_err(|error| error.to_string())
                }

                #[cfg(not(debug_assertions))]
                {
                    return match delete_protected(SERVICE) {
                        Ok(()) => {
                            let _ = delete_fallback(FALLBACK_SERVICE);
                            Ok(())
                        }
                        Err(error) => {
                            log::warn!(
                                "protected keychain delete unavailable; using login keychain fallback: {error}"
                            );
                            delete_fallback(FALLBACK_SERVICE).map_err(|error| error.to_string())
                        }
                    };
                }
            } else {
                #[cfg(debug_assertions)]
                {
                    set_fallback(FALLBACK_SERVICE, &key).map_err(|error| error.to_string())?;
                }

                #[cfg(not(debug_assertions))]
                {
                    match set_protected(SERVICE, &key) {
                        Ok(()) => {}
                        Err(error) => {
                            log::warn!(
                                "protected keychain write unavailable; using login keychain fallback: {error}"
                            );
                            set_fallback(FALLBACK_SERVICE, &key)
                                .map_err(|error| error.to_string())?;
                        }
                    }
                }

                match read_stored_api_key() {
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
            let current_entry = entry(SERVICE).map_err(|e| e.to_string())?;
            if key.is_empty() {
                // Best-effort delete; NoEntry just means nothing was stored.
                match current_entry.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => {
                        if let Ok(previous) = entry(PREVIOUS_SERVICE) {
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
            if let Ok(previous) = entry(PREVIOUS_SERVICE) {
                let _ = previous.delete_credential();
            }
            match read_stored_api_key() {
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

/// Migrate a legacy plaintext key (from `settings.json`) into the keychain.
/// Returns the key that should now be held in memory. The caller is
/// responsible for clearing the plaintext field and re-saving settings.
#[allow(dead_code)]
pub fn migrate_plaintext(plaintext: &str) -> Option<String> {
    if keychain_disabled() {
        // In tests we can't persist to a keychain, so just hand back the
        // plaintext key for in-memory use.
        return if plaintext.is_empty() {
            None
        } else {
            Some(plaintext.to_string())
        };
    }
    if plaintext.is_empty() {
        return get_api_key();
    }
    match set_api_key(plaintext) {
        Ok(()) => Some(plaintext.to_string()),
        Err(error) => {
            log::error!("failed to migrate API key into keychain: {error}");
            // Fall back to whatever (if anything) is already in the keychain.
            get_api_key()
        }
    }
}

/// Resolve a legacy plaintext key or an existing OS credential without
/// converting an unavailable keychain into a destructive settings update.
pub fn resolve_api_key(plaintext: &str) -> ApiKeyState {
    if !plaintext.is_empty() {
        if keychain_disabled() {
            return ApiKeyState::Configured(plaintext.to_owned());
        }
        return match set_api_key(plaintext) {
            Ok(()) => ApiKeyState::Configured(plaintext.to_owned()),
            Err(error) => ApiKeyState::Unavailable(error),
        };
    }
    get_api_key_state()
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
}
