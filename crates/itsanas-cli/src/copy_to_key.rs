//! `itsanas copy-to-key DIR`: this account on a USB key, ready to start on
//! any computer it is plugged into.
//!
//! A local copy, with no coordinator in it (Nicolas, 2026-10-22): the key is
//! a portable copy (`portable.rs`, every system's program and a client-only
//! home) whose home already holds the account, sealed under a passphrase of
//! its own, with a device of its own. In this order, and nothing written on
//! the key before the passphrases and the room are checked:
//!
//! 1. the target: a full path, not inside this account's home, vault or
//!    synced folder, and not a key that already holds an account -- never
//!    overwritten;
//! 2. this machine's passphrase, which reads the secrets from the keystore
//!    (the store stays the daemon's);
//! 3. a new passphrase for the key, at least [`MIN_LENGTH`] characters and
//!    neither this machine's nor a trivially weak one ([`weak_passphrase`]);
//! 4. room on the drive for the programs and the account's files ([`room`]);
//! 5. the portable copy, then the account in its home: the master only, a
//!    fresh device, the coordinator, the synced folder `ITSaNAS/Fichiers`;
//! 6. enrolment now if the coordinator answers, or at the key's first start
//!    ([`enrol_at_start`]), said either way.
//!
//! Each step is written to `<home>/copy-to-key.status` ([`STATUS`]), so a run
//! in the background (`--detach`) can be followed by the Settings page.
//!
//! # What the key is, said every time
//!
//! Its keystore holds the account's master secret. Whoever has the key and
//! its passphrase **is** the account; removing the key from the device list
//! stops the other machines dialling it, not its holder reading and writing.
//! A device given device and data keys without the master is the real fix
//! (ROADMAP).

use std::{
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use itsanas_node::node::Identity;
use zeroize::Zeroizing;

use crate::{
    config::Config,
    error::{CliError, Result},
    node::{Node, SNAPSHOT},
    portable,
    setup::{Secret, SecretPrompt, secrets, sign},
};

/// The progress file, in the home of the account being copied.
pub(crate) const STATUS: &str = "copy-to-key.status";

/// The shortest passphrase a key gets. The key's keystore is the account,
/// and a lost key is an offline attack on it that Argon2 slows and does not
/// stop.
pub(crate) const MIN_LENGTH: usize = 12;

/// The synced folder on the key, beside its home.
pub(crate) const FOLDER: &str = "Fichiers";

/// What the five programs take when the release cannot say (0.1.11: about
/// 40 MiB together).
const PROGRAMS_GUESS: u64 = 45 * 1024 * 1024;

/// The flag the background copy is started with: its two passphrases come
/// on standard input, never in its argv or environment.
pub(crate) const ON_STDIN: &str = "--passphrases-on-stdin";

/// What the window asks before the key's passphrase.
pub(crate) const KEY_PURPOSE: &str = "A NEW passphrase for the USB key, at least 12 characters. \
     Whoever opens this key with it IS your account: they can read and change all your files, \
     and removing the key from your devices later does not lock them out. Choose a long one \
     that you use nowhere else, and not this machine's.";

/// What the window asks the second time.
const KEY_AGAIN: &str = "The same passphrase for the USB key again:";

/// The two passphrases of a copy, wiped when dropped.
pub(crate) struct Passphrases {
    /// This machine's: it opens the keystore the secrets are read from.
    pub(crate) machine: Secret,
    /// The key's: its keystore is sealed under it.
    pub(crate) key: Secret,
}

impl std::fmt::Debug for Passphrases {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Passphrases(redacted)")
    }
}

// ---------------------------------------------------------------------------
// The rules, as pure functions
// ---------------------------------------------------------------------------

