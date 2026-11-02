//! Showing the 24 words again, and changing the passphrase, from Settings.
//!
//! Both start from the passphrase the person types in a native window, never in
//! the page: what comes back to the page is a sentence, not a secret. The words
//! are shown by the same window that showed them when the account was made.

use std::path::Path;

use itsanas_node::node::Identity;

use super::{SecretPrompt, ServiceControl, forgot, sign};
use crate::{
    error::{CliError, Result},
    node::Node,
};

/// Ask the passphrase, then show the account's 24 words in the prompt's window.
pub(crate) fn show_words(home: &Path, prompt: &mut dyn SecretPrompt) -> Result<String> {
    if !Node::exists(home) {
        return Err(CliError::NoNode(home.to_path_buf()));
    }
    let passphrase = prompt.passphrase(&sign::account_purpose(home, "to show its 24 words"))?;
    // Wrong passphrase: refused here, with nothing shown.
    let phrase = Identity::recovery_phrase(home, &passphrase)?;
    prompt.show_words(phrase.as_str())?;
    Ok("The 24 words were shown in the window. Nothing was kept anywhere new.".to_owned())
}

/// Change the passphrase of this machine's keystore: the current one is asked
/// first and proven, then the new one (asked again when refused), and the copy
/// the service reads is renewed when there was one. The same re-sealing
/// `itsanas passphrase` does.
pub(crate) fn change_passphrase(
    home: &Path,
    prompt: &mut dyn SecretPrompt,
    service: &dyn ServiceControl,
) -> Result<String> {
    if !Node::exists(home) {
        return Err(CliError::NoNode(home.to_path_buf()));
    }
    let current = prompt.passphrase(&sign::account_purpose(home, "to change it"))?;
    Identity::secrets(home, &current)?;
    let new = forgot::new_passphrase(prompt, service)?;
    let renew = service.stored_passphrase().is_some();
    Node::change_passphrase(home, &current, &new)?;
    if renew {
        // Otherwise the service would start next time with a passphrase that
        // opens nothing.
        service.store_passphrase(&new)?;
    }
    Ok(if renew {
        "The passphrase is changed, and the background service has the new one.".to_owned()
    } else {
        "The passphrase is changed.".to_owned()
    })
}
