//! Making, joining and keeping an account: `init`, `login`, `passphrase`,
//! `check-passphrase`, `whoami` and `words`.

use std::{
    fmt::Write as _,
    io::IsTerminal as _,
    path::{Path, PathBuf},
};

use crate::opening::{NEW_PASSPHRASE_ENV, open, passphrase};
use crate::ports::settle_listen_port;
use crate::{coordinator, node, setup};
use crate::{
    error::{CliError, Result},
    node::{Node, phrase_words},
};

pub(crate) fn init(home: &Path, username: &str) -> Result<()> {
    if Node::exists(home) {
        return Err(CliError::NodeExists(home.to_path_buf()));
    }

    println!("Creating a new account at {}.", home.display());
    println!(
        "The passphrase protects this machine's copy of your keys, and the \
         escrow copy a coordinator would hold. Choose a long one."
    );

    let (mut node, phrase) = Node::create(home, &passphrase(true)?, username)?;
    settle_listen_port(&mut node)?;

    println!();
    println!("Account created.");
    println!("  username : {username}");
    println!("  user id  : {}", node.store.owner());
    println!("  device   : {}", node.store.device_id());
    println!();
    println!("┌─ RECOVERY PHRASE ─────────────────────────────────────────────┐");
    println!("│ Write these 24 words down, on paper, and keep them somewhere  │");
    println!("│ your house burning down would not reach.                      │");
    println!("│                                                               │");
    println!("│ They ARE your data. Anyone who has them can read everything    │");
    println!("│ you store. If you lose them AND forget your passphrase, every │");
    println!("│ byte is gone — there is no reset, and nobody can help you.     │");
    println!("└───────────────────────────────────────────────────────────────┘");
    println!();

    print!("{}", phrase_grid(phrase.as_str()));
    println!();
    println!("This phrase is shown once and is not stored anywhere on this machine.");
    println!();
    // `serve` serves peers and never syncs, so an account set up by following
    // this line hosted other people's data and never moved its own -- and the
    // first thing `itsanas status` then says is "synced folder none". The
    // order below is the one a person actually needs, and `daemon` is the
    // process they want.
    println!("Next, in order:");
    println!("  itsanas folder <path>     the directory kept in step with this account");
    println!("  itsanas pledge 10G        space offered to others, which is what earns yours");
    println!("  itsanas daemon            serve peers and sync on a timer");
    println!();
    println!(
        "To join a coordinator that already exists, before the daemon:\n  \
         itsanas coordinator <host:port>\n  itsanas register"
    );

    Ok(())
}