/// Why `key` will not do as the key's passphrase; `None` when it will.
///
/// Too short; this machine's own (one guess opens both); the same few
/// characters repeated (`aaaaaaaaaaaa`, `abcabcabcabc`); a run of keys or
/// digits (`123456789012`, `abcdefghijkl`); or the username with too little
/// around it -- the username counts for nothing, it is on every claim.
#[must_use]
pub(crate) fn weak_passphrase(key: &str, machine: &str, username: &str) -> Option<String> {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() < MIN_LENGTH {
        return Some(format!(
            "is {} characters long; a key's needs at least {MIN_LENGTH}",
            chars.len()
        ));
    }
    if key == machine {
        return Some(
            "is this machine's passphrase: one guess would open both. Choose another".to_owned(),
        );
    }
    let repeats = (1..=chars.len() / 2).any(|period| {
        chars.len().is_multiple_of(period)
            && chars
                .iter()
                .enumerate()
                .all(|(at, c)| *c == chars[at % period])
    });
    if repeats {
        return Some("is the same few characters repeated".to_owned());
    }
    if is_run(&chars) {
        return Some("is a run of keys or digits, the first thing anybody tries".to_owned());
    }
    let user = username.trim().to_lowercase();
    if user.chars().count() >= 3 {
        let rest = key.to_lowercase().replace(&user, "");
        if rest.chars().count() < MIN_LENGTH {
            return Some(format!(
                "is mostly your username, which counts for nothing: it needs {MIN_LENGTH} \
                 characters besides it"
            ));
        }
    }
    None
}

/// Every character one up (or one down) from the one before: digits wrap
/// round, so `123456789012` is one.
fn is_run(chars: &[char]) -> bool {
    let step = |a: char, b: char| -> i64 {
        match (a.to_digit(10), b.to_digit(10)) {
            (Some(a), Some(b)) => match (b + 10 - a) % 10 {
                1 => 1,
                9 => -1,
                _ => 0,
            },
            _ => i64::from(u32::from(b)) - i64::from(u32::from(a)),
        }
    };
    let steps: Vec<i64> = chars
        .windows(2)
        .map(|pair| step(pair[0], pair[1]))
        .collect();
    steps.iter().all(|s| *s == 1) || steps.iter().all(|s| *s == -1)
}

/// Whether the key has room: `Ok` with what was counted, `Err` with why
/// not. `programs` is the release's size when it could be read, `content`
/// the account's files when a status said it, `free` the drive's.
///
/// # Errors
///
/// When the drive has less free than the copy needs.
pub(crate) fn room(
    programs: Option<u64>,
    content: Option<u64>,
    free: Option<u64>,
) -> std::result::Result<String, String> {
    use crate::config::format_size;
    let programs = programs.unwrap_or(PROGRAMS_GUESS);
    let needed = programs.saturating_add(content.unwrap_or(0));
    let counted = match content {
        Some(content) => format!(
            "{} for the programs and {} for your files",
            format_size(programs),
            format_size(content)
        ),
        None => format!(
            "{} for the programs; the size of your files is not known yet (no status from \
             this machine's service), so they were not counted",
            format_size(programs)
        ),
    };
    match free {
        None => Ok(format!(
            "how much room the drive has could not be read; it needs about {}: {counted}",
            format_size(needed)
        )),
        Some(free) if free < needed => Err(format!(
            "the drive has {} free and the copy needs about {}: {counted}. Free some room on \
             it, or choose a bigger key. Nothing was written",
            format_size(free),
            format_size(needed)
        )),
        Some(free) => Ok(format!(
            "room: about {} needed ({counted}), {} free",
            format_size(needed),
            format_size(free)
        )),
    }
}

/// The account's content on this machine, from the status the daemon last
/// wrote (its "your content" line), rounded up: the store is the daemon's
/// while it runs.
#[must_use]
pub(crate) fn content_in(snapshot: &str) -> Option<u64> {
    let line = snapshot
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("your content"))?;
    let (number, unit) = line.trim().split_once(' ')?;
    let unit: u64 = match unit.trim() {
        "B" => return number.parse().ok(),
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        "TiB" => 1 << 40,
        _ => return None,
    };
    // `format_size` floors to a tenth: one tenth more never under-counts.
    let (whole, tenths) = number.split_once('.').unwrap_or((number, "0"));
    let whole: u64 = whole.parse().ok()?;
    let tenths: u64 = tenths.parse().ok()?;
    Some(
        whole
            .saturating_mul(unit)
            .saturating_add(tenths.saturating_add(1).saturating_mul(unit) / 10),
    )
}

