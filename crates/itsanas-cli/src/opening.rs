//! Opening this node: the passphrase prompts, and the running daemon that may
//! hold the store already (paused for a command, or handed the job).

use std::{
    io::IsTerminal as _,
    path::{Path, PathBuf},
};

use crate::cli::Command;
use crate::{config, jobs, lifecycle, node, setup};
use crate::{
    error::{CliError, Result},
    node::Node,
};

/// Environment variable that supplies the passphrase non-interactively.
///
/// For cron jobs and systemd units, which have no terminal to prompt at. A
/// passphrase in the environment is visible to anything that can read the
/// process's environment, so this is a deliberate trade the operator makes,
/// not a default.
pub(crate) const PASSPHRASE_ENV: &str = "ITSANAS_PASSPHRASE";

/// Where `itsanas passphrase` reads the new passphrase when nothing can prompt.
pub(crate) const NEW_PASSPHRASE_ENV: &str = "ITSANAS_NEW_PASSPHRASE";

/// Which node a passphrase prompt is for, set once in `run`.
///
/// "Passphrase:" alone was what three accounts on one machine each printed
/// (Nicolas, 2026-10-14): a person sharing the machine could not tell whose
/// keys a prompt was about to open, nor type the right one.
pub(crate) static PROMPT_FOR: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The account a prompt names: the username of the node at `home` when it
/// has one, the one being created or joined otherwise, and the home itself.
pub(crate) fn prompt_label(home: &Path, command: &Command) -> String {
    let named = match command {
        Command::Init { username } | Command::Login { username, .. } => Some(username.clone()),
        _ => itsanas_node::config::Config::load(&Node::config_path(home))
            .ok()
            .map(|config| config.username)
            .filter(|name| !name.is_empty()),
    };
    match named {
        Some(name) => format!("'{name}' ({})", home.display()),
        None => home.display().to_string(),
    }
}

/// Who and what a passphrase prompt is for, set once per run.
pub(crate) fn set_prompt_context(home: &Path, command: &Command) {
    let _ = PROMPT_FOR.set(prompt_label(home, command));
    if let Some(name) = subcommand_name(std::env::args().skip(1)) {
        let _ = PROMPT_COMMAND.set(name);
    }
}

/// The first argument that is neither a global flag nor a flag's value.
pub(crate) fn subcommand_name(args: impl Iterator<Item = String>) -> Option<String> {
    let mut value_next = false;
    for arg in args {
        if std::mem::replace(
            &mut value_next,
            matches!(arg.as_str(), "--home" | "--instance"),
        ) {
            continue;
        }
        if !arg.starts_with('-') {
            return Some(arg);
        }
    }
    None
}

/// The subcommand a prompt is for (`doctor`, `init`, ...), set once in `run`.
pub(crate) static PROMPT_COMMAND: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// What a passphrase prompt is about, printed before it: whether to choose
/// one or type the one chosen before, for which account on which machine,
/// what it unlocks, and that it is not the 24 words. "Passphrase for
/// 'nicolas' (C:\Users\..)" alone left a person guessing all four
/// (Nicolas, 2026-10-22, during a hand test).
pub(crate) fn passphrase_explanation(
    confirm: bool,
    label: Option<&str>,
    command: Option<&str>,
) -> String {
    let account = label.unwrap_or("this account");
    let doing = command.map_or_else(String::new, |command| format!(" for `itsanas {command}`"));
    if confirm {
        format!(
            "Choose a passphrase for the account {account} on this machine. It protects \
             this machine's copy of the account's keys: you will type it here to start \
             ITSaNAS and for some commands. It is not the 24 recovery words, and each \
             machine of the account may have its own. {} to {} characters, asked twice.",
            node::MIN_PASSPHRASE_CHARS,
            node::MAX_PASSPHRASE_CHARS,
        )
    } else {
        format!(
            "Type the passphrase of the account {account}: the one chosen when this account \
             was set up on this machine. It is not the 24 recovery words. It unlocks this \
             machine's keys{doing}; it is not sent anywhere."
        )
    }
}

/// The prompt for `what`, naming the account it unlocks.
pub(crate) fn prompt_text(what: &str, label: Option<&str>) -> String {
    match label {
        Some(label) => format!("{what} for {label}: "),
        None => format!("{what}: "),
    }
}

