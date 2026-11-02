//! What another shell may take from this crate: the phone's app.
//!
//! A feature lives in the library; a shell draws only what it alone can. The
//! setup engine and the page are the only first run and settings there are. A
//! shell supplies the two things the engine asks of it -- where a secret is
//! asked ([`SecretPrompt`]) and how the node is run in the background
//! ([`ServiceControl`]) -- and a [`Backends`] naming what it has
//! ([`Features`]); [`Server`] then serves the page on 127.0.0.1 for its web
//! view. Everything else in the crate stays private.

pub use crate::error::{CliError, Result};
pub use crate::setup::secrets::{Secret, SecretPrompt};
pub use crate::setup::service::ServiceControl;
pub use crate::setup::web::{
    Backends, Features, Held, Host, MakePrompt, MakeService, Mode, NoHost, Nodes, Server, Slot,
    Stop,
};

// What the phone's few remaining native calls need of the shared code. The rest
// of a shell's reach is the page, served by [`Server`].
pub use crate::account::{Network, after_open, check_words, describe, restore_account};
pub use crate::failure::{Answer, Failure};
pub use crate::rounds::{NO_MACHINE_YET, RoundTotals, run_round, targets};

/// Recover the passphrase of the account at `home` from its 24 words, asked
/// through `prompt`: the one setup step a locked phone needs before any node
/// is open, so before there is a page to do it. The coordinator is asked whose
/// account the name is, as on a computer.
///
/// # Errors
/// What `itsanas forgot` would say.
pub fn forgot(
    home: &std::path::Path,
    prompt: &mut dyn SecretPrompt,
    service: &dyn ServiceControl,
) -> Result<String> {
    crate::setup::forgot::forgot(home, prompt, service, itsanas_node::coordinator::name_owner)
}