/// Where the copy goes, `<dir>/ITSaNAS`, or why not there.
///
/// # Errors
///
/// A path `jobs::check_path` refuses, a drive that is not there, a place
/// inside this account's home, vault or synced folder (or holding one), or a
/// key that already holds an account.
pub(crate) fn check_target(home: &Path, config: &Config, dir: &Path) -> Result<PathBuf> {
    let dir = crate::jobs::check_path(dir).map_err(CliError::Usage)?;
    if crate::drives::existing_ancestor(&dir).is_none() {
        return Err(CliError::Usage(format!(
            "{} does not exist: no drive or folder there (is the key plugged in?)",
            dir.display()
        )));
    }
    let root = dir.join(portable::DIR);
    let vault = Node::vault_path(home, config);
    let mut theirs = vec![
        (home.to_path_buf(), "this account's home"),
        (vault, "its vault"),
    ];
    if let Some(folder) = &config.folder {
        theirs.push((folder.clone(), "its synced folder"));
    }
    for (outer, what) in theirs {
        if root.starts_with(&outer) || outer.starts_with(&root) {
            return Err(CliError::Usage(format!(
                "{} is inside {what} ({}), or holds it: the key would copy the account into \
                 itself. Choose the key's own drive",
                dir.display(),
                outer.display()
            )));
        }
    }
    if Node::exists(&root.join("home")) {
        return Err(CliError::Usage(format!(
            "this key already holds an account ({}); choose another folder. Nothing was changed",
            root.join("home").display()
        )));
    }
    Ok(root)
}

// ---------------------------------------------------------------------------
// Progress, for a page that follows a run in the background
// ---------------------------------------------------------------------------

/// A JSON string literal.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `<home>/copy-to-key.status`: `{"step", "done", "said", "error"}`, replaced
/// whole through a temporary name at each step. Best effort: a status that
/// cannot be written never stops the copy.
struct Status<'a>(&'a Path);

