//! The passphrase of a computer's service, sealed behind Windows Hello or Touch
//! ID instead of lying in a file. Optional and off by default. The system asks
//! once, when the session opens; refused, the service stays stopped and the
//! passphrase is typed. The price: a machine restarted with nobody signed in
//! does not start by itself.

use std::path::{Path, PathBuf};

use itsanas_crypto::{SealContext, SymmetricKey, UserId, open_random, seal_random};

use super::secrets::Secret;
use super::service::ServiceControl;

/// Where the clear passphrase lives while nothing seals it.
pub(crate) trait Keeper {
    fn stored(&self) -> Option<Secret>;
    fn store(&self, passphrase: &Secret) -> crate::error::Result<()>;
    fn forget(&self) -> crate::error::Result<()>;
}

impl Keeper for dyn ServiceControl {
    fn stored(&self) -> Option<Secret> {
        self.stored_passphrase()
    }
    fn store(&self, passphrase: &Secret) -> crate::error::Result<()> {
        self.store_passphrase(passphrase).map(|_| ())
    }
    fn forget(&self) -> crate::error::Result<()> {
        self.forget_passphrase().map(|_| ())
    }
}

/// What only the person at this machine can produce, each time the same.
pub(crate) type Gate = fn() -> Result<[u8; 32], String>;

const PURPOSE: &str = "itsanas service passphrase";

fn context() -> SealContext<'static> {
    SealContext {
        purpose: PURPOSE,
        owner: UserId::from_bytes([0; 32]),
        address: b"",
    }
}

fn sealed_file(home: &Path) -> PathBuf {
    home.join("passphrase.sealed")
}

/// Whether this machine's passphrase is sealed.
pub(crate) fn is_on(home: &Path) -> bool {
    sealed_file(home).is_file()
}

/// Seal what the service remembers, then forget the file: both never remain.
pub(crate) fn enable(
    home: &Path,
    service: &(impl Keeper + ?Sized),
    gate: Gate,
) -> Result<(), String> {
    let passphrase = service
        .stored()
        .ok_or("no passphrase is kept on this machine to protect")?;
    let key = SymmetricKey::new(gate()?);
    let sealed = seal_random(&key, &context(), passphrase.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::write(sealed_file(home), sealed).map_err(|e| e.to_string())?;
    // Read back before the only other copy goes.
    if !opened(home, gate).is_ok_and(|back| back.as_str() == passphrase.as_str()) {
        let _ = std::fs::remove_file(sealed_file(home));
        return Err(
            "the system did not give the same answer twice; nothing was changed".to_owned(),
        );
    }
    service.forget().map_err(|e| e.to_string())?;
    Ok(())
}

/// Put the passphrase back in the service's file and drop the seal.
pub(crate) fn disable(
    home: &Path,
    service: &(impl Keeper + ?Sized),
    gate: Gate,
) -> Result<(), String> {
    let passphrase = opened(home, gate)?;
    service.store(&passphrase).map_err(|e| e.to_string())?;
    std::fs::remove_file(sealed_file(home)).map_err(|e| e.to_string())
}

/// The passphrase behind the seal: the system asks the person.
pub(crate) fn opened(home: &Path, gate: Gate) -> Result<Secret, String> {
    let sealed = std::fs::read(sealed_file(home)).map_err(|e| e.to_string())?;
    let key = SymmetricKey::new(gate()?);
    let plain = open_random(&key, &context(), &sealed).map_err(|e| e.to_string())?;
    String::from_utf8(plain)
        .map(Secret::new)
        .map_err(|e| e.to_string())
}

/// This system's way of asking.
pub(crate) fn platform_gate() -> Result<[u8; 32], String> {
    #[cfg(windows)]
    return hello::secret();
    #[cfg(target_os = "macos")]
    return touch::secret();
    #[cfg(not(any(windows, target_os = "macos")))]
    Err("this system has no unlock this program can ask for".to_owned())
}

/// A sign-in the system verifies, turned into a key: Windows Hello signs a fixed
/// challenge with a key it keeps, and that signature is the same each time.
#[cfg(windows)]
mod hello {
    use windows::Security::Credentials::{
        KeyCredentialCreationOption, KeyCredentialManager, KeyCredentialStatus,
    };
    use windows::Security::Cryptography::CryptographicBuffer;
    use windows::core::{Array, HSTRING};

    const CHALLENGE: &[u8] = b"itsanas service passphrase, version 1";

    pub(super) fn secret() -> Result<[u8; 32], String> {
        ask().map_err(|e| format!("Windows Hello: {e}"))?
    }

    fn ask() -> windows::core::Result<Result<[u8; 32], String>> {
        if !KeyCredentialManager::IsSupportedAsync()?.get()? {
            return Ok(Err("it is not set up on this machine".to_owned()));
        }
        let name = HSTRING::from("ITSaNAS");
        let mut found = KeyCredentialManager::OpenAsync(&name)?.get()?;
        if found.Status()? != KeyCredentialStatus::Success {
            found = KeyCredentialManager::RequestCreateAsync(
                &name,
                KeyCredentialCreationOption::FailIfExists,
            )?
            .get()?;
            if found.Status()? != KeyCredentialStatus::Success {
                return Ok(Err("it was refused".to_owned()));
            }
        }
        let challenge = CryptographicBuffer::CreateFromByteArray(CHALLENGE)?;
        let signed = found.Credential()?.RequestSignAsync(&challenge)?.get()?;
        if signed.Status()? != KeyCredentialStatus::Success {
            return Ok(Err("it was refused".to_owned()));
        }
        let mut bytes = Array::<u8>::new();
        CryptographicBuffer::CopyToByteArray(&signed.Result()?, &mut bytes)?;
        Ok(Ok(*blake3::hash(&bytes).as_bytes()))
    }
}

/// Touch ID asks; the key is in the login keychain. An unsigned program cannot
/// tie a keychain item to the fingerprint itself, so this is less strong than
/// Windows Hello: the keychain alone would give the key to the logged-in user.
#[cfg(target_os = "macos")]
mod touch {
    use security_framework::passwords::{get_generic_password, set_generic_password};

    const SERVICE: &str = "ITSaNAS";
    const ACCOUNT: &str = "service passphrase key";

    pub(super) fn secret() -> Result<[u8; 32], String> {
        if !itsanas_touchid::ask("unlock ITSaNAS") {
            return Err("Touch ID refused".to_owned());
        }
        if let Ok(key) = get_generic_password(SERVICE, ACCOUNT) {
            return Ok(*blake3::hash(&key).as_bytes());
        }
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|e| e.to_string())?;
        set_generic_password(SERVICE, ACCOUNT, &key).map_err(|e| e.to_string())?;
        Ok(*blake3::hash(&key).as_bytes())
    }
}

