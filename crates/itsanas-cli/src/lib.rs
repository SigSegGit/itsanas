//! `itsanas` — the command-line interface, as a library and a thin binary.
//!
//! `src/main.rs` only calls [`run`]. Another shell (the phone's app) links this
//! crate and takes what [`shell`] names, so a feature is written once, here, and
//! a shell draws only what it alone can: its secret screens and its way of
//! running in the background.
//!
//! Everything a node can do today. The commands are grouped
//! roughly by how often you need them: `status`, `put`, `get` and `sync` daily;
//! `init`, `login` and `pledge` once per machine; `doctor` and `gc` when
//! something looks wrong.
//!
//! # What this is not yet
//!
//! There is no repair execution and no scheduled storage challenge — both need
//! the coordinator to say who the peers are. Recorded in `docs/ROADMAP.md`
//! rather than glossed over.

// What every shell shares: the machine's account (logging out, deleting), the
// account's devices, the figures a phone shows of its space, the machines to
// dial. `pub` so the phone's app reaches them through `shell`.
pub mod account;
mod app_mode;
mod backup;
mod bench;
mod control;
mod copy_to_key;
mod daemon;
pub mod deleting;
pub mod devices;
mod discovery;
mod drives;
pub mod failure;
pub mod file_tree;
pub mod freeing;
mod host_run;
mod jobs;
mod lifecycle;
pub mod limits;
mod natpmp;
pub mod phone;
pub mod phone_folder;
mod placing;
mod portable;
pub mod rounds;
mod safety;
pub mod trash;
pub use safety::{note_phone_contact, phone_silence};
mod setup;
pub mod shell;
/// Stand-ins for a coordinator and a serving machine, for a shell's tests.
#[cfg(any(test, feature = "testing"))]
pub mod testing;
mod update;
mod upnp;

// The commands, each module one concern of them.
mod cli;
mod coverage;
mod device_command;
mod doctor;
mod drive_commands;
mod email;
mod files;
mod instances;
mod joining;
mod leaving;
mod opening;
mod ports;
mod refusals;
mod registration;
mod space;
mod status;
mod steering;
mod syncing;
#[cfg(test)]
mod test_fixtures;

// What the daemon, `setup`, the phone and the other modules reach as
// `crate::name`, from where it now lives.
use cli::{Cli, Command};
use device_command::device;
use doctor::{Inbound, convert_packs, doctor, gc, network_check};
#[cfg(test)]
use doctor::{conversion_due, convert_stores};
use drive_commands::{backup_command, switch_empty_vault, to_a_drive};
use email::email_command;
use files::{
    fetch_absent, fetch_version_here, file_job, folder, free, get, host_targets, list, put, remove,
    rename, scan, set_folder, taken_output, trash_command, versions,
};
use instances::{instances_report, migrate};
use joining::{
    change_passphrase, check_passphrase, init, login, phrase_grid, recover_from_coordinator,
    whoami, words,
};
use leaving::{delete_account, leave, record_departure, refuse_if_departed, rejoin};
#[cfg(test)]
use opening::pause_daemon;
use opening::{
    PASSPHRASE_ENV, ask_daemon, daemon_holds, open, resume_paused_daemon, set_prompt_context,
};
use ports::{first_free_port, listen_on, port_is_free, settle_listen_port, sibling_ports};
use refusals::{ChainSource, describe_no_room, describe_refusal, describe_refused_chains};
use registration::{
    announce_as, apply_coordinator, coordinator_setting, invite, register, register_and_announce,
    register_only,
};
use space::{keep, pledge, set_pledge_counting, space};
use status::{brief_status, free_for_pledge, render_status_and_copies, status};
use steering::{steer, steer_said};
use syncing::{
    account_targets, add_peer, connect_peer, forget_peer, peer, serve, start_daemon, sync,
};