impl Status<'_> {
    fn write(&self, step: &str, done: bool, said: &str, error: Option<&str>) {
        let text = format!(
            "{{\"step\":{},\"done\":{done},\"said\":{},\"error\":{}}}\n",
            json_string(step),
            json_string(said),
            error.map_or_else(|| "null".to_owned(), json_string)
        );
        let staging = self.0.join(format!("{STATUS}.tmp"));
        if std::fs::write(&staging, text).is_ok() {
            let _ = std::fs::rename(&staging, self.0.join(STATUS));
        }
    }

    fn step(&self, step: &str, said: &str) {
        self.write(step, false, said, None);
    }

    fn ended(&self, result: &Result<String>) {
        match result {
            Ok(said) => self.write("done", true, said, None),
            Err(error) => self.write("failed", true, "", Some(&error.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// The copy
// ---------------------------------------------------------------------------

/// Copy the account at `home` to `<dir>/ITSaNAS`, the passphrases already
/// asked; what to tell the person. Every check is made again here, before
/// anything is written: a background run trusts nothing its parent checked.
///
/// `programs` is the release's size when known, `fetch` puts the programs
/// in `bin/` ([`portable::make_with`]), `register` enrols the key's node.
///
/// # Errors
///
/// Any refusal of the module's list, or the disk. A failed enrolment is not
/// one: the key enrols itself at its first start.
pub(crate) fn copy(
    home: &Path,
    dir: &Path,
    passes: &Passphrases,
    programs: Option<u64>,
    fetch: impl FnOnce(&Path) -> Result<Vec<String>>,
    register: impl FnOnce(&Node) -> Result<()>,
) -> Result<String> {
    let result = copy_steps(home, dir, passes, programs, fetch, register);
    Status(home).ended(&result);
    result
}

/// [`copy`], before its end is recorded.
fn copy_steps(
    home: &Path,
    dir: &Path,
    passes: &Passphrases,
    programs: Option<u64>,
    fetch: impl FnOnce(&Path) -> Result<Vec<String>>,
    register: impl FnOnce(&Node) -> Result<()>,
) -> Result<String> {
    let status = Status(home);
    status.step("checking", "checking the key");
    if !Node::exists(home) {
        return Err(CliError::NoNode(home.to_path_buf()));
    }
    let config = Config::load(&Node::config_path(home))?;
    let root = check_target(home, &config, dir)?;

    status.step("passphrase", "opening this machine's keys");
    let secrets = Identity::secrets(home, &passes.machine)?;
    if let Some(why) = weak_passphrase(&passes.key, &passes.machine, &config.username) {
        return Err(CliError::Usage(format!(
            "the key's passphrase {why}. Nothing was written"
        )));
    }

    status.step("space", "checking the room on the key");
    let content = std::fs::read_to_string(home.join(SNAPSHOT))
        .ok()
        .and_then(|text| content_in(&text));
    let counted =
        room(programs, content, crate::drives::free_bytes(dir)).map_err(CliError::Usage)?;

    status.step(
        "programs",
        "writing the programs for every system on the key",
    );
    let made = portable::make_with(dir, fetch)?;

    status.step(
        "account",
        "sealing the account on the key under its own passphrase",
    );
    let key_home = root.join("home");
    let mut node =
        Node::restore_from_secrets(&key_home, &passes.key, &config.username, secrets.as_bytes())?;
    drop(secrets);
    node.config.coordinator.clone_from(&config.coordinator);
    node.config
        .coordinator_device
        .clone_from(&config.coordinator_device);
    node.save_config()?;
    let folder = root.join(FOLDER);
    std::fs::create_dir_all(&folder).map_err(|source| CliError::Io {
        path: folder.clone(),
        source,
    })?;
    crate::set_folder(&mut node, &folder)?;
    crate::settle_listen_port(&mut node)?;

    status.step("enrolling", "enrolling the key with the coordinator");
    let enrolled = match &node.config.coordinator {
        None => "No coordinator is set, so it finds your other machines on the same network \
                 only"
            .to_owned(),
        Some(address) => match register(&node) {
            Ok(()) => format!("It is enrolled with {address} as a device of its own"),
            Err(error) => format!(
                "It is not enrolled yet ({error}): it enrols itself the first time it is \
                 started, with nothing to do"
            ),
        },
    };

    Ok(format!(
        "The key is ready, in {root}, with its own device {device}.\n\
         {enrolled}.\n\
         {made}.\n\
         {counted}.\n\
         To use it: on Windows double-click ITSaNAS.cmd; on a Mac double-click \
         itsanas.command; on Linux run `sh itsanas.sh`. It asks the key's passphrase, not this \
         machine's, and your files appear in {folder}.\n\
         Whoever has this key and its passphrase has your account: removing it from your \
         devices does not lock them out. Keep it like a house key.",
        root = root.display(),
        device = node.store.device_id(),
        folder = folder.display(),
    ))
}

/// At a daemon's start: a node with a coordinator and no `registered`
/// marker -- a key that could not enrol when it was made -- enrols once,
/// through `register`, and writes the marker. Failing, it carries on
/// unenrolled and tries again at the next start. What to print, `None` when
/// there was nothing to do.
pub(crate) fn enrol_at_start(
    node: &Node,
    register: impl FnOnce(&Node) -> Result<()>,
) -> Option<String> {
    let address = node.config.coordinator.as_deref()?;
    let marker = node.home.join(crate::REGISTERED);
    if marker.exists() {
        return None;
    }
    Some(match register(node) {
        Ok(()) => {
            let _ = std::fs::write(&marker, address);
            format!("  enrolled  with {address}, at this first start")
        }
        Err(error) => format!(
            "  enrolled  not yet ({error}); it is tried again at the next start, and the \
             machines on this network sync meanwhile"
        ),
    })
}

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

/// `itsanas copy-to-key DIR [--detach]`.
///
/// # Errors
///
/// What [`copy`] refuses, a passphrase that was not given, or a background
/// copy that could not be started.
pub(crate) fn command(home: &Path, dir: &Path, detach: bool, on_stdin: bool) -> Result<()> {
    let absolute = |path: &Path| {
        std::path::absolute(path).map_err(|source| CliError::Io {
            path: path.to_owned(),
            source,
        })
    };
    let home = absolute(home)?;
    let dir = absolute(dir)?;
    let result = if on_stdin {
        read_passphrases(std::io::stdin().lock()).and_then(|passes| run(&home, &dir, &passes))
    } else {
        ask(&home, &dir).and_then(|passes| {
            if detach {
                in_background(&home, &dir, &passes)
            } else {
                run(&home, &dir, &passes)
            }
        })
    };
    if result.is_err() {
        Status(&home).ended(&result);
    }
    println!("{}", result?);
    Ok(())
}

/// The copy with this build's release and coordinator.
fn run(home: &Path, dir: &Path, passes: &Passphrases) -> Result<String> {
    let curl = crate::update::Curl;
    let programs = crate::update::Updater::of_this_build(&curl)
        .and_then(|updater| updater.release_bytes())
        .ok();
    copy(
        home,
        dir,
        passes,
        programs,
        |bin| crate::update::Updater::of_this_build(&curl)?.release_binaries(bin),
        |node| crate::register_and_announce(node, None).map(|_| ()),
    )
}

/// The two passphrases, in windows of the system's own (or this terminal),
/// each checked as soon as it is typed: the target before any question, this
/// machine's against its keystore, the key's against the rules.
fn ask(home: &Path, dir: &Path) -> Result<Passphrases> {
    if !Node::exists(home) {
        return Err(CliError::NoNode(home.to_path_buf()));
    }
    let config = Config::load(&Node::config_path(home))?;
    check_target(home, &config, dir)?;
    let mut prompt = secrets::Native::new(secrets::choose()?);
    let machine = prompt.passphrase(&sign::account_purpose(home, "to copy it to a key"))?;
    Identity::secrets(home, &machine)?;
    println!("{KEY_PURPOSE}");
    let key = prompt.passphrase(KEY_PURPOSE)?;
    if let Some(why) = weak_passphrase(&key, &machine, &config.username) {
        return Err(CliError::Usage(format!(
            "the key's passphrase {why}. Nothing was written"
        )));
    }
    if *prompt.passphrase(KEY_AGAIN)? != *key {
        return Err(CliError::Usage(
            "the two passphrases for the key did not match. Nothing was written".to_owned(),
        ));
    }
    Ok(Passphrases { machine, key })
}

/// Start the copy again in the background and return at once.
///
/// The passphrases go to it through a pipe on its standard input, the rule
/// `setup/secrets.rs` keeps for every window: never its argv or environment,
/// which every process of the same user can read. The pipe is the child's
/// alone, written once and closed.
fn in_background(home: &Path, dir: &Path, passes: &Passphrases) -> Result<String> {
    if [&passes.machine, &passes.key]
        .iter()
        .any(|pass| pass.contains(['\n', '\r']))
    {
        return Err(CliError::Usage(
            "a passphrase holding a line break cannot be handed to a background copy; run \
             `itsanas copy-to-key` without --detach"
                .to_owned(),
        ));
    }
    let exe = std::env::current_exe().map_err(|source| CliError::Io {
        path: PathBuf::from("<this program>"),
        source,
    })?;
    let mut command = Command::new(exe);
    command
        .arg("--home")
        .arg(home)
        .arg("copy-to-key")
        .arg(dir)
        .arg(ON_STDIN)
        .env_remove("ITSANAS_INSTANCE")
        .env_remove("ITSANAS_HOME")
        .env_remove(crate::PASSPHRASE_ENV)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        // Its own process group: a Ctrl-C in this terminal, or the terminal
        // closing, does not stop the copy halfway.
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        // No console window, and out of this console's Ctrl-C.
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    // Before the child starts: it writes its own steps from then on.
    Status(home).step("started", "copying in the background");
    let mut child = command.spawn().map_err(|source| CliError::Io {
        path: PathBuf::from("<background copy>"),
        source,
    })?;
    let lines = Zeroizing::new(format!("{}\n{}\n", *passes.machine, *passes.key));
    let handed = child
        .stdin
        .take()
        .map(|mut pipe| pipe.write_all(lines.as_bytes()));
    if !matches!(handed, Some(Ok(()))) {
        let _ = child.kill();
        return Err(CliError::Usage(
            "the background copy could not be given its passphrases; nothing was written"
                .to_owned(),
        ));
    }
    Ok(format!(
        "Copying in the background. Follow it in Settings, or in {}",
        home.join(STATUS).display()
    ))
}

/// The two lines [`in_background`] writes: this machine's passphrase, then
/// the key's.
fn read_passphrases(input: impl std::io::Read) -> Result<Passphrases> {
    let mut text = Zeroizing::new(String::new());
    input
        .take(64 * 1024)
        .read_to_string(&mut text)
        .map_err(|source| CliError::Io {
            path: PathBuf::from("<standard input>"),
            source,
        })?;
    let mut lines = text.lines();
    let mut next = || {
        lines
            .next()
            .filter(|line| !line.is_empty())
            .map(|line| Zeroizing::new(line.to_owned()))
            .ok_or_else(|| {
                CliError::Usage(format!(
                    "{ON_STDIN} reads two passphrases on standard input, one a line"
                ))
            })
    };
    let machine = next()?;
    let key = next()?;
    Ok(Passphrases { machine, key })
}

#[cfg(test)]
mod tests;