/// Obtain the passphrase, from the environment or by prompting.
pub(crate) fn passphrase(confirm: bool) -> Result<String> {
    if let Ok(value) = std::env::var(PASSPHRASE_ENV) {
        return Ok(value);
    }

    if !std::io::stdin().is_terminal() {
        return Err(CliError::Usage(format!(
            "no terminal to prompt on. Set {PASSPHRASE_ENV} for non-interactive \
             use, understanding that anything able to read this process's \
             environment can then read the passphrase."
        )));
    }

    ask_passphrase(
        confirm,
        PROMPT_FOR.get().map(String::as_str),
        PROMPT_COMMAND.get().map(String::as_str),
        &mut || rpassword::ConfigBuilder::new().build(),
    )
}

/// What the terminal shows before a passphrase is read: the explanation, a
/// line break, then the prompt -- one text, which `rpassword` writes and
/// flushes in one go on the stream it then reads from (`/dev/tty`, or
/// `CONOUT$` on Windows). The explanation used to go to stderr and the prompt
/// to the terminal, so with stderr piped through `tee` the prompt came first
/// and the explanation after it, on the same line (Mac, hand test of
/// 2026-10-22).
pub(crate) fn passphrase_prompt(
    explanation: Option<&str>,
    what: &str,
    label: Option<&str>,
) -> String {
    let prompt = prompt_text(what, label);
    match explanation {
        Some(explanation) => format!("{explanation}\n{prompt}"),
        None => prompt,
    }
}

/// Ask the passphrase on the terminal `terminal` opens, once or (choosing
/// one) twice. The terminal is a parameter so that a test sees what is
/// written, and in which order; [`passphrase`] passes `rpassword`'s own.
pub(crate) fn ask_passphrase(
    confirm: bool,
    label: Option<&str>,
    command: Option<&str>,
    terminal: &mut dyn FnMut() -> rpassword::Config,
) -> Result<String> {
    let read = |prompt: String, config: rpassword::Config| {
        rpassword::prompt_password_with_config(prompt, config).map_err(|error| CliError::Io {
            path: PathBuf::from("<terminal>"),
            source: error,
        })
    };
    let explanation = passphrase_explanation(confirm, label, command);
    let entered = read(
        passphrase_prompt(Some(&explanation), "Passphrase", label),
        terminal(),
    )?;

    if confirm {
        let again = read(
            passphrase_prompt(None, "Confirm passphrase", label),
            terminal(),
        )?;
        if again != entered {
            return Err(CliError::Usage("the passphrases did not match".to_owned()));
        }
    }

    if entered.is_empty() {
        return Err(CliError::Usage(
            "an empty passphrase protects nothing".to_owned(),
        ));
    }

    Ok(entered)
}

pub(crate) fn open(home: &Path) -> Result<Node> {
    if !Node::exists(home) {
        return Err(CliError::NoNode(home.to_path_buf()));
    }
    let passphrase = passphrase(false)?;
    if daemon_holds(home) {
        pause_daemon(home)?;
    }
    Node::open(home, &passphrase)
}

