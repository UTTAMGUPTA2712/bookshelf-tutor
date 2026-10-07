//! API keys live in the OS keychain (Secret Service / Keychain / Credential Manager),
//! never in the database, the WebView, or export files.

use crate::error::{AppError, AppResult};

const SERVICE: &str = "com.bookshelf.tutor";

fn entry(provider_id: &str) -> AppResult<keyring::Entry> {
    keyring::Entry::new(SERVICE, &format!("provider:{provider_id}"))
        .map_err(|e| AppError::Secret(format!("Keychain unavailable: {e}")))
}

pub fn set_key(provider_id: &str, key: &str) -> AppResult<()> {
    entry(provider_id)?
        .set_password(key)
        .map_err(|e| AppError::Secret(format!("Could not save the API key to the keychain: {e}")))
}

pub fn get_key(provider_id: &str) -> AppResult<Option<String>> {
    match entry(provider_id)?.get_password() {
        Ok(k) => Ok(Some(k)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(AppError::Secret(format!("Could not read the API key from the keychain: {e}"))),
    }
}

pub fn delete_key(provider_id: &str) -> AppResult<()> {
    match entry(provider_id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(AppError::Secret(format!("Could not delete the API key: {e}"))),
    }
}
