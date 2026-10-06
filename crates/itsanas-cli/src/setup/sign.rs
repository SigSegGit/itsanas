//! `itsanas signout` and `itsanas signin`: 0f's "Disconnect", from a terminal.
//!
//! Signing out is not leaving. The keys, the store and what this machine
//! holds for other members stay on this disk; only the passphrase file the
//! background service reads is deleted, and the service is stopped and kept
//! from starting at logon -- so nothing on this machine can unlock the
//! account until somebody types the passphrase again. That is what a person
//! lending the machine, or selling it next week, wants from "sign out".
//! Leaving the network for good is `itsanas leave`; removing everything is
//! `install/clean.*`.

use std::{path::Path, time::Duration};

use itsanas_node::node::Identity;

use super::{SecretPrompt, ServiceControl, secrets, service, wait_until};
use crate::{
    error::{CliError, Result},
    node::Node,
};

/// What signing out means for the others, in plain words. Said every time:
/// a person who signs out for a month should know whom it costs.
pub(crate) const SIGNED_OUT: &str = "Signed out. Your files, this machine's keys and what it \
     holds for the other members all stay on this disk. While signed out this machine neither \
     syncs nor answers the others' checks, so the copies it keeps for them stop counting until \
     you come back. Sign back in with `itsanas signin`.";

/// Stop the service, keep it from starting at logon, and delete the file it
/// reads the passphrase from. Returns what was done, for a terminal or a page.
pub(crate) fn sign_out(home: &Path, service: &dyn ServiceControl) -> Result<String> {
    if !Node::exists(home) {
        return Err(CliError::NoNode(home.to_path_buf()));
    }
    let mut done = Vec::new();
    if service.installed() {
        // Autostart off first: a service that restarts on failure would
        // otherwise be started again by its supervisor while the file goes.
        service.set_autostart(false)?;
        let _ = service.stop();
        done.push("the background service is stopped and will not start at logon");
    }
    let store = Node::store_path(home);
    if !wait_until(Duration::from_secs(20), || {
        !itsanas_store::Store::is_locked(&store)
    }) {
        return Err(CliError::Usage(
            "a daemon still holds this node (one started by hand? stop it with Ctrl+C where \
             it runs) and the passphrase file was left in place"
                .to_owned(),
        ));
    }
    let file = service.passphrase_file();
    match std::fs::remove_file(&file) {
        Ok(()) => done.push("the passphrase file is deleted"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            done.push("there was no passphrase file to delete");
        }
        Err(source) => return Err(CliError::Io { path: file, source }),
    }
    Ok(done.join("; "))
}

/// Ask the passphrase, prove it opens this machine's keys, write it back
/// where the service reads it, and start the service.
pub(crate) fn sign_in(
    home: &Path,
    service: &dyn ServiceControl,
    prompt: &mut dyn SecretPrompt,
) -> Result<String> {
    if !Node::exists(home) {
        return Err(CliError::NoNode(home.to_path_buf()));
    }
    let passphrase = prompt.passphrase("This machine's passphrase, to sign back in:")?;
    // Checked before anything is written: a mistyped passphrase saved for
    // the service is a daemon that fails at every logon, silently.
    Identity::open(home, &passphrase)?;
    let file = service.passphrase_file();
    service::write_passphrase_file(&file, &passphrase)?;
    if !service.installed() {
        return Ok(format!(
            "signed in: {} opens this machine's keys again. No background service is \
             installed; `itsanas setup` installs one, or start the node with `itsanas daemon`",
            file.display()
        ));
    }
    service.set_autostart(true)?;
    service.start()?;
    Ok("signed in: the background service runs again and starts at logon".to_owned())
}

/// `itsanas signout`.
pub(crate) fn signout(home: &Path, instance: Option<&str>) -> Result<()> {
    let platform = service::Platform::of_this_machine(home, instance);
    let done = sign_out(home, &platform)?;
    println!("{done}.");
    println!();
    println!("{SIGNED_OUT}");
    Ok(())
}

/// `itsanas signin`.
pub(crate) fn signin(home: &Path, instance: Option<&str>) -> Result<()> {
    let platform = service::Platform::of_this_machine(home, instance);
    let mut prompt = secrets::Native::new(secrets::choose()?);
    println!("{}", sign_in(home, &platform, &mut prompt)?);
    Ok(())
}