/// Set when this command stopped the running daemon to open the store: it
/// starts again once the command is done ([`resume_paused_daemon`]).
pub(crate) static PAUSED_DAEMON: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Stop the daemon that holds the store, for the length of this command, as
/// the Settings page does. Before 2026-11-17 a command that opens the store
/// refused while the daemon ran, and a machine set up as a service always runs
/// it: `keep`, `peer`, `device` and the rest needed it stopped by hand. The
/// commonest commands do not stop it at all: they are handed to it (`jobs.rs`).
pub(crate) fn pause_daemon(home: &Path) -> Result<()> {
    eprintln!(
        "the running daemon holds this account: it stops for this command and starts again after"
    );
    if !lifecycle::stop_and_wait(home) {
        return Err(CliError::Usage(
            "the running daemon did not stop; nothing was changed. Try again in a moment."
                .to_owned(),
        ));
    }
    PAUSED_DAEMON.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// Start again the daemon [`pause_daemon`] stopped: its service, when the
/// account at `home` has one here; otherwise say how.
pub(crate) fn resume_paused_daemon(home: &Path) {
    use setup::service::ServiceControl as _;
    if !PAUSED_DAEMON.swap(false, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let instance = config::node_homes(&config::user_home())
        .into_iter()
        .find(|(_, at)| at == home)
        .map(|(name, _)| name);
    let started = instance.is_some_and(|name| {
        // `node_homes` names the default node "(unnamed)": its service has
        // no instance suffix.
        let instance = (name != "(unnamed)").then_some(name.as_str());
        let service = setup::service::Platform::of_this_machine(home, instance);
        service.installed() && service.start().is_ok()
    });
    if started {
        eprintln!("the daemon runs again");
    } else {
        eprintln!(
            "the daemon was stopped for this command and has no service here to start it: \
             start it again with `itsanas daemon`"
        );
    }
}

/// Whether a passphrase could be obtained without failing.
///
/// Mirrors the two ways [`passphrase`] can succeed: the environment variable,
/// or a terminal to prompt on. Asked before opening a node so that a command
/// which has a usable answer without keys can give it, instead of failing with
/// advice about environment variables to somebody who only wanted to know
/// whether their files are safe.
pub(crate) fn passphrase_available() -> bool {
    std::env::var(PASSPHRASE_ENV).is_ok() || std::io::stdin().is_terminal()
}

pub(crate) fn daemon_holds(home: &Path) -> bool {
    Node::exists(home) && itsanas_store::Store::is_locked(Node::store_path(home))
}

/// Ask the running daemon for a disk job and wait for its answer, printing it.
pub(crate) fn hand_to_daemon(home: &Path, kind: jobs::Kind, path: &Path) -> Result<()> {
    println!("{}", ask_daemon(home, kind, path, None)?);
    Ok(())
}

/// Ask the running daemon for a job and wait for what it says. What it does
/// on the way goes to standard error: standard output carries the answer,
/// which for `ls` and `get` is the listing or the file.
pub(crate) fn ask_daemon(
    home: &Path,
    kind: jobs::Kind,
    path: &Path,
    name: Option<&str>,
) -> Result<String> {
    let wanted = std::path::absolute(path).map_err(|source| CliError::Io {
        path: path.to_owned(),
        source,
    })?;
    let id = jobs::ask_named(home, kind, &wanted, name).map_err(CliError::Usage)?;
    if !kind.is_file_command() {
        eprintln!(
            "the running daemon does this; waiting for it (Ctrl-C stops waiting, not the job)"
        );
    }
    let asked = std::time::Instant::now();
    let mut said_running = false;
    let mut alone_since: Option<std::time::Instant> = None;
    let moving = matches!(kind, jobs::Kind::MoveVault { .. });
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let report = jobs::result(home);
        let mine = report.as_ref().filter(|report| report.id == id);
        // No daemon holds the store: during a move that is the copy, and for
        // anything else for more than a moment, a daemon that went away.
        let held = daemon_holds(home);
        alone_since = if held {
            None
        } else {
            alone_since.or_else(|| Some(std::time::Instant::now()))
        };
        let gone =
            alone_since.is_some_and(|since| since.elapsed() > std::time::Duration::from_secs(30));
        match mine {
            Some(report) if report.state == jobs::State::Done => {
                return Ok(report.said.clone());
            }
            Some(report) if report.state == jobs::State::Failed => {
                return Err(CliError::Usage(report.said.clone()));
            }
            Some(report) => {
                if !said_running {
                    eprintln!("{}...", report.said);
                    said_running = true;
                }
                if gone && !moving {
                    return Err(CliError::Usage(
                        "the daemon stopped during the job; it says how it ended when it starts \
                         again"
                            .to_owned(),
                    ));
                }
            }
            None if asked.elapsed() > std::time::Duration::from_secs(30)
                && (gone || !home.join(jobs::JOB).exists()) =>
            {
                return Err(CliError::Usage(if gone {
                    let _ = std::fs::remove_file(home.join(jobs::JOB));
                    "the daemon stopped before taking the job; run the command again".to_owned()
                } else {
                    "another disk job, from Settings or a second terminal, replaced this one; \
                     run it again once that one is done"
                        .to_owned()
                }));
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::passphrase_explanation;
    use super::prompt_label;
    use super::prompt_text;
    use crate::cli::Command;

    #[test]
    fn red_team_a_passphrase_prompt_names_the_account_it_unlocks() {
        // Three accounts on one machine each asked "Passphrase:" (Nicolas,
        // 2026-10-14). Sabotage: drop the label and this goes red.
        let base = tempfile::tempdir().unwrap();
        let home = base.path().join(".itsanas-test");
        let label = prompt_label(
            &home,
            &Command::Init {
                username: "test".to_owned(),
            },
        );
        let text = prompt_text("Passphrase", Some(&label));
        assert!(text.contains("'test'"), "{text}");
        assert!(text.contains(".itsanas-test"), "{text}");
        assert_eq!(prompt_text("Passphrase", None), "Passphrase: ");
    }

    /// A terminal for `ask_passphrase` that keeps one transcript: what was
    /// written reaches it only when flushed, and the first read of each
    /// answer is marked `<read>` -- so the order a person sees is the order
    /// in the transcript.
    #[derive(Clone, Default)]
    struct Transcript {
        shown: std::rc::Rc<std::cell::RefCell<String>>,
        pending: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
    }

    impl std::io::Write for Transcript {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.pending.borrow_mut().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            let pending = std::mem::take(&mut *self.pending.borrow_mut());
            self.shown
                .borrow_mut()
                .push_str(&String::from_utf8_lossy(&pending));
            Ok(())
        }
    }

    struct Typed {
        answer: std::io::Cursor<Vec<u8>>,
        seen: Transcript,
        marked: bool,
    }

    impl std::io::Read for Typed {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !std::mem::replace(&mut self.marked, true) {
                self.seen.shown.borrow_mut().push_str("<read>");
            }
            self.answer.read(buf)
        }
    }

    #[test]
    fn red_team_the_passphrase_explanation_is_on_the_terminal_and_flushed_before_the_prompt() {
        // The Mac log through `tee` read "Passphrase for 'test' (...): Type
        // the passphrase of the account 'test' ...": the explanation went to
        // stderr, the prompt to the terminal. Sabotage: print the explanation
        // with eprintln! again and pass the bare prompt -- the transcript
        // then starts with the prompt.
        let terminal = Transcript::default();
        let label = Some("'test' (/Users/n/.itsanas-test)");
        let typed = super::ask_passphrase(true, label, Some("init"), &mut || {
            rpassword::ConfigBuilder::new()
                .input_reader(Typed {
                    answer: std::io::Cursor::new(b"correct horse\n".to_vec()),
                    seen: terminal.clone(),
                    marked: false,
                })
                .output_writer(terminal.clone())
                .build()
        })
        .expect("the passphrase is read");
        assert_eq!(typed, "correct horse");
        let shown = terminal.shown.borrow().clone();
        let explanation = passphrase_explanation(true, label, Some("init"));
        let expected = format!("{explanation}\n{}<read>", prompt_text("Passphrase", label));
        assert!(
            shown.starts_with(&expected),
            "the explanation, then the prompt, both shown before the answer is read: {shown:?}"
        );
        assert_eq!(
            shown.matches(explanation.as_str()).count(),
            1,
            "explained once, not again at the confirmation: {shown:?}"
        );
        assert!(
            shown.contains(&format!(
                "{}<read>",
                prompt_text("Confirm passphrase", label)
            )),
            "{shown:?}"
        );
    }

    #[test]
    fn a_passphrase_prompt_says_whether_to_choose_or_type_and_what_it_is_not() {
        let typing = passphrase_explanation(false, Some("'nicolas' (C:\\home)"), Some("doctor"));
        assert!(typing.starts_with("Type"), "{typing}");
        assert!(typing.contains("'nicolas'"), "{typing}");
        assert!(typing.contains("itsanas doctor"), "{typing}");
        assert!(typing.contains("not the 24"), "{typing}");
        let choosing = passphrase_explanation(true, Some("'test' (x)"), Some("init"));
        assert!(choosing.starts_with("Choose"), "{choosing}");
        assert!(choosing.contains("not the 24"), "{choosing}");
        assert!(choosing.contains("twice"), "{choosing}");
        let args = ["--home", "C:\\x", "doctor", "--deep"].map(String::from);
        assert_eq!(
            super::subcommand_name(args.into_iter()).as_deref(),
            Some("doctor")
        );
    }
}