// The node itself -- keystore, configuration, and the round that honours what
// this device keeps -- lives in `itsanas-node`, because the Android shell needs
// exactly the same things and two implementations of the passphrase handling is
// one too many. These are re-exports, so the paths this binary already used
// still resolve and say where the code went.
mod config {
    pub use itsanas_node::config::*;
}
mod coordinator {
    pub use itsanas_node::coordinator::*;
}
mod error {
    pub use itsanas_node::NodeError as CliError;
    pub use itsanas_node::Result;
}
mod keeping {
    pub use itsanas_node::keeping::*;
}
mod node {
    pub use itsanas_node::node::*;
}

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::Parser;
use itsanas_net::session;

/// How many complete copies elsewhere this system exists to keep.
///
/// Two, because one is a copy and two is a system: with one, the machine
/// holding it going away takes the last copy with it, and nobody finds out
/// until they need it. `REPLICATION_TARGET` is three and counts this machine,
/// which is the same promise with the headroom that variable availability
/// needs -- a copy on a laptop that is shut is a copy you cannot reach today.
pub const SAFE_COPIES: usize = 2;

/// Whether the account has the copies it is for, for a shell that says so itself.
/// No data is no question, not a failed answer to one.
#[must_use]
pub fn account_is_safe(coverage: &itsanas_store::Coverage) -> bool {
    coverage.live_chunks == 0 || coverage.meets(SAFE_COPIES)
}

/// How many machines failed their last storage challenge: what they hold no
/// longer counts as a copy. The daemon's snapshot and a phone both say this
/// number, and an answered round pays a failure off.
///
/// Only failures within `LIVE_FOR`: a machine that failed and then left for
/// good never answers again, and without this the icon stays orange for ever
/// over a machine whose copies stopped counting long ago. One still failing
/// renews its date every round.
///
/// # Errors
///
/// When the store cannot be read.
pub fn machines_failing(node: &Node, now: u64) -> Result<usize> {
    Ok(node
        .store
        .unreliable_devices()?
        .iter()
        .filter(|(_, record)| {
            record.consecutive_failures > 0
                && now.saturating_sub(record.last_failure_unix) < itsanas_store::holders::LIVE_FOR
        })
        .count())
}

/// Why this node's vault refuses a write, if it does. A node away from its
/// vault, or pledging nothing, promised nobody. Makes a write, so a shell asks
/// when a person looks, not on a timer.
#[must_use]
pub fn vault_refuses(node: &Node) -> Option<String> {
    if node.away || node.config.pledge_bytes == 0 {
        return None;
    }
    itsanas_node::pledges::probe_write(&node.vault_dir())
        .err()
        .map(|error| error.to_string())
}

use crate::{
    config::{format_size, parse_size},
    coordinator::{REGISTERED, registered_with},
    error::{CliError, Result},
    node::{Node, phrase_words},
};

/// Whether a panic message is the one std emits when its output has gone away.
///
/// The exact text is std's, and this one was copied from a real run on the
/// Raspberry Pi: `itsanas status | head -20` printed twenty lines and then
///
/// ```text
/// thread 'main' panicked at library/std/src/io/stdio.rs:1166:9:
/// failed printing to stdout: Broken pipe (os error 32)
/// ```
///
/// The tail of that message is the platform's, so the prefix is what is
/// matched. Widening it is the dangerous direction: a hook that swallows the
/// wrong panic hides a real crash, which is why the test for this spends its
/// effort on what must *not* match.
fn looks_like_a_closed_pipe(message: &str) -> bool {
    message.starts_with("failed printing to stdout")
        || message.starts_with("failed printing to stderr")
}

