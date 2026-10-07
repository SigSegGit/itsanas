//! What setup is told: from a file (`--answers`), or asked in a terminal
//! (`--text`). Never a secret -- those come from [`super::secrets`].
//!
//! # The file
//!
//! A small subset of TOML, so a provisioning script or a person writes it the
//! way they would any configuration -- `key = "text"`, `key = true`,
//! `key = 120`, `#` comments -- and this crate takes no TOML dependency for
//! a dozen keys. An unknown key is refused, as the node's own configuration
//! refuses one: a typo silently ignored is how a machine ends up offering
//! nothing while its owner believes they offered a terabyte.
//!
//! ```toml
//! instance = "tester"            # optional: a second node on this machine
//! folder = "~/ITSaNAS-tester"    # default ~/ITSaNAS, or ~/ITSaNAS-NAME
//! account = "new"                # or "join"
//! username = "camille"
//! # from = "coord.example:9800"  # join from a coordinator's recovery container
//! coordinator = "coord.example:9800"
//! coordinator_device = "ab12..." # its device id: `itsanas-coordinator --identity`
//! invite = "CODE"                # the first registration on an invite-only coordinator
//! pledge = "10G"                 # space offered to others
//! announce = "home.example:9801" # only where a forward or IPv6 reaches this machine
//! service = true                 # install and start the background service
//! tray = true                    # and the tray icon at logon
//! updates = "notify"             # or "auto" (install by itself) or "off"
//! verify_seconds = 120           # how long the final check waits
//! ```
//!
//! The passphrase comes from `ITSANAS_PASSPHRASE` and the 24 words of an
//! account being joined from `--phrase-file`, exactly where `init` and
//! `login` already take them.

use std::{
    io::{BufRead as _, Write as _},
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{
    config::{format_size, parse_size},
    error::{CliError, Result},
    node::Node,
};

/// New account, or one that exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Account {
    New {
        username: String,
    },
    /// `from`: a coordinator holding the account's recovery container;
    /// without it, the 24 words.
    Join {
        username: String,
        from: Option<String>,
    },
}

impl Account {
    pub(crate) fn username(&self) -> &str {
        match self {
            Self::New { username } | Self::Join { username, .. } => username,
        }
    }
}

/// Every non-secret answer. `None` means "keep what this machine has, or do
/// without": re-running setup with fewer answers never undoes a setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Answers {
    pub(crate) instance: Option<String>,
    pub(crate) folder: Option<PathBuf>,
    pub(crate) account: Option<Account>,
    pub(crate) coordinator: Option<String>,
    pub(crate) coordinator_device: Option<String>,
    pub(crate) invite: Option<String>,
    pub(crate) pledge: Option<u64>,
    pub(crate) announce: Option<String>,
    pub(crate) service: bool,
    pub(crate) tray: bool,
    /// What the daemon does about a newer release; `None` keeps the node's.
    pub(crate) updates: Option<crate::config::Updates>,
    pub(crate) verify_for: Duration,
}

/// How long the final check waits for a daemon that just started, unless told.
pub(crate) const DEFAULT_VERIFY: Duration = Duration::from_secs(120);

impl Default for Answers {
    fn default() -> Self {
        Self {
            instance: None,
            folder: None,
            account: None,
            coordinator: None,
            coordinator_device: None,
            invite: None,
            pledge: None,
            announce: None,
            service: true,
            tray: true,
            updates: None,
            verify_for: DEFAULT_VERIFY,
        }
    }
}

/// The folder kept in step when nobody says which: one per node, so two
/// accounts on one machine never share one.
pub(crate) fn default_folder(base: &Path, instance: Option<&str>) -> PathBuf {
    base.join(instance.map_or_else(|| "ITSaNAS".to_owned(), |name| format!("ITSaNAS-{name}")))
}

