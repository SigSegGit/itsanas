//! Where the iPhone remembers the passphrase: its Keychain, on this device,
//! readable from the first unlock after the phone starts, so that a round iOS
//! grants with the app away can open the account.
//!
//! Touch ID or Face ID is a lock on the app's screen, as in most apps that sync:
//! the unlock screen asks for the finger before it opens the account. An item
//! bound to the finger could not be read in the background, and sync would stop
//! for whoever turned the lock on. The engine is still told there is no
//! remembered passphrase while the lock is on: a step of the page asks for it.
//!
//! A machine that is not an iPhone has no Keychain; the stand-in lets the tests
//! of this crate run on every machine of the CI.

use zeroize::Zeroizing;

/// Why nothing could be kept, in the system's words.
pub(crate) type Kept<T> = Result<T, String>;

const PASSPHRASE: &str = "passphrase";
/// Present when the unlock screen asks for a finger.
const GUARDED: &str = "biometric";

#[cfg(target_os = "ios")]
mod system {
    use super::Kept;
    use security_framework::access_control::{ProtectionMode, SecAccessControl};
    use security_framework::passwords::{
        PasswordOptions, delete_generic_password, get_generic_password,
        set_generic_password_options,
    };

    const SERVICE: &str = "fr.ngas.itsanas";

    pub(super) fn get(account: &str) -> Option<Vec<u8>> {
        get_generic_password(SERVICE, account).ok()
    }

    pub(super) fn set(account: &str, secret: &str) -> Kept<()> {
        // Deleted first: an update cannot change an item's protection.
        forget(account);
        let control = SecAccessControl::create_with_protection(
            Some(ProtectionMode::AccessibleAfterFirstUnlockThisDeviceOnly),
            0,
        )
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
    pub(super) fn set(account: &str, secret: &str) -> Kept<()> {
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

/// Whether the unlock screen asks for a finger.
pub(crate) fn guarded() -> bool {
    system::get(GUARDED).is_some()
}

/// What the lock holds until a first finger says which ones are enrolled.
const UNSEEN: &str = "on";

/// Whether the fingers enrolled now, as `state` gives them (iOS's
/// `evaluatedPolicyDomainState`), are those the lock first saw. The first
/// finger after the lock is turned on records them; a finger enrolled later
/// changes `state`, and then only the passphrase opens, until the lock is
/// turned off and on again. With no lock, any state will do.
pub(crate) fn same_fingers(state: &str) -> bool {
    if state.is_empty() {
        return !guarded();
    }
    match system::get(GUARDED) {
        None => true,
        Some(kept) if kept == UNSEEN.as_bytes() => system::set(GUARDED, state).is_ok(),
        Some(kept) => kept == state.as_bytes(),
    }
}

/// The passphrase, for opening the account.
pub(crate) fn unlock() -> Option<Zeroizing<String>> {
    let bytes = Zeroizing::new(system::get(PASSPHRASE)?);
    String::from_utf8(bytes.to_vec()).ok().map(Zeroizing::new)
}

/// The passphrase, for the engine: never while the screen is locked by a finger.
pub(crate) fn for_the_engine() -> Option<Zeroizing<String>> {
    if guarded() { None } else { unlock() }
}

/// Keep a passphrase, in place of the one kept.
pub(crate) fn set(secret: &str) -> Kept<()> {
    system::set(PASSPHRASE, secret)
}

/// Forget the passphrase and whether a finger was asked: signing out.
pub(crate) fn forget() {
    system::forget(PASSPHRASE);
    system::forget(GUARDED);
}

/// Ask for a finger at the unlock screen, or stop. Whether it is now as asked.
pub(crate) fn set_guarded(on: bool) -> bool {
    if on == guarded() {
        return true;
    }
    if on {
        // With nothing kept the finger would have nothing to open.
        unlock().is_some() && system::set(GUARDED, UNSEEN).is_ok()
    } else {
        system::forget(GUARDED);
        true
    }
}
