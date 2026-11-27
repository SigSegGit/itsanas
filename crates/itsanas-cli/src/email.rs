//! `itsanas email`: the address bound to the account, and the code a
//! recovery from the passphrase asks once that address is verified.

use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::Path;

use itsanas_node::coordinator;

use crate::cli::EmailAction;
use crate::{CliError, Result, open};

/// Where a recovery reads the mailed code when there is no terminal.
pub const EMAIL_CODE_ENV: &str = "ITSANAS_EMAIL_CODE";

pub(crate) fn email_command(home: &Path, action: Option<EmailAction>) -> Result<()> {
    match action {
        None => show(home),
        Some(EmailAction::Set { address }) => set(home, &address),
        Some(EmailAction::Confirm { code }) => {
            let node = open(home)?;
            coordinator::confirm_email(&node, &code)?;
            println!("Address verified.");
            println!("Recovering this account from its passphrase now also needs a code mailed");
            println!("there. The 24 words never do.");
            Ok(())
        }
        Some(EmailAction::Remove) => {
            let node = open(home)?;
            coordinator::bind_email(&node, None)?;
            println!("No address any more: a recovery from the passphrase needs no mailed code.");
            Ok(())
        }
        Some(EmailAction::Remind {
            address,
            coordinator: at,
            device,
        }) => {
            let expect = device
                .as_deref()
                .map(coordinator::parse_device)
                .transpose()?;
            coordinator::remind_username(&at, expect, &address)?;
            println!("If an account's verified address is {address}, its name was mailed there.");
            Ok(())
        }
    }
}

fn show(home: &Path) -> Result<()> {
    let node = open(home)?;
    match coordinator::email_status(&node)? {
        None => println!("No address. `itsanas email set <address>` binds one."),
        Some(status) if status.verified => println!("{} (verified)", status.address),
        Some(status) => {
            println!("{} (waiting for its code)", status.address);
            println!("`itsanas email confirm <code>` with the code mailed there.");
        }
    }
    Ok(())
}

fn set(home: &Path, address: &str) -> Result<()> {
    let node = open(home)?;
    println!("Once {address} is verified, recovering this account from its passphrase");
    println!("will also need a code mailed there: lose that mailbox and only the 24 words,");
    println!("which never need one, or another machine of the account, which can remove");
    println!("the address, get you back in.");
    coordinator::bind_email(&node, Some(address))?;
    println!();
    println!("A code was mailed to {address}.");
    if !std::io::stdin().is_terminal() {
        println!("`itsanas email confirm <code>` with it, within 15 minutes.");
        return Ok(());
    }
    let code = read_code("The code (Enter to type it later with `itsanas email confirm`): ")?;
    if code.is_empty() {
        return Ok(());
    }
    coordinator::confirm_email(&node, &code)?;
    println!("Address verified.");
    Ok(())
}

/// The mailed code a recovery from `address` needs for `username`: the
/// coordinator is asked to mail it (it says nothing of whether it did), then
/// it is read from the terminal, or from [`EMAIL_CODE_ENV`] without one.
/// Empty for an account with no address.
pub(crate) fn recovery_code(
    address: &str,
    expect: Option<itsanas_crypto::DeviceId>,
    username: &str,
) -> Result<String> {
    if let Ok(code) = std::env::var(EMAIL_CODE_ENV) {
        return Ok(code.trim().to_owned());
    }
    if !std::io::stdin().is_terminal() {
        return Ok(String::new());
    }
    coordinator::ask_recovery_code(address, expect, username)?;
    println!("If this account has an email address, a code was just mailed to it.");
    read_code("The code (Enter if the account has no address): ")
}

fn read_code(prompt: &str) -> Result<String> {
    print!("{prompt}");
    std::io::stdout()
        .flush()
        .map_err(|error| CliError::Usage(error.to_string()))?;
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|error| CliError::Usage(error.to_string()))?;
    Ok(line.trim().to_owned())
}
