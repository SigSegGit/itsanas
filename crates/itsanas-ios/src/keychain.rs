//! Where the iPhone remembers the passphrase: its Keychain, on this device and
//! readable only while it is unlocked, as Android's keystore does.
//!
//! A machine that is not an iPhone has no Keychain; the stand-in lets the tests
//! of this crate run on every machine of the CI.

/// Why nothing could be read or kept, in the system's words.
pub(crate) type Kept<T> = Result<T, String>;

#[cfg(target_os = "ios")]
mod system {
    use super::Kept;
    use security_framework::access_control::{ProtectionMode, SecAccessControl};
    use security_framework::passwords::{
        PasswordOptions, delete_generic_password, get_generic_password,
        set_generic_password_options,
    };

    const SERVICE: &str = "fr.ngas.itsanas";
    const ACCOUNT: &str = "passphrase";

    pub(super) fn get() -> Option<Vec<u8>> {
        get_generic_password(SERVICE, ACCOUNT).ok()
    }

    pub(super) fn set(secret: &str) -> Kept<()> {
        // Deleted first: an update cannot change an item's protection.
        forget();
        let protection = SecAccessControl::create_with_protection(
            Some(ProtectionMode::AccessibleWhenUnlockedThisDeviceOnly),
            0,
        )
        .map_err(|error| error.to_string())?;
        let mut options = PasswordOptions::new_generic_password(SERVICE, ACCOUNT);
        options.set_access_control(protection);
        set_generic_password_options(secret.as_bytes(), options).map_err(|error| error.to_string())
    }

    pub(super) fn forget() {
        let _ = delete_generic_password(SERVICE, ACCOUNT);
    }
}

#[cfg(not(target_os = "ios"))]
mod system {
    use super::Kept;
    use std::sync::{Mutex, PoisonError};

    static HELD: Mutex<Option<Vec<u8>>> = Mutex::new(None);

    pub(super) fn get() -> Option<Vec<u8>> {
        HELD.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    #[allow(clippy::unnecessary_wraps)] // The Keychain's answer has this shape.
    pub(super) fn set(secret: &str) -> Kept<()> {
        *HELD.lock().unwrap_or_else(PoisonError::into_inner) = Some(secret.as_bytes().to_vec());
        Ok(())
    }

    pub(super) fn forget() {
        *HELD.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

/// The remembered passphrase, if there is a readable one.
pub(crate) fn get() -> Option<zeroize::Zeroizing<String>> {
    let bytes = zeroize::Zeroizing::new(system::get()?);
    String::from_utf8(bytes.to_vec())
        .ok()
        .map(zeroize::Zeroizing::new)
}

pub(crate) fn set(secret: &str) -> Kept<()> {
    system::set(secret)
}

pub(crate) fn forget() {
    system::forget();
}
