//! The command: `itsanas copy-to-key DIR`, its passphrases asked in windows
//! of the system's own, and its run in the background.

use std::{
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use itsanas_node::node::Identity;
use zeroize::Zeroizing;

use super::{
    KEY_PURPOSE, ON_STDIN, Passphrases, STATUS, Status, check_target, copy, weak_passphrase,
};
use crate::{
    config::Config,
    error::{CliError, Result},
    node::Node,
    setup::{SecretPrompt, secrets, sign},
};

/// What the window asks the second time.
const KEY_AGAIN: &str = "The same passphrase for the USB key again:";

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
    let curl = crate::update::releases();
    let programs = crate::update::Updater::of_this_build(&curl)
        .and_then(|updater| updater.release_bytes())
        .ok();
    copy(
        home,
        dir,
        passes,
        programs,
        |bin| crate::update::Updater::of_this_build(&curl)?.release_binaries(bin),
        crate::register_only,
        |me| {
            if me.config.coordinator.is_none() {
                return Ok(None);
            }
            let live =
                itsanas_node::coordinator::enrolled_as(&me.config, &me.device, me.user.user_id())?;
            Ok(live.map(|list| list.len()))
        },
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
    // Sized once, so no growth leaves a copy of a passphrase behind in a
    // freed allocation the wipe never reaches.
    let mut lines = Zeroizing::new(String::with_capacity(
        passes.machine.len() + passes.key.len() + 2,
    ));
    for pass in [&passes.machine, &passes.key] {
        lines.push_str(pass);
        lines.push('\n');
    }
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

/// `itsanas --home HOME copy-to-key DIR --detach`, for the Settings page: the
/// child opens the two passphrase windows itself (the page never sees a
/// passphrase), then hands the copy to a background run of its own and
/// exits. Its progress is [`progress`].
///
/// # Errors
///
/// When this program cannot be started again.
pub(crate) fn start_detached(home: &Path, dir: &Path) -> Result<std::process::Child> {
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
        .arg("--detach")
        .env_remove("ITSANAS_INSTANCE")
        .env_remove("ITSANAS_HOME")
        .env_remove(crate::PASSPHRASE_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        // No console window flashing behind the passphrase windows.
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.spawn().map_err(|source| CliError::Io {
        path: PathBuf::from("<copy-to-key>"),
        source,
    })
}

/// The two lines [`in_background`] writes: this machine's passphrase, then
/// the key's.
pub(super) fn read_passphrases(mut input: impl std::io::Read) -> Result<Passphrases> {
    /// Two passphrases and their line breaks, with room to spare: a buffer
    /// of a fixed size never grows, so never leaves a copy behind.
    const CAP: usize = 16 * 1024;
    let failed = |source| CliError::Io {
        path: PathBuf::from("<standard input>"),
        source,
    };
    let mut buffer = Zeroizing::new(vec![0u8; CAP]);
    let mut filled = 0;
    loop {
        if filled == CAP {
            return Err(CliError::Usage(format!(
                "{ON_STDIN} reads two passphrases of less than {CAP} bytes together"
            )));
        }
        match input.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(failed(error)),
        }
    }
    let text = std::str::from_utf8(&buffer[..filled])
        .map_err(|_| CliError::Usage(format!("{ON_STDIN} reads two passphrases in UTF-8")))?;
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
