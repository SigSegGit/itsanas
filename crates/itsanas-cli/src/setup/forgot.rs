//! "I forgot my passphrase": the 24 words replace the keys on this machine.
//!
//! Reachable before unlocking, which is the point: nothing here needs the old
//! passphrase, only the words the account was made with. The new keystore is
//! sealed under a passphrase chosen now; the account is the same, the device is
//! a new one (a device key is made with the node), so the old device is asked to
//! be forgotten once the new one is enrolled.
//!
//! Order matters, because a mistake here destroys the only copy of the keys:
//! everything that can refuse (the words, whose account they open, the new
//! passphrase) is settled before anything is touched; the old node is set aside
//! rather than deleted until the new one exists, and put back if it does not.

use std::path::{Path, PathBuf};
use std::time::Duration;

use itsanas_crypto::{DeviceId, MasterSecret, UserKeys};
use itsanas_node::node::VAULT_MARKER;
use zeroize::Zeroizing;

use super::{OwnerCheck, Secret, SecretPrompt, ServiceControl, service, wait_until};
use crate::{
    config::Config,
    error::{CliError, Result},
    node::Node,
};

/// How many times the new passphrase is asked for before the whole thing stops.
const ATTEMPTS: usize = 3;

/// Replace the node in `home` with one made from the words the prompt is given,
/// sealed under a new passphrase. Says what was done.
///
/// `owner_check` asks the coordinator whose account the name is: words of another
/// account are refused, and a coordinator that cannot be asked does not stop a
/// person from recovering their own.
pub(crate) fn forgot(
    home: &Path,
    prompt: &mut dyn SecretPrompt,
    service: &dyn ServiceControl,
    owner_check: OwnerCheck,
) -> Result<String> {
    if !Node::exists(home) {
        return Err(CliError::NoNode(home.to_path_buf()));
    }
    let config = Config::load(&Node::config_path(home))?;

    let typed = prompt.recovery_phrase()?;
    let words = Zeroizing::new(crate::phrase_words(&typed));
    let master = MasterSecret::from_recovery_phrase(&words)?;
    refuse_words_of_another_account(&config, &master, owner_check)?;
    let passphrase = new_passphrase(prompt, service)?;

    let old_device = std::fs::read_to_string(Node::vault_path(home, &config).join(VAULT_MARKER))
        .ok()
        .and_then(|text| text.trim().parse::<DeviceId>().ok());

    // Nothing can refuse from here on but the disk. The service is stopped so
    // the store can move; it is started again whatever happens.
    let managed = service.installed();
    if managed {
        service.stop()?;
        if !wait_until(Duration::from_secs(20), || {
            !itsanas_store::Store::is_locked(Node::store_path(home))
        }) {
            let _ = service.start();
            return Err(CliError::Usage(
                "the background service still holds this node twenty seconds after being asked \
                 to stop; nothing was changed"
                    .to_owned(),
            ));
        }
    }
    let result = replace(home, &config, &words, &passphrase, old_device);
    if managed {
        if result.is_ok() {
            // The new passphrase, where the service reads it: the old one opens
            // nothing now.
            let _ = service.store_passphrase(&passphrase);
        }
        let _ = service.start();
    }
    result
}

/// The account the name belongs to at the coordinator, when it answers, must be
/// the one the words open.
fn refuse_words_of_another_account(
    config: &Config,
    master: &MasterSecret,
    owner_check: OwnerCheck,
) -> Result<()> {
    let Some(address) = config.coordinator.as_deref() else {
        return Ok(());
    };
    if config.username.is_empty() {
        return Ok(());
    }
    if let Ok(Some(owner)) = owner_check(address, &config.username)
        && owner != UserKeys::derive(master).user_id()
    {
        return Err(CliError::Usage(format!(
            "these words are not the words of the account {:?}; nothing was changed",
            config.username
        )));
    }
    Ok(())
}

/// A new passphrase, asked again when it is refused.
pub(super) fn new_passphrase(
    prompt: &mut dyn SecretPrompt,
    service: &dyn ServiceControl,
) -> Result<Secret> {
    let mut last = None;
    for _ in 0..ATTEMPTS {
        let passphrase = prompt.new_passphrase()?;
        let refusal = crate::node::check_new_passphrase(&passphrase)
            .err()
            .or_else(|| {
                service
                    .installed()
                    .then(|| service::fits_service_file(std::env::consts::OS, &passphrase).err())
                    .flatten()
            });
        match refusal {
            None => return Ok(passphrase),
            Some(error) => last = Some(error),
        }
    }
    Err(last.unwrap_or_else(|| CliError::Usage("no passphrase was given".to_owned())))
}

/// Set the old node aside, make the new one, drop the old; put the old back when
/// the new could not be made.
fn replace(
    home: &Path,
    config: &Config,
    words: &str,
    passphrase: &Secret,
    old_device: Option<DeviceId>,
) -> Result<String> {
    let aside = set_aside(home)?;
    let mut node = match Node::restore(home, passphrase, &config.username, words) {
        Ok(node) => node,
        Err(error) => {
            let _ = std::fs::remove_dir_all(home);
            let _ = std::fs::rename(&aside, home);
            return Err(error);
        }
    };
    // The same machine, the same account: what was set up stays, but the vault
    // of the old device belonged to it alone.
    node.config = Config {
        vault: None,
        ..config.clone()
    };
    if let Err(error) = node.save_config() {
        drop(node);
        let _ = std::fs::remove_dir_all(home);
        let _ = std::fs::rename(&aside, home);
        return Err(error);
    }
    let _ = std::fs::remove_dir_all(&aside);

    // Best effort, and last: the keys are replaced whether or not the network
    // answers. A device that cannot be forgotten now can be, from Settings.
    let still_listed = node.config.coordinator.is_some()
        && crate::register_only(&node).is_ok()
        && old_device.is_some_and(|old| {
            crate::coordinator::forget_device(&node, old, itsanas_discover::now_unix()).is_err()
        });
    let leftover = if still_listed {
        " The machine's old device is still listed with the coordinator: forget it from \
         another device of the account."
    } else {
        ""
    };
    Ok(format!(
        "The keys on this machine are replaced: the account {:?} is the same, sealed under \
         the new passphrase.{leftover}",
        config.username
    ))
}

/// Move the node to a name beside it, so that it can be put back.
fn set_aside(home: &Path) -> Result<PathBuf> {
    let name = home.file_name().map_or_else(
        || "node".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let aside = home.with_file_name(format!("{name}.replaced-{}", itsanas_discover::now_unix()));
    std::fs::rename(home, &aside).map_err(|source| CliError::Io {
        path: home.to_path_buf(),
        source,
    })?;
    Ok(aside)
}