/// Re-seal this machine's keystore under a new passphrase, and optionally the
/// recovery container too.
pub(crate) fn change_passphrase(home: &Path, recovery: bool) -> Result<()> {
    if !Node::exists(home) {
        return Err(CliError::NoNode(home.to_path_buf()));
    }

    println!("Current passphrase for this machine's keystore.");
    let current = passphrase(false)?;

    let new = if let Ok(value) = std::env::var(NEW_PASSPHRASE_ENV) {
        value
    } else if std::io::stdin().is_terminal() {
        let entered =
            rpassword::prompt_password("New passphrase: ").map_err(|error| CliError::Io {
                path: PathBuf::from("<terminal>"),
                source: error,
            })?;
        let again = rpassword::prompt_password("Confirm new passphrase: ").map_err(|error| {
            CliError::Io {
                path: PathBuf::from("<terminal>"),
                source: error,
            }
        })?;
        if again != entered {
            return Err(CliError::Usage("the passphrases did not match".to_owned()));
        }
        entered
    } else {
        return Err(CliError::Usage(format!(
            "no terminal to prompt on. Set {NEW_PASSPHRASE_ENV} for non-interactive use."
        )));
    };
    // Before the recovery container: it would go under a passphrase the
    // keystore then refuses, which is the split the comment below is about.
    node::check_new_passphrase(&new)?;

    // The recovery container first, the keystore second. The container is the
    // step that fails -- a device not enrolled, a coordinator not answering --
    // and the first version did it second, so either failure left the keystore
    // under the new passphrase and the container under the old one, from a
    // command whose comment promised nothing would change. The keystore is a
    // local write-then-rename; if it fails after the container went through,
    // the split is the recoverable one and the message says exactly what it is.
    if recovery {
        let node = match Node::open(home, &current) {
            Err(CliError::Store(itsanas_store::StoreError::Locked(_))) => {
                return Err(CliError::Usage(
                    "the daemon holds this node, and re-sealing the recovery container opens it; \
                     stop the daemon, run this again, then start it"
                        .to_owned(),
                ));
            }
            Err(other) => return Err(other),
            Ok(node) if node.config.coordinator.is_none() => {
                return Err(CliError::Usage(
                    "no coordinator is configured, so there is no recovery container to re-seal"
                        .to_owned(),
                ));
            }
            Ok(node) => node,
        };
        coordinator::set_escrow(&node, Some(&new), &node.secrets)?;
        println!("recovery container re-sealed under the new passphrase.");
    }

    // Sealed, the system asks now, and the seal the daemon reads follows.
    if let Err(error) = setup::unlock::changing(home, &new, setup::unlock::platform_gate, || {
        Node::change_passphrase(home, &current, &new)
    }) {
        if recovery {
            eprintln!(
                "the recovery container is already under the NEW passphrase, and this \
                 machine's keystore is still under the old one. Run this again."
            );
        }
        return Err(error);
    }
    println!("passphrase changed for this machine's keystore.");
    if setup::unlock::is_on(home) {
        println!("the copy sealed for the daemon is sealed again under it.");
    }

    println!();
    println!("Still under the old passphrase, and not changed by this:");
    println!("  - whatever starts the daemon without a terminal: on Windows");
    println!("    %LOCALAPPDATA%\\itsanas\\passphrase.txt, on Linux ITSANAS_PASSPHRASE in");
    println!("    ~/.config/itsanas/environment. Update it now, or the daemon will not");
    println!("    start next time. A daemon already running keeps its unlocked keys.");
    if !recovery {
        println!("  - a recovery container lodged with a coordinator, if there is one.");
        println!("    `itsanas passphrase --recovery` does both; to re-seal it alone now,");
        println!("    `itsanas register --recovery`.");
    }
    Ok(())
}

pub(crate) fn login(
    home: &Path,
    username: &str,
    phrase_file: Option<&std::path::Path>,
    from: Option<&str>,
    device: Option<&str>,
) -> Result<()> {
    if Node::exists(home) {
        return Err(CliError::NodeExists(home.to_path_buf()));
    }

    if let Some(address) = from {
        return login_from_coordinator(home, username, address, device);
    }

    let phrase = if let Some(path) = phrase_file {
        std::fs::read_to_string(path).map_err(|error| CliError::Io {
            path: path.to_owned(),
            source: error,
        })?
    } else {
        if !std::io::stdin().is_terminal() {
            return Err(CliError::Usage(
                "no terminal to prompt on; pass --phrase-file".to_owned(),
            ));
        }
        // The phrase is as sensitive as a password, so it is read without echo
        // for the same reason.
        rpassword::prompt_password("Recovery phrase (24 words): ").map_err(|error| {
            CliError::Io {
                path: PathBuf::from("<terminal>"),
                source: error,
            }
        })?
    };

    println!("Choose a passphrase for this machine's keystore.");
    let mut node = Node::restore(home, &passphrase(true)?, username, &phrase_words(&phrase))?;
    settle_listen_port(&mut node)?;

    println!("Account restored.");
    println!("  user id : {}", node.store.owner());
    println!(
        "  device  : {} (new for this machine)",
        node.store.device_id()
    );
    println!();
    println!("{}", recovered_note(false));
    println!();
    println!("Nothing has been downloaded yet. Run `itsanas sync <peer>` to pull");
    println!("your data from a peer or a host that is holding it.");

    Ok(())
}