#[cfg(test)]
#[allow(clippy::unnecessary_wraps)]
mod tests {
    use super::*;
    use crate::error::Result;
    use std::cell::RefCell;

    struct Stand(RefCell<Option<Secret>>);

    impl Keeper for Stand {
        fn stored(&self) -> Option<Secret> {
            self.0.borrow().clone()
        }
        fn store(&self, passphrase: &Secret) -> Result<()> {
            *self.0.borrow_mut() = Some(passphrase.clone());
            Ok(())
        }
        fn forget(&self) -> Result<()> {
            *self.0.borrow_mut() = None;
            Ok(())
        }
    }

    fn stand() -> Stand {
        Stand(RefCell::new(Some(Secret::new("kept here".to_owned()))))
    }

    fn the_person() -> std::result::Result<[u8; 32], String> {
        Ok([7; 32])
    }
    fn a_refusal() -> std::result::Result<[u8; 32], String> {
        Err("refused".to_owned())
    }
    fn someone_else() -> std::result::Result<[u8; 32], String> {
        Ok([8; 32])
    }
    fn changes_its_mind() -> std::result::Result<[u8; 32], String> {
        static CALLS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
        let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok([call; 32])
    }

    /// On: the clear copy is gone, and the seal gives the passphrase to the
    /// person and to nobody else. Off: it comes back. Sabotage: skip
    /// `forget_passphrase`; seal with a fixed key.
    #[test]
    fn red_team_a_sealed_passphrase_opens_for_the_person_and_leaves_no_clear_copy() {
        let dir = tempfile::tempdir().expect("scratch");
        let service = stand();
        enable(dir.path(), &service, the_person).expect("seals");
        assert!(
            service.stored().is_none(),
            "the clear file survived: the fingerprint protects nothing"
        );
        assert_eq!(
            opened(dir.path(), the_person).expect("opens").as_str(),
            "kept here"
        );
        assert!(
            opened(dir.path(), someone_else).is_err(),
            "the seal opened with another key"
        );
        assert!(opened(dir.path(), a_refusal).is_err());
        disable(dir.path(), &service, the_person).expect("unseals");
        assert!(!is_on(dir.path()));
        assert_eq!(service.stored().expect("back").as_str(), "kept here");
    }

    /// A refusal, or a system that does not answer the same twice, leaves the
    /// passphrase where it was: otherwise nobody could ever start this machine
    /// again. Sabotage: forget before reading back.
    #[test]
    fn red_team_a_seal_that_cannot_be_opened_never_replaces_the_passphrase() {
        let dir = tempfile::tempdir().expect("scratch");
        let service = stand();
        assert!(enable(dir.path(), &service, a_refusal).is_err());
        assert!(enable(dir.path(), &service, changes_its_mind).is_err());
        assert!(
            !is_on(dir.path()),
            "a seal nobody can open was left in place"
        );
        assert_eq!(
            service.stored().expect("still there").as_str(),
            "kept here",
            "the machine's only passphrase was lost to a seal that does not open"
        );
    }
}
