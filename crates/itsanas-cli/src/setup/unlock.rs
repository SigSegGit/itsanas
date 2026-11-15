//! The passphrase of a computer's service, sealed behind Windows Hello or Touch
//! ID instead of lying in a file. Optional and off by default. The price: a
//! machine restarted with nobody signed in does not start by itself.
//!
//! # Once per sign-in, for every account
//!
//! The key the system gives is the same for every account of this person on
//! this machine. When the services start at sign-in, the person has just
//! proved who they are to the system: the key is kept under the system's own
//! protection of this person's data (DPAPI on Windows, the login keychain on a
//! Mac), which that sign-in unlocks, and the services read it there
//! ([`opened_at_start`]). The system asks only when that copy is missing or no
//! longer opens the seal, then keeps it. Three accounts used to ask three
//! times on top of the sign-in itself (Nicolas, 2026-11-15). A delete for good
//! or the 24 words still ask, every time ([`opened`] with [`platform_gate`]).

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

/// Where the system keeps the key between two starts.
pub(crate) trait Remembered {
    fn load(&self) -> Option<[u8; 32]>;
    fn save(&self, key: &[u8; 32]);
    fn forget(&self);
}

/// The passphrase behind the seal when a service starts at sign-in: the key the
/// sign-in already unlocked, and the system asked only when it is missing or
/// does not open this seal, then kept for the next account and the next start.
pub(crate) fn opened_at_start(
    home: &Path,
    kept: &dyn Remembered,
    gate: Gate,
) -> Result<Secret, String> {
    let sealed = std::fs::read(sealed_file(home)).map_err(|e| e.to_string())?;
    let open = |key: [u8; 32]| {
        let plain =
            open_random(&SymmetricKey::new(key), &context(), &sealed).map_err(|e| e.to_string())?;
        String::from_utf8(plain)
            .map(Secret::new)
            .map_err(|e| e.to_string())
    };
    if let Some(key) = kept.load() {
        if let Ok(passphrase) = open(key) {
            return Ok(passphrase);
        }
        // Another key than this seal's: a credential made again. Asked below.
        kept.forget();
    }
    let key = gate()?;
    let passphrase = open(key)?;
    kept.save(&key);
    Ok(passphrase)
}

/// The system's own keeping of the key for this person, unlocked by their sign-in.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SignIn;

impl Remembered for SignIn {
    fn load(&self) -> Option<[u8; 32]> {
        platform_keep::load()
    }
    fn save(&self, key: &[u8; 32]) {
        platform_keep::save(key);
    }
    fn forget(&self) {
        platform_keep::forget();
    }
}

/// Run `change`, which puts the keystore under `new`, and keep the seal in step:
/// the service reads only the seal, and would start next time with a passphrase
/// that opens nothing. The system asks first, and its key must open the seal in
/// place, so a refusal or another person changes nothing at all.
pub(crate) fn changing<T>(
    home: &Path,
    new: &str,
    gate: Gate,
    change: impl FnOnce() -> crate::error::Result<T>,
) -> crate::error::Result<T> {
    if !is_on(home) {
        return change();
    }
    let sealed = sealed_with(home, new, gate).map_err(|why| {
        crate::error::CliError::Usage(format!(
            "the passphrase is unchanged, because the seal could not follow it: {why}"
        ))
    })?;
    let done = change()?;
    let staged = home.join("passphrase.sealed.new");
    std::fs::write(&staged, sealed)
        .and_then(|()| std::fs::rename(&staged, sealed_file(home)))
        .map_err(|e| {
            crate::error::CliError::Usage(format!(
                "the passphrase is changed, but the seal still holds the old one: {e}"
            ))
        })?;
    Ok(done)
}

/// `new` sealed under the key that opens the seal in place, or why not.
fn sealed_with(home: &Path, new: &str, gate: Gate) -> Result<Vec<u8>, String> {
    let held = std::fs::read(sealed_file(home)).map_err(|e| e.to_string())?;
    let key = SymmetricKey::new(gate()?);
    open_random(&key, &context(), &held)
        .map_err(|_| "the system did not give the key that sealed it".to_owned())?;
    seal_random(&key, &context(), new.as_bytes()).map_err(|e| e.to_string())
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

/// The key kept under DPAPI for this Windows user, in a file of their own profile:
/// any program of theirs can read it while they are signed in, as it could read
/// their files, and nobody can without their sign-in.
#[cfg(windows)]
mod platform_keep {
    use windows::Security::Cryptography::CryptographicBuffer;
    use windows::Security::Cryptography::DataProtection::DataProtectionProvider;
    use windows::core::{Array, HSTRING};

    fn file() -> Option<std::path::PathBuf> {
        std::env::var_os("LOCALAPPDATA").map(|dir| {
            std::path::PathBuf::from(dir)
                .join("ITSaNAS")
                .join("unlock.key")
        })
    }

    fn unprotect(sealed: &[u8]) -> windows::core::Result<Vec<u8>> {
        let provider = DataProtectionProvider::new()?;
        let buffer = CryptographicBuffer::CreateFromByteArray(sealed)?;
        let plain = provider.UnprotectAsync(&buffer)?.get()?;
        let mut bytes = Array::<u8>::new();
        CryptographicBuffer::CopyToByteArray(&plain, &mut bytes)?;
        Ok(bytes.to_vec())
    }

    fn protect(key: &[u8]) -> windows::core::Result<Vec<u8>> {
        let provider =
            DataProtectionProvider::CreateOverloadExplicit(&HSTRING::from("LOCAL=user"))?;
        let buffer = CryptographicBuffer::CreateFromByteArray(key)?;
        let sealed = provider.ProtectAsync(&buffer)?.get()?;
        let mut bytes = Array::<u8>::new();
        CryptographicBuffer::CopyToByteArray(&sealed, &mut bytes)?;
        Ok(bytes.to_vec())
    }

    pub(super) fn load() -> Option<[u8; 32]> {
        let sealed = std::fs::read(file()?).ok()?;
        unprotect(&sealed).ok()?.try_into().ok()
    }

    pub(super) fn save(key: &[u8; 32]) {
        let (Some(file), Ok(sealed)) = (file(), protect(key)) else {
            return;
        };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, sealed);
    }

    pub(super) fn forget() {
        if let Some(file) = file() {
            let _ = std::fs::remove_file(file);
        }
    }

    /// The system's protection itself, without the file: what is protected
    /// for this user comes back the same, and the protected bytes are not it.
    #[cfg(test)]
    #[test]
    fn a_key_protected_for_this_user_comes_back_and_is_not_kept_in_clear() {
        let key = [42u8; 32];
        let sealed = protect(&key).expect("protect");
        assert!(
            !sealed.windows(32).any(|window| window == key),
            "the key lies in clear in the file that keeps it"
        );
        assert_eq!(unprotect(&sealed).expect("unprotect"), key);
    }
}