/// `~` and `~/...`, which every example in the documentation writes.
pub(super) fn expand(text: &str, base: &Path) -> PathBuf {
    match text.strip_prefix('~') {
        Some("") => base.to_owned(),
        Some(rest) if rest.starts_with(['/', '\\']) => base.join(&rest[1..]),
        _ => PathBuf::from(text),
    }
}

#[derive(Debug)]
enum Value {
    Text(String),
    Flag(bool),
    Number(u64),
}

fn parse_value(raw: &str, line: usize) -> Result<Value> {
    let bad = |why: &str| CliError::Usage(format!("answers, line {line}: {why}"));
    let raw = raw.trim();
    if let Some(inner) = raw.strip_prefix('\'') {
        // A literal string: nothing escaped, which is what a Windows path wants.
        let text = inner
            .strip_suffix('\'')
            .ok_or_else(|| bad("a 'string' is not closed"))?;
        return Ok(Value::Text(text.to_owned()));
    }
    if let Some(inner) = raw.strip_prefix('"') {
        let mut text = String::new();
        let mut chars = inner.chars();
        loop {
            match chars.next() {
                None => return Err(bad("a \"string\" is not closed")),
                Some('"') => break,
                Some('\\') => match chars.next() {
                    Some('\\') => text.push('\\'),
                    Some('"') => text.push('"'),
                    Some('n') => text.push('\n'),
                    Some('t') => text.push('\t'),
                    _ => {
                        return Err(bad(
                            "unknown escape; use 'single quotes' for a Windows path",
                        ));
                    }
                },
                Some(other) => text.push(other),
            }
        }
        let rest = chars.as_str().trim();
        if !rest.is_empty() && !rest.starts_with('#') {
            return Err(bad("something follows the closing quote"));
        }
        return Ok(Value::Text(text));
    }
    let bare = raw.split('#').next().unwrap_or("").trim();
    match bare {
        "true" => Ok(Value::Flag(true)),
        "false" => Ok(Value::Flag(false)),
        number => number
            .parse()
            .map(Value::Number)
            .map_err(|_| bad("expected \"text\", true, false or a number")),
    }
}

/// Parse an answers file. `base` is the user's home, for `~`.
pub(crate) fn parse(text: &str, base: &Path) -> Result<Answers> {
    let mut answers = Answers::default();
    let mut account_kind: Option<String> = None;
    let mut username = None;
    let mut from = None;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, raw) = line.split_once('=').ok_or_else(|| {
            CliError::Usage(format!("answers, line {number}: expected `key = value`"))
        })?;
        let key = key.trim();
        let value = parse_value(raw, number)?;
        let wrong =
            |what: &str| CliError::Usage(format!("answers, line {number}: `{key}` takes {what}"));
        match (key, value) {
            ("instance", Value::Text(v)) => answers.instance = Some(v),
            ("folder", Value::Text(v)) => answers.folder = Some(expand(&v, base)),
            ("account", Value::Text(v)) => account_kind = Some(v),
            ("username", Value::Text(v)) => username = Some(v),
            ("from", Value::Text(v)) => from = Some(v),
            ("coordinator", Value::Text(v)) => answers.coordinator = Some(v),
            ("coordinator_device", Value::Text(v)) => answers.coordinator_device = Some(v),
            ("invite", Value::Text(v)) => answers.invite = Some(v),
            ("pledge", Value::Text(v)) => answers.pledge = Some(parse_size(&v)?),
            ("announce", Value::Text(v)) => answers.announce = Some(v),
            ("updates", Value::Text(v)) => {
                answers.updates = Some(crate::config::Updates::parse(&v).ok_or_else(|| {
                    CliError::Usage(format!(
                        "answers, line {number}: updates is \"auto\", \"notify\" or \"off\""
                    ))
                })?);
            }
            ("service", Value::Flag(v)) => answers.service = v,
            ("tray", Value::Flag(v)) => answers.tray = v,
            ("verify_seconds", Value::Number(v)) => answers.verify_for = Duration::from_secs(v),
            ("service" | "tray", _) => return Err(wrong("true or false")),
            ("verify_seconds", _) => return Err(wrong("a number of seconds")),
            (
                "instance" | "folder" | "account" | "username" | "from" | "coordinator"
                | "coordinator_device" | "invite" | "pledge" | "announce" | "updates",
                _,
            ) => return Err(wrong("\"text\"")),
            (other, _) => {
                return Err(CliError::Usage(format!(
                    "answers, line {number}: unknown key `{other}`"
                )));
            }
        }
    }
    answers.account = match (account_kind.as_deref(), username) {
        (None, None) => None,
        (Some("new"), Some(username)) => {
            if from.is_some() {
                return Err(CliError::Usage(
                    "answers: `from` restores an account; it needs account = \"join\"".to_owned(),
                ));
            }
            Some(Account::New { username })
        }
        (Some("join"), Some(username)) => Some(Account::Join { username, from }),
        (Some("new" | "join"), None) => {
            return Err(CliError::Usage(
                "answers: an account needs its `username`".to_owned(),
            ));
        }
        (None, Some(_)) => {
            return Err(CliError::Usage(
                "answers: `username` needs account = \"new\" or \"join\"".to_owned(),
            ));
        }
        (Some(other), _) => {
            return Err(CliError::Usage(format!(
                "answers: account is \"new\" or \"join\", not {other:?}"
            )));
        }
    };
    Ok(answers)
}