/// Leave quietly when the reader closes the pipe.
///
/// `itsanas status | head` is an ordinary thing to type, and it closes the
/// pipe as soon as head has its lines. Rust disables SIGPIPE at startup, so
/// the next `println!` panics instead of the process dying the way every other
/// command-line program does. `install/provision.sh` pipes `itsanas status`
/// into `head` and printed a Rust panic, and a note about `RUST_BACKTRACE`, in
/// the middle of a successful install.
///
/// Restoring SIGPIPE is the usual fix and needs `unsafe`, which this crate
/// forbids for reasons that outrank this. So the panic is intercepted instead:
/// the cost of the string match failing one day is the behaviour we have now.
fn leave_quietly_when_the_pipe_closes() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<String>()
            .map_or("", String::as_str);
        if looks_like_a_closed_pipe(message) {
            // Not a failure: the reader asked for less than was on offer.
            std::process::exit(0);
        }
        previous(info);
    }));
}

/// The whole program: what `itsanas` does when it is started, as an exit code.
pub fn run() -> ExitCode {
    leave_quietly_when_the_pipe_closes();

    match dispatch() {
        // A daemon asked to restart (`lifecycle.rs`) exits with the code its
        // service manager starts again, on the program now installed.
        Ok(()) if daemon::restart_asked() => {
            ExitCode::from(u8::try_from(lifecycle::RESTART_EXIT).expect("a small exit code"))
        }
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("itsanas: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Which node a command acts on: `--instance`, else `--home`, else [`config::unnamed_home`].
fn resolve_home(instance: Option<&str>, home: Option<PathBuf>) -> Result<PathBuf> {
    let explicit = instance.is_none() && home.is_some();
    Ok(match (instance, home) {
        (Some(name), home) => {
            let named = config::instance_home(name)?;
            // Same refusal as provision.sh: two answers to "which node" is a
            // mistake to report, not a precedence rule to guess at.
            if let Some(home) = home.filter(|home| *home != named) {
                return Err(CliError::Usage(format!(
                    "--instance {name} keeps its node in {}, but the home is {}; drop one",
                    named.display(),
                    home.display()
                )));
            }
            named
        }
        (None, Some(home)) => home,
        (None, None) => config::unnamed_home(&config::user_home())?,
    })
    .and_then(|chosen| {
        // An explicit `--home` is the user's answer; only a derived one can
        // be the wrong guess between HOME and USERPROFILE.
        if explicit {
            return Ok(chosen);
        }
        refuse_stranded(
            chosen,
            &config::user_home(),
            config::other_user_home().as_deref(),
        )
    })
}

/// Refuse a derived home that holds no node when the other home variable's
/// does: see [`config::stranded_node`].
fn refuse_stranded(chosen: PathBuf, base: &Path, other: Option<&Path>) -> Result<PathBuf> {
    match config::stranded_node(&chosen, base, other) {
        None => Ok(chosen),
        Some(there) => Err(CliError::Usage(format!(
            "no node in {}, but there is one in {} (made when the other of HOME and \
             USERPROFILE named the home). Use it with `--home {}`, or move that directory \
             to {} with the daemon stopped. Running `init` or `login` here would make a \
             second identity.",
            chosen.display(),
            there.display(),
            there.display(),
            chosen.display()
        ))),
    }
}

/// The commands that look at every home on the machine, not at one:
/// resolving a home first would make them refuse exactly when they are the
/// answer. `None` for every other command.
fn machine_wide(cli: &Cli) -> Option<Result<()>> {
    match &cli.command {
        Command::Instances => {
            print!("{}", instances_report(&config::user_home()));
            Some(Ok(()))
        }
        // Neither is about one account: `bench` measures this machine with a
        // throwaway identity, and `update` replaces the one program every
        // instance here runs (the hand test of 2026-10-22: both refused on a
        // machine with named instances only).
        Command::Bench { size, quick } => {
            Some(parse_size(size).and_then(|size| bench::run(size, *quick)))
        }
        Command::Update { check } => Some(update::command(*check)),
        Command::Migrate { name } => {
            // It moves ~/.itsanas and nothing else; ignoring an explicit home
            // would migrate a node the user did not name.
            let unnamed = config::user_home().join(".itsanas");
            if cli.instance.is_some() || cli.home.as_ref().is_some_and(|home| *home != unnamed) {
                return Some(Err(CliError::Usage(format!(
                    "migrate moves {} only; drop --instance / --home (ITSANAS_HOME)",
                    unnamed.display()
                ))));
            }
            Some(migrate(name.as_deref()))
        }
        _ => None,
    }
}

fn dispatch() -> Result<()> {
    let cli = Cli::parse();
    if let Some(done) = machine_wide(&cli) {
        return done;
    }
    let home = resolve_home(cli.instance.as_deref(), cli.home)?;
    set_prompt_context(&home, &cli.command);
    let done = command(&home, cli.instance.as_deref(), cli.command);
    resume_paused_daemon(&home);
    done
}

/// The command, on the node at `home`.
fn command(home: &Path, instance: Option<&str>, command: Command) -> Result<()> {
    let home = home.to_owned();
    match command {
        Command::Init { username } => init(&home, &username),
        Command::Login {
            username,
            phrase_file,
            from,
            device,
        } => login(
            &home,
            &username,
            phrase_file.as_deref(),
            from.as_deref(),
            device.as_deref(),
        ),
        Command::Coordinator {
            address,
            device,
            forget,
        } => coordinator_setting(&home, address.as_deref(), device.as_deref(), forget),
        Command::Register {
            recovery,
            withdraw_recovery,
            invite,
        } => register(&home, recovery, withdraw_recovery, invite.as_deref()),
        Command::Invite { uses, days } => invite(&home, uses, days),
        Command::Passphrase { recovery } => change_passphrase(&home, recovery),
        Command::Status { brief } => status(&home, brief),
        command @ (Command::Setup { .. }
        | Command::Signout
        | Command::Signin
        | Command::Settings { .. }) => setup_command(&home, instance, command),
        command @ (Command::Pause { .. }
        | Command::Resume
        | Command::SyncNow
        | Command::Interval { .. }
        | Command::App { .. }) => steer(&home, &command),
        Command::Instances | Command::Migrate { .. } => {
            unreachable!("answered before a home is resolved")
        }
        Command::Whoami => whoami(&home),
        Command::CheckPassphrase => check_passphrase(&home),
        Command::Ls => list(&home),
        Command::Put { path, source } => put(&home, &path, &source),
        Command::Get { path, destination } => get(&home, &path, destination.as_deref()),
        Command::Rm { path, now } => remove(&home, &path, now),
        Command::Free { path, keep } => free(&home, path.as_deref(), keep),
        Command::Mv { from, to } => rename(&home, &from, &to),
        Command::Versions { path, restore } => versions(&home, &path, restore),
        Command::Trash { action } => trash_command(&home, action),
        Command::Folder { path, confirm } => folder(&home, path.as_deref(), confirm),
        Command::Backup { action } => backup_command(&home, action),
        Command::Email { action } => email_command(&home, action),
        command
        @ (Command::Portable { .. } | Command::CopyToKey { .. } | Command::Vault { .. }) => {
            to_a_drive(&home, command)
        }
        Command::Scan { deep } => scan(&home, deep),
        Command::Space {
            pledge,
            keep,
            apply,
        } => space(&home, pledge.as_deref(), keep.as_deref(), apply),
        Command::Keep { size, order, only } => {
            keep(&home, size.as_deref(), order.as_deref(), &only)
        }
        Command::Pledge { size } => pledge(&home, &size),
        Command::Device { what } => device(&home, &what),
        Command::Listen { address } => listen_on(&home, address.as_deref()),
        Command::Announce { address, forget } => announce_as(&home, address.as_deref(), forget),
        Command::Leave { drain_for } => leave(&home, drain_for),
        Command::Rejoin => rejoin(&home),
        Command::DeleteAccount { confirm, drain_for } => delete_account(&home, &confirm, drain_for),
        Command::Serve { listen } => serve(&home, listen.as_deref()),
        Command::Daemon {
            listen,
            interval,
            metered,
            no_discovery,
        } => start_daemon(&home, listen.as_deref(), interval, metered, !no_discovery),
        Command::Sync {
            address,
            metadata_only,
        } => sync(
            &home,
            address.as_deref(),
            if metadata_only {
                session::Scope::Metadata
            } else {
                session::Scope::Everything
            },
        ),
        Command::Peer { action } => peer(&home, action),
        Command::Words => words(&home),
        Command::Doctor { deep } => doctor(&home, deep),
        Command::Update { .. } | Command::Bench { .. } => unreachable!("machine-wide"),
        Command::ConvertPacks => convert_packs(&home),
        Command::Gc { grace } => gc(&home, grace),
    }
}

/// `setup`, `signout`, `signin` and `settings`: the engine of [`setup`], from
/// here.
fn setup_command(home: &Path, instance: Option<&str>, command: Command) -> Result<()> {
    match command {
        Command::Setup {
            text,
            answers,
            phrase_file,
        } => setup::command(home, instance, text, answers.as_deref(), phrase_file),
        Command::Signout => setup::sign::signout(home, instance),
        Command::Signin => setup::sign::signin(home, instance),
        Command::Settings { section } => setup::web::settings(home, instance, section.as_deref()),
        _ => unreachable!("only setup, signout, signin and settings are sent here"),
    }
}

#[cfg(test)]
mod tests {
    use super::looks_like_a_closed_pipe;
    use crate::test_fixtures::fake_node;

    /// A derived home with no node, when the other of HOME / USERPROFILE holds
    /// one at the same place, is refused with `--home` named: `init` there
    /// would mint a second identity beside the real one.
    #[test]
    fn red_team_a_node_under_the_other_home_variable_is_not_shadowed() {
        let base = tempfile::tempdir().expect("temp dir");
        let (profile, home) = (base.path().join("profile"), base.path().join("home"));
        fake_node(&home.join(".itsanas"), "alice", None);
        let chosen = profile.join(".itsanas");

        let refused = super::refuse_stranded(chosen.clone(), &profile, Some(&home))
            .expect_err("a second identity could be made beside the node under HOME");
        let text = refused.to_string();
        assert!(
            text.contains("--home") && text.contains(&home.join(".itsanas").display().to_string()),
            "the refusal must name the node and the flag: {text}"
        );
        assert_eq!(
            super::refuse_stranded(chosen.clone(), &profile, None).expect("no other home"),
            chosen
        );
    }

    #[test]
    fn a_panic_that_is_not_a_closed_pipe_is_never_swallowed() {
        // This is the half of the hook that can do damage. Exiting 0 on the
        // wrong panic turns a crash into a silent success, which is worse than
        // the noisy `head` output it was written to remove.
        for message in [
            "index out of bounds: the len is 3 but the index is 7",
            "called `Option::unwrap()` on a `None` value",
            "attempt to subtract with overflow",
            "assertion failed: the store lied about what it holds",
            "",
        ] {
            assert!(
                !looks_like_a_closed_pipe(message),
                "a real panic would be reported as success: {message:?}"
            );
        }
    }

    #[test]
    fn the_message_std_prints_when_a_pipe_closes_is_recognised() {
        // Copied from a run on the Raspberry Pi, `itsanas status | head -20`.
        // The tail after the colon is the platform's -- "Broken pipe (os error
        // 32)" on Linux, a different sentence on Windows -- so only the prefix
        // is matched, and the two spellings are here to say so.
        assert!(looks_like_a_closed_pipe(
            "failed printing to stdout: Broken pipe (os error 32)"
        ));
        assert!(looks_like_a_closed_pipe(
            "failed printing to stdout: The pipe is being closed. (os error 232)"
        ));
    }
}