/// On a Mac the key is in the login keychain already (`touch`): the sign-in
/// unlocked it, so reading it asks nothing.
#[cfg(target_os = "macos")]
mod platform_keep {
    pub(super) fn load() -> Option<[u8; 32]> {
        super::touch::kept()
    }
    pub(super) fn save(_: &[u8; 32]) {}
    pub(super) fn forget() {}
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform_keep {
    pub(super) fn load() -> Option<[u8; 32]> {
        None
    }
    pub(super) fn save(_: &[u8; 32]) {}
    pub(super) fn forget() {}
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

    /// The key, read from the login keychain without asking: what the sign-in
    /// unlocked.
    pub(super) fn kept() -> Option<[u8; 32]> {
        get_generic_password(SERVICE, ACCOUNT)
            .ok()
            .map(|key| *blake3::hash(&key).as_bytes())
    }

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

    /// A passphrase changed while sealed is the one the seal gives, and a
    /// refusal or another person changes neither the keystore nor the seal.
    /// Sabotage: skip writing the new seal; skip the check that the key opens
    /// the seal in place.
    #[test]
    fn red_team_a_passphrase_changed_while_sealed_is_the_one_the_seal_gives() {
        let dir = tempfile::tempdir().expect("scratch");
        enable(dir.path(), &stand(), the_person).expect("seals");
        for gate in [a_refusal as Gate, someone_else] {
            let mut ran = false;
            let refused = changing(dir.path(), "the new one", gate, || {
                ran = true;
                Ok(())
            });
            assert!(
                refused.is_err() && !ran,
                "the keystore changed though the seal could not follow it"
            );
        }
        changing(dir.path(), "the new one", the_person, || Ok(())).expect("changes");
        assert_eq!(
            opened(dir.path(), the_person).expect("opens").as_str(),
            "the new one",
            "the service would start with the old passphrase, which opens nothing now"
        );
    }

    /// The key kept between starts, in memory.
    struct Kept(RefCell<Option<[u8; 32]>>);

    impl Remembered for Kept {
        fn load(&self) -> Option<[u8; 32]> {
            *self.0.borrow()
        }
        fn save(&self, key: &[u8; 32]) {
            *self.0.borrow_mut() = Some(*key);
        }
        fn forget(&self) {
            *self.0.borrow_mut() = None;
        }
    }

    static ASKED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

    fn the_person_counted() -> std::result::Result<[u8; 32], String> {
        ASKED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok([7; 32])
    }

    /// Three accounts on one machine, their services started at sign-in: the
    /// system asks once, not three times on top of the sign-in, and a key kept
    /// from another credential is dropped and asked again rather than locking
    /// every account out. Sabotage: make `opened_at_start` ignore what is kept.
    #[test]
    fn red_team_three_accounts_at_sign_in_ask_the_system_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let homes: Vec<_> = (0..3)
            .map(|n| dir.path().join(format!("account-{n}")))
            .collect();
        for home in &homes {
            std::fs::create_dir_all(home).expect("home");
            enable(home, &stand(), the_person).expect("sealed");
        }
        let kept = Kept(RefCell::new(None));
        ASKED.store(0, std::sync::atomic::Ordering::SeqCst);
        for home in &homes {
            let passphrase = opened_at_start(home, &kept, the_person_counted).expect("opened");
            assert_eq!(passphrase.as_str(), "kept here");
        }
        assert_eq!(
            ASKED.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "every account asked the system again: three prompts on top of the sign-in"
        );

        *kept.0.borrow_mut() = Some([8; 32]);
        let passphrase = opened_at_start(&homes[0], &kept, the_person_counted).expect("opened");
        assert_eq!(passphrase.as_str(), "kept here");
        assert_eq!(
            kept.load(),
            Some([7; 32]),
            "a key that no longer opens the seal stayed kept: every start would fail"
        );
        assert!(
            opened_at_start(&homes[0], &Kept(RefCell::new(None)), a_refusal).is_err(),
            "a refusal opened the seal"
        );
    }
}