/// Restore an account from a coordinator, using a passphrase alone.
///
/// The machine has nothing: no device key, no account, no store. It fetches the
/// sealed container by name, opens it with the passphrase, and writes a local
/// keystore from what was inside — with a **new device key**, because the
/// container carries the account's identity and this is a different machine.
pub(crate) fn login_from_coordinator(
    home: &Path,
    username: &str,
    address: &str,
    device: Option<&str>,
) -> Result<()> {
    // Parsed before the passphrase is asked: a mistyped id fails first.
    device.map(coordinator::parse_device).transpose()?;

    println!("Recovering {username:?} from {address}.");
    println!("This needs the passphrase the container was sealed with, which is");
    println!("the passphrase of whichever machine lodged it — not necessarily one");
    println!("you have used on this machine.");
    let secret = zeroize::Zeroizing::new(passphrase(false)?);
    let expect = device.map(coordinator::parse_device).transpose()?;
    let code = crate::email::recovery_code(address, expect, username)?;
    let node = recover_from_coordinator(home, username, address, device, &secret, &code)?;

    println!();
    println!("Account restored.");
    println!("  user id     : {}", node.store.owner());
    println!(
        "  device      : {} (new for this machine)",
        node.store.device_id()
    );
    println!("  coordinator : {address} (kept for this machine)");
    println!();
    println!("{}", recovered_note(true));
    println!();
    println!("Nothing has been downloaded yet. Next, on this machine:");
    println!("  itsanas register           enrol it, so your other machines find it");
    println!("  itsanas pledge <size>      a machine that pledges nothing relays nothing");
    println!("  itsanas folder <directory>");
    println!("  itsanas daemon");

    Ok(())
}

/// What a recovery says about the 24 words. After the words were typed,
/// that they are still the backup; after `--from`, where the person may not
/// have them at hand, that this is the same account, whose words have not
/// changed and are shown by `itsanas words` on any of its machines. "Your
/// 24-word phrase is unchanged" said after `--from` read as if they ought to
/// be on the desk (hand test, 2026-10-22).
pub(crate) fn recovered_note(from_coordinator: bool) -> &'static str {
    if from_coordinator {
        "This is the same account as before, not a new one: its 24 recovery words\n\
         are the same as before. `itsanas words` shows them, on any machine of the account."
    } else {
        "Your 24-word phrase is unchanged and still the ultimate backup:\n\
         this recovered the same identity, it did not create a new one."
    }
}

/// Fetch the account's recovery container from `address`, open it with
/// `secret` and write this machine's node from it: `login --from` without
/// its messages, so `setup` runs the same restore.
pub(crate) fn recover_from_coordinator(
    home: &Path,
    username: &str,
    address: &str,
    device: Option<&str>,
    secret: &str,
    code: &str,
) -> Result<Node> {
    let expect = device.map(coordinator::parse_device).transpose()?;
    let secrets = coordinator::fetch_escrow_with_code(address, expect, username, secret, code)?;
    let mut node = Node::restore_from_secrets(home, secret, username, &secrets)?;

    // The coordinator that just proved it holds this account is the one to
    // keep. This used to be forgotten: the message below told the reader to
    // run `itsanas register`, which then failed for want of a coordinator, and
    // `sync` found only machines on the same network -- so a recovery on a
    // network away from the others restored an identity and nothing else.
    node.config.coordinator = Some(address.to_owned());
    node.config.coordinator_device = device.map(str::to_owned);
    node.save_config()?;
    settle_listen_port(&mut node)?;
    Ok(node)
}

pub(crate) fn check_passphrase(home: &Path) -> Result<()> {
    if !Node::exists(home) {
        return Err(CliError::NoNode(home.to_path_buf()));
    }
    let identity = itsanas_node::node::Identity::open(home, &passphrase(false)?)?;
    println!("passphrase accepted for {}", identity.config.username);
    Ok(())
}