// ---------------------------------------------------------------------------
// In a terminal
// ---------------------------------------------------------------------------

fn ask(question: &str, default: Option<&str>) -> Result<String> {
    match default {
        Some(default) if !default.is_empty() => print!("{question} [{default}]: "),
        _ => print!("{question}: "),
    }
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|error| CliError::Io {
            path: PathBuf::from("<terminal>"),
            source: error,
        })?;
    let line = line.trim();
    Ok(if line.is_empty() {
        default.unwrap_or("").to_owned()
    } else {
        line.to_owned()
    })
}

fn optional(text: String) -> Option<String> {
    (!text.is_empty()).then_some(text)
}

fn yes(text: &str) -> bool {
    matches!(
        text.to_ascii_lowercase().as_str(),
        "y" | "yes" | "o" | "oui"
    )
}

/// The account's questions, only when this machine has none.
fn ask_account() -> Result<Account> {
    let kind = loop {
        let kind = ask(
            "A new account, or join one you already have on another machine? (new/join)",
            Some("new"),
        )?;
        if matches!(kind.as_str(), "new" | "join") {
            break kind;
        }
        println!("  new or join, please.");
    };
    let username = loop {
        let name = ask("Your account name", None)?;
        if !name.is_empty() {
            break name;
        }
    };
    if kind == "new" {
        return Ok(Account::New { username });
    }
    let how = ask(
        "Restore it from your 24 words, or from a coordinator that holds a recovery container? \
         (words/coordinator)",
        Some("words"),
    )?;
    let from = if how.starts_with('c') {
        optional(ask("The coordinator, host:port", None)?)
    } else {
        None
    };
    Ok(Account::Join { username, from })
}

