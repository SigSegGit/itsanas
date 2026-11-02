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
pub use crate::setup::web::{Backends, Features, Host, Mode, NoHost, Server};

// What the phone's few remaining native calls need of the shared code. The rest
// of a shell's reach is the page, served by [`Server`].
pub use crate::account::{Network, describe, restore_account};
pub use crate::failure::{Answer, Failure};
pub use crate::rounds::{NO_MACHINE_YET, RoundTotals, run_round, targets};
