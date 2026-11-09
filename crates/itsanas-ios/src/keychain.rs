//! Where the iPhone remembers the passphrase: its Keychain, on this device and
//! readable only while it is unlocked, or only with Touch ID or Face ID.
//!
//! With Touch ID on, nothing hands the passphrase over without the finger, and
//! the engine is told there is none and asks: reading it for a step of setup
//! would put the system's prompt in the middle of a page. Android does the same
//! (`Host.storedPassphrase`).
//!
//! A machine that is not an iPhone has no Keychain; the stand-in lets the tests
//! of this crate run on every machine of the CI.

use zeroize::Zeroizing;

/// Why nothing could be kept, in the system's words.
pub(crate) type Kept<T> = Result<T, String>;

const PASSPHRASE: &str = "passphrase";
/// Present when Touch ID guards the passphrase. Its own item, unguarded, so
/// that asking whether it is on does not ask for a finger.
const GUARDED: &str = "biometric";

#[cfg(target_os = "ios")]
mod system {
    use super::Kept;
    use security_framework::access_control::{ProtectionMode, SecAccessControl};
    use security_framework::passwords::{
        AccessControlOptions, PasswordOptions, delete_generic_password, get_generic_password,
        set_generic_password_options,
    };

    const SERVICE: &str = "fr.ngas.itsanas";

    pub(super) fn get(account: &str) -> Option<Vec<u8>> {
        get_generic_password(SERVICE, account).ok()
    }

    pub(super) fn set(account: &str, secret: &str, finger: bool) -> Kept<()> {
        // Deleted first: an update cannot change an item's protection.
        forget(account);
        // The current fingers only: one enrolled later opens nothing.
        let (protection, flags) = if finger {
            (
                ProtectionMode::AccessibleWhenPasscodeSetThisDeviceOnly,
                AccessControlOptions::BIOMETRY_CURRENT_SET.bits(),
            )
        } else {
            (ProtectionMode::AccessibleWhenUnlockedThisDeviceOnly, 0)
        };
        let control = SecAccessControl::create_with_protection(Some(protection), flags)
            .map_err(|error| error.to_string())?;
        let mut options = PasswordOptions::new_generic_password(SERVICE, account);
        options.set_access_control(control);
        set_generic_password_options(secret.as_bytes(), options).map_err(|error| error.to_string())
    }

    pub(super) fn forget(account: &str) {
        let _ = delete_generic_password(SERVICE, account);
    }
}

#[cfg(not(target_os = "ios"))]
mod system {
    use super::Kept;
    use std::collections::BTreeMap;
    use std::sync::{Mutex, PoisonError};

    static HELD: Mutex<BTreeMap<String, Vec<u8>>> = Mutex::new(BTreeMap::new());

    pub(super) fn get(account: &str) -> Option<Vec<u8>> {
        HELD.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(account)
            .cloned()
    }

    #[allow(clippy::unnecessary_wraps)] // The Keychain's answer has this shape.
    pub(super) fn set(account: &str, secret: &str, _finger: bool) -> Kept<()> {
        HELD.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(account.to_owned(), secret.as_bytes().to_vec());
        Ok(())
    }

    pub(super) fn forget(account: &str) {
        HELD.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(account);
    }
}

/// Whether Touch ID guards the passphrase.
pub(crate) fn guarded() -> bool {
    system::get(GUARDED).is_some()
}

/// The passphrase, for opening the account: asks for the finger when guarded.
pub(crate) fn unlock() -> Option<Zeroizing<String>> {
    let bytes = Zeroizing::new(system::get(PASSPHRASE)?);
    String::from_utf8(bytes.to_vec()).ok().map(Zeroizing::new)
}

/// The passphrase, for the engine: never when a finger would be needed.
pub(crate) fn for_the_engine() -> Option<Zeroizing<String>> {
    if guarded() { None } else { unlock() }
}

/// Keep a passphrase that opened the account, as it is guarded today.
pub(crate) fn set(secret: &str) -> Kept<()> {
    system::set(PASSPHRASE, secret, guarded())
}

/// Keep what the engine stores. Guarded, it is left alone: a new item would
/// need no finger, and the next opening with a typed passphrase renews it.
pub(crate) fn from_the_engine(secret: &str) -> Kept<()> {
    if guarded() { Ok(()) } else { set(secret) }
}

/// Forget the passphrase and whether a finger guarded it: signing out.
pub(crate) fn forget() {
    system::forget(PASSPHRASE);
    system::forget(GUARDED);
}

/// Guard the passphrase with Touch ID, or stop. Whether it is now as asked.
pub(crate) fn set_guarded(on: bool) -> bool {
    if on == guarded() {
        return true;
    }
    // Off asks for the finger here, which is what turning it off should need.
    let Some(passphrase) = unlock() else {
        return false;
    };
    if system::set(PASSPHRASE, &passphrase, on).is_err() {
        // `set` deleted the item first: a phone with no finger enrolled must
        // not lose its passphrase for having asked.
        let _ = system::set(PASSPHRASE, &passphrase, !on);
        return false;
    }
    if on {
        system::set(GUARDED, "on", false).is_ok()
    } else {
        system::forget(GUARDED);
        true
    }
}