pub(crate) fn whoami(home: &Path) -> Result<()> {
    let node = open(home)?;
    println!("{}", node.store.owner());
    Ok(())
}

/// The phrase as `init` shows it: numbered, four words a line.
pub(crate) fn phrase_grid(phrase: &str) -> String {
    let mut out = String::new();
    for (index, word) in phrase.split_whitespace().enumerate() {
        let _ = write!(out, "{:>2}. {:<12}", index + 1, word);
        if index % 4 == 3 {
            out.push('\n');
        }
    }
    out
}

/// `itsanas words`: the recovery phrase, from the keystore and its passphrase.
pub(crate) fn words(home: &Path) -> Result<()> {
    let phrase = itsanas_node::node::Identity::recovery_phrase(home, &passphrase(false)?)?;
    println!("The 24 recovery words of this account. Whoever reads them owns it:");
    println!("write them on paper, keep the paper away from this machine, and");
    println!("clear this window afterwards.");
    println!();
    print!("{}", words_text(phrase.as_str()));
    Ok(())
}

/// What `itsanas words` shows: the numbered grid to copy onto paper, then the
/// same words on one line, to paste into `login` or a phrase file -- the grid
/// pasted as it stands carries its numbers and its line breaks.
pub(crate) fn words_text(phrase: &str) -> String {
    let words: Vec<&str> = phrase.split_whitespace().collect();
    format!(
        "{}\nOn one line, to copy and paste:\n{}\n",
        phrase_grid(phrase),
        words.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::phrase_grid;
    use crate::node::phrase_words;

    /// The grid `init` prints, pasted into `--phrase-file` as it stands, must
    /// read back as the 24 words -- a person copies what the screen shows.
    #[test]
    fn the_phrase_as_init_prints_it_reads_back_as_the_words() {
        let words: Vec<String> = (1..=24).map(|i| format!("word{i}")).collect();
        let phrase = words.join(" ");
        assert_eq!(phrase_words(&phrase_grid(&phrase)), phrase);
        assert_eq!(phrase_words(&words.join("\n")), phrase, "one word a line");
    }

    /// `itsanas words` shows the grid for paper and the same words on one
    /// line for a paste. Sabotage: print the grid alone -- no line of the
    /// output is then the phrase.
    #[test]
    fn itsanas_words_prints_the_grid_and_then_the_words_on_one_line() {
        let words: Vec<String> = (1..=24).map(|i| format!("word{i}")).collect();
        let phrase = words.join(" ");
        let text = super::words_text(&phrase);
        let grid = phrase_grid(&phrase);
        assert!(text.starts_with(&grid), "the grid first: {text}");
        let last = text.lines().last().unwrap_or_default();
        assert_eq!(
            last, phrase,
            "the last line is the 24 words, in order: {text}"
        );
    }

    /// After `--from` the person may not have the words at hand: the message
    /// says it is the same account and where to see them, and keeps the
    /// "ultimate backup" sentence for the path where they were just typed.
    /// Sabotage: return the words version for both.
    #[test]
    fn a_recovery_from_a_coordinator_says_where_the_words_are_and_the_words_path_keeps_its_note() {
        let from = super::recovered_note(true);
        assert!(from.contains("same account"), "{from}");
        assert!(from.contains("24 recovery words"), "{from}");
        assert!(from.contains("same as before"), "{from}");
        assert!(from.contains("`itsanas words`"), "{from}");
        assert!(from.contains("any machine of the account"), "{from}");
        assert!(!from.contains("ultimate backup"), "{from}");
        let typed = super::recovered_note(false);
        assert!(typed.contains("24-word phrase is unchanged"), "{typed}");
        assert!(typed.contains("ultimate backup"), "{typed}");
        assert!(typed.contains("did not create a new one"), "{typed}");
    }
}