/// The questions for what this machine does not have yet, and only those:
/// asking again for a setting already made is the opposite of resilient.
pub(crate) fn ask_in_terminal(home: &Path, instance: Option<&str>) -> Result<Answers> {
    let mut answers = Answers {
        instance: instance.map(str::to_owned),
        ..Answers::default()
    };
    let config = crate::config::Config::load(&Node::config_path(home)).ok();
    let exists = Node::exists(home);
    println!("Setting up ITSaNAS on this machine. Press Enter to take what is in [brackets].");
    println!();
    if !exists {
        answers.account = Some(ask_account()?);
    }
    let base = crate::config::user_home();
    if config.as_ref().and_then(|c| c.folder.as_ref()).is_none() {
        let default = default_folder(&base, instance);
        let folder = ask(
            "The folder to keep in step with your account",
            Some(&default.display().to_string()),
        )?;
        answers.folder = Some(expand(&folder, &base));
    }
    let joined_from = matches!(&answers.account, Some(Account::Join { from: Some(_), .. }));
    if config
        .as_ref()
        .and_then(|c| c.coordinator.as_ref())
        .is_none()
        && !joined_from
    {
        answers.coordinator = optional(ask(
            "A coordinator, host:port, to reach your machines on other networks (Enter for none)",
            None,
        )?);
    }
    if answers.coordinator.is_some() {
        answers.coordinator_device = optional(ask(
            "Its device id, from `itsanas-coordinator --identity` (Enter to trust the address)",
            None,
        )?);
        answers.invite = optional(ask("An invitation code, if you were sent one", None)?);
        answers.announce = optional(ask(
            "An address others can dial to reach this machine, host:port (Enter if it moves, \
             or has none)",
            None,
        )?);
    }
    if config.as_ref().is_none_or(|c| c.pledge_bytes == 0) {
        answers.pledge = ask_pledge(home)?;
    }
    answers.service = yes(&ask(
        "Run it in the background, with a tray icon, from now on? (y/n)",
        Some("y"),
    )?);
    answers.tray = answers.service;
    Ok(answers)
}

fn ask_pledge(home: &Path) -> Result<Option<u64>> {
    let free = fs4::available_space(home.parent().unwrap_or(home)).ok();
    if let Some(free) = free {
        println!("  This disk has {} free.", format_size(free));
    }
    loop {
        let text = ask(
            "Space to offer the other members, e.g. 10G (it is what earns room for your own \
             files; Enter for none yet)",
            None,
        )?;
        if text.is_empty() {
            return Ok(None);
        }
        match parse_size(&text) {
            Ok(bytes) => return Ok(Some(bytes)),
            Err(error) => println!("  {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answers_file_says_everything_but_the_secrets() {
        let base = Path::new("/home/camille");
        let answers = parse(
            "# a tester's machine\n\
             instance = \"tester\"\n\
             folder = \"~/ITSaNAS-tester\"\n\
             account = \"new\"\n\
             username = \"camille\"\n\
             coordinator = \"coord.example:9800\"\n\
             pledge = \"10G\"   # offered\n\
             service = false\n\
             verify_seconds = 30\n",
            base,
        )
        .expect("parse");
        assert_eq!(answers.instance.as_deref(), Some("tester"));
        assert_eq!(
            answers.folder,
            Some(base.join("ITSaNAS-tester")),
            "`~` was not the home: the folder would be made in the working directory"
        );
        assert_eq!(
            answers.account,
            Some(Account::New {
                username: "camille".to_owned()
            })
        );
        assert_eq!(answers.pledge, Some(10 * 1024 * 1024 * 1024));
        assert!(
            !answers.service,
            "service = false installed a service anyway"
        );
        assert_eq!(answers.verify_for, Duration::from_secs(30));
    }

    #[test]
    fn a_mistyped_answer_is_refused_not_ignored() {
        let base = Path::new("/h");
        for (text, why) in [
            ("pledg = \"10G\"\n", "an unknown key"),
            ("service = \"no\"\n", "a flag given as text"),
            ("account = \"new\"\n", "an account with no username"),
            ("account = \"maybe\"\nusername = \"x\"\n", "an account kind"),
            ("folder = \"C:\\Users\"\n", "an escape TOML does not have"),
        ] {
            assert!(
                parse(text, base).is_err(),
                "{why} was accepted: a machine would be set up differently from what its \
                 owner wrote, and nothing would say so"
            );
        }
        assert_eq!(
            parse("folder = 'C:\\Users\\c\\ITSaNAS'\n", base)
                .expect("literal")
                .folder,
            Some(PathBuf::from("C:\\Users\\c\\ITSaNAS")),
            "a Windows path in single quotes was changed"
        );
    }
}
