//! An email address bound to the account, as the coordinator keeps it
//! (`itsanas_coord::service` email messages).

use itsanas_coord::protocol::{EMAIL_VERSION, EmailStatus, Request, Response};
use itsanas_coord::server::CoordClient;
use itsanas_crypto::DeviceId;

use super::dial;
use crate::node::Node;
use crate::{NodeError as CliError, Result};

/// What the member is told when their coordinator predates email.
pub const NO_EMAIL_HERE: &str =
    "this coordinator is older than email addresses; it cannot bind one until it is updated";

/// Bind `address` to the account (the coordinator mails a code), or unbind
/// it with `None`.
///
/// # Errors
///
/// The coordinator's refusal, or one too old to speak of email.
pub fn bind_email(node: &Node, address: Option<&str>) -> Result<()> {
    let mut client = dial(node)?;
    done(
        &mut client,
        &Request::BindEmail {
            address: address.map(str::to_owned),
        },
    )
}

/// Give back the code [`bind_email`] had mailed.
///
/// # Errors
///
/// A wrong or ended code, or the coordinator's refusal.
pub fn confirm_email(node: &Node, code: &str) -> Result<()> {
    let mut client = dial(node)?;
    done(
        &mut client,
        &Request::ConfirmEmail {
            code: code.trim().to_owned(),
        },
    )
}

/// The address bound to the account, if any, and whether it is verified.
///
/// # Errors
///
/// The coordinator's refusal, or one too old to speak of email.
pub fn email_status(node: &Node) -> Result<Option<EmailStatus>> {
    let mut client = dial(node)?;
    recent(&client)?;
    match client.ask(&Request::Email)? {
        Response::Email(status) => Ok(status),
        Response::Refused(why) => Err(CliError::Usage(why)),
        other => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
    }
}

/// Ask `address`'s coordinator to mail a recovery code for `username` if
/// the account has a verified address. Says nothing either way, by design;
/// a coordinator too old for email is simply not asked.
///
/// # Errors
///
/// If the coordinator cannot be reached.
pub fn ask_recovery_code(address: &str, expect: Option<DeviceId>, username: &str) -> Result<()> {
    let device = itsanas_crypto::DeviceKeys::generate()?;
    let mut client = CoordClient::connect(address, &device, expect)
        .map_err(|error| CliError::Usage(format!("{address}: {error}")))?;
    if client.version() >= EMAIL_VERSION {
        let _ = client.ask(&Request::AskRecovery {
            username: username.to_owned(),
        })?;
    }
    Ok(())
}

/// Ask `address`'s coordinator to mail the account name bound to `email`.
///
/// # Errors
///
/// If the coordinator cannot be reached, or predates email.
pub fn remind_username(address: &str, expect: Option<DeviceId>, email: &str) -> Result<()> {
    let device = itsanas_crypto::DeviceKeys::generate()?;
    let mut client = CoordClient::connect(address, &device, expect)
        .map_err(|error| CliError::Usage(format!("{address}: {error}")))?;
    done(
        &mut client,
        &Request::RemindUsername {
            address: email.trim().to_owned(),
        },
    )
}

fn recent(client: &CoordClient) -> Result<()> {
    if client.version() < EMAIL_VERSION {
        return Err(CliError::Usage(NO_EMAIL_HERE.to_owned()));
    }
    Ok(())
}

fn done(client: &mut CoordClient, request: &Request) -> Result<()> {
    recent(client)?;
    match client.ask(request)? {
        Response::Done => Ok(()),
        Response::Refused(why) => Err(CliError::Usage(why)),
        other => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
    }
}
