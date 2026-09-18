//! `itsanas` — the command-line interface.
//!
//! Everything a node can do today, from one binary. The commands are grouped
//! roughly by how often you need them: `status`, `put`, `get` and `sync` daily;
//! `init`, `login` and `pledge` once per machine; `doctor` and `gc` when
//! something looks wrong.
//!
//! # What this is not yet
//!
//! There is no repair execution and no scheduled storage challenge — both need
//! the coordinator to say who the peers are. Recorded in `docs/ROADMAP.md`
//! rather than glossed over.

mod bench;
mod daemon;
mod discovery;

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
    fmt::Write as _,
    io::{IsTerminal as _, Read as _, Write as _},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::atomic::AtomicBool,
};

use clap::{Parser, Subcommand};
use itsanas_crypto::DeviceId;
use itsanas_net::{PeerClient, PeerServer, PeerService, Pledge, session};
use itsanas_store::REPLICATION_TARGET;

/// How many complete copies elsewhere this system exists to keep.
///
/// Two, because one is a copy and two is a system: with one, the machine
/// holding it going away takes the last copy with it, and nobody finds out
/// until they need it. `REPLICATION_TARGET` is three and counts this machine,
/// which is the same promise with the headroom that variable availability
/// needs -- a copy on a laptop that is shut is a copy you cannot reach today.
const SAFE_COPIES: usize = 2;

use crate::{
    config::{format_size, parse_size},
    error::{CliError, Result},
    node::{Node, SNAPSHOT},
};

/// Environment variable that supplies the passphrase non-interactively.
///
/// For cron jobs and systemd units, which have no terminal to prompt at. A
/// passphrase in the environment is visible to anything that can read the
/// process's environment, so this is a deliberate trade the operator makes,
/// not a default.
const PASSPHRASE_ENV: &str = "ITSANAS_PASSPHRASE";

/// Where `itsanas passphrase` reads the new passphrase when nothing can prompt.
const NEW_PASSPHRASE_ENV: &str = "ITSANAS_NEW_PASSPHRASE";

/// How many machines should hold each chunk, this one included.
///

#[derive(Parser)]
#[command(
    name = "itsanas",
    version,
    about = "Peer-to-peer mutual storage: your data on their disks, unreadable to them",
    long_about = None,
)]
struct Cli {
    /// Where this node keeps its state.
    #[arg(long, global = true, env = "ITSANAS_HOME")]
    home: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

/// What to do with the devices on this account.
#[derive(Debug, Subcommand)]
enum DeviceCommand {
    /// List them, as the coordinator has them.
    List,
    /// Withdraw one, so nothing dials it again.
    ///
    /// Takes the full device id or the twelve-character short form -- the short
    /// form because that is what the error naming a dead device prints, and
    /// asking somebody to go and find the long one is asking them to do work
    /// the program can do.
    Forget {
        /// The device to withdraw.
        device: String,
    },
}

#[derive(Subcommand)]
enum Command {
    /// Create a new account on this machine and print its recovery phrase.
    Init {
        /// Account name, as it would be registered with a coordinator.
        #[arg(long)]
        username: String,
    },
    /// Restore an existing account on this machine.
    ///
    /// From the 24-word phrase by default, or from a coordinator with
    /// `--from <host:port>` if the account lodged a recovery container.
    Login {
        #[arg(long)]
        username: String,
        /// Read the 24-word phrase from this file instead of prompting.
        #[arg(long)]
        phrase_file: Option<PathBuf>,
        /// Recover from this coordinator using the passphrase alone.
        #[arg(long, conflicts_with = "phrase_file")]
        from: Option<String>,
        /// The device that coordinator must prove itself to be.
        #[arg(long, requires = "from")]
        device: Option<String>,
    },
    /// Change the passphrase that protects this machine's keys.
    ///
    /// Only this machine's keystore. Whatever starts the daemon still supplies
    /// the old one, and a recovery container lodged with a coordinator stays
    /// sealed under the passphrase it was lodged with; the command says how to
    /// update both. Reads the new passphrase from `ITSANAS_NEW_PASSPHRASE` when
    /// there is no terminal to prompt on.
    Passphrase {
        /// Also re-seal the recovery container at the coordinator under the new
        /// passphrase.
        ///
        /// Without it the container keeps the old passphrase, and a recovery
        /// months later fails with the one this machine now uses. Needs the
        /// daemon stopped, because re-sealing opens the node.
        #[arg(long)]
        recovery: bool,
    },
    /// Show this node's identity, contents and hosting.
    Status,
    /// Show this account's public identity.
    Whoami,
    /// List the files this node knows about.
    Ls,
    /// Store a file.
    Put {
        /// Logical path inside the account, e.g. `notes/todo.txt`.
        path: String,
        /// Local file to read. `-` reads standard input.
        source: PathBuf,
    },
    /// Retrieve a file.
    Get {
        path: String,
        /// Where to write it. Omit to write to standard output.
        destination: Option<PathBuf>,
    },
    /// Delete a file, leaving a tombstone so it does not come back.
    Rm { path: String },
    /// Set, or show, the directory kept in step with this account.
    ///
    /// Once set, files put in it are uploaded, files deleted from it are
    /// deleted everywhere, and changes from other devices appear in it.
    Folder {
        /// Where the synced folder lives. Omit to print the current one.
        path: Option<PathBuf>,
        /// Apply deletions a pass held back because there were too many.
        ///
        /// A pass that would remove most of the folder holds the deletions
        /// instead of writing them: an unmounted disk and a folder somebody
        /// emptied look identical from here, and deletions replicate to every
        /// machine of the account. This says a person looked and they really
        /// are meant to go.
        #[arg(long)]
        confirm: bool,
    },
    /// Reconcile the synced folder with the store, once.
    ///
    /// The daemon does this continuously; this is for running it by hand.
    Scan {
        /// Re-hash every file instead of trusting size and modification time.
        ///
        /// Catches a file rewritten within the same second at exactly the same
        /// length, which the fast path cannot see.
        #[arg(long)]
        deep: bool,
    },
    /// Say how much of your own data to keep on this device.
    ///
    /// A phone has a few gigabytes free and an account can have hundreds. What
    /// does not fit is not downloaded -- it stays listed and can be fetched
    /// when you open it, rather than being brought down and deleted.
    ///
    /// This is room for *your* data. `pledge` is room you offer *others*, and
    /// the two are not the same decision: having a terabyte free is not
    /// agreeing to lend a terabyte.
    Keep {
        /// e.g. `2G`, or `all` to hold everything.
        size: Option<String>,
        /// Which files matter most when the limit cannot hold them all:
        /// `newest`, `oldest` or `smallest`.
        ///
        /// Without this the limit bounds the quantity and says nothing about
        /// the choice, so what a device ends up with is whatever the log
        /// replayed first -- which is the order things were written, possibly
        /// by another machine, years ago.
        #[arg(long)]
        order: Option<String>,
        /// Hold only these paths, and let go of the rest. Repeatable.
        ///
        /// `--only all` clears the restriction. A directory prefix matches
        /// everything under it; `Photos` does not match `Photos-old`.
        #[arg(long)]
        only: Vec<String>,
    },
    /// Show what this machine can offer, what that earns, and what limits it.
    ///
    /// Two limits decide how much of your own data a device may hold: the free
    /// space on the disk the node lives on, and what you have offered other
    /// people — this network gives storage in proportion to storage provided.
    /// Both, together, before anything is committed to.
    Space {
        /// Space to offer other people, e.g. `100G`.
        #[arg(long)]
        pledge: Option<String>,
        /// Space for your own data, e.g. `10G`, or `all`.
        #[arg(long)]
        keep: Option<String>,
        /// Set them, rather than only saying whether they would fit.
        #[arg(long)]
        apply: bool,
    },
    /// Say how much space this node offers to other people.
    Pledge {
        /// e.g. `500M`, `10G`, `1T`.
        size: String,
    },
    /// The devices enrolled in this account.
    Device {
        #[command(subcommand)]
        what: DeviceCommand,
    },
    /// Show or set the address this node serves on.
    ///
    /// This is the address `register` and `announce` publish, so changing it
    /// with `serve --listen` alone is not enough: the coordinator would keep
    /// handing other members the old port and they would dial whatever now
    /// answers there. Set it here, and the published address follows.
    Listen {
        /// e.g. `0.0.0.0:9797`. Omit to print the current one.
        address: Option<String>,
    },
    /// The address to publish, when it is not where this node listens.
    ///
    /// What a member on another network dials to reach this machine: the name
    /// or public address of a forwarded port, or a global IPv6 address, with
    /// the port as seen from outside. Without it a node publishes its address
    /// on whatever LAN it is on, which is right at home and useless anywhere
    /// else.
    ///
    /// A machine that moves -- a laptop, a phone -- wants none of this. It has
    /// no address another network can dial, it takes part by dialling out, and
    /// one reachable side per pair is enough.
    Announce {
        /// e.g. `ngas.fr:9801`. Omit to print the current setting.
        address: Option<String>,
        /// Go back to publishing the address this node reaches from.
        #[arg(long, conflicts_with = "address")]
        forget: bool,
    },
    /// Serve peers.
    Serve {
        /// Address to listen on. Defaults to the configured `listen`.
        #[arg(long)]
        listen: Option<String>,
    },
    /// Serve peers and sync on a timer, in one process, until interrupted.
    ///
    /// This is how a node is meant to run. `serve` and `sync` cannot run
    /// simultaneously against the same node — the index is held under an
    /// exclusive lock — so two cron entries would fight. The daemon does both,
    /// and unlocks the keys once instead of on every scheduled sync.
    Daemon {
        #[arg(long)]
        listen: Option<String>,
        /// Seconds between sync rounds. Omit to let the sync policy decide.
        ///
        /// The policy in `itsanas-policy` is what the phone and the Mac shell
        /// use too, so leaving this alone means every machine reaches the same
        /// schedule from the same decision table instead of three copies of a
        /// number that drift apart.
        #[arg(long)]
        interval: Option<u64>,
        /// This connection is charged by the gigabyte.
        ///
        /// A laptop tethered to a phone, or a machine on a capped plan. The
        /// daemon then exchanges the signed log — kilobytes — and downloads no
        /// file contents at all, once a day rather than every five minutes.
        ///
        /// Asked for rather than detected: Windows and macOS both expose the
        /// answer, but guessing it from the interface type is how a sync tool
        /// ends up costing somebody fifty euros, and a phone's own hotspot is
        /// Wi-Fi.
        #[arg(long)]
        metered: bool,
        /// Do not announce this node on the local network, and do not listen
        /// for others.
        ///
        /// Local discovery is what lets machines in one house find each other
        /// with nothing configured. Turning it off means every peer has to be
        /// added by hand, and is for networks where broadcast traffic is
        /// unwelcome or where the node should not advertise that it exists.
        #[arg(long)]
        no_discovery: bool,
    },
    /// Run one sync round against a peer.
    Sync {
        /// Peer address, e.g. `pi.local:9797`. Omit to use configured peers.
        address: Option<String>,
        /// Exchange the log but download nothing.
        ///
        /// For an expensive connection — mobile data, or a laptop tethered to a
        /// phone. Files appear in `itsanas ls` marked "not here", and a later
        /// round without this flag fetches them.
        #[arg(long)]
        metadata_only: bool,
    },
    /// Set, or show, the coordinator this node uses.
    ///
    /// A coordinator is optional. Machines on the same network find each other
    /// with no server at all; what this adds is reaching a machine on a
    /// *different* network, and recovering an account from a passphrase.
    Coordinator {
        /// `host:port`. Omit to show the current setting.
        address: Option<String>,
        /// The device the coordinator must prove itself to be.
        ///
        /// Get it with `itsanas-coordinator --identity`. Without it an address
        /// that resolves elsewhere is trusted; with it, refused.
        #[arg(long)]
        device: Option<String>,
        /// Stop using a coordinator.
        #[arg(long, conflicts_with_all = ["address", "device"])]
        forget: bool,
    },
    /// Invite somebody to join the coordinator this node uses.
    ///
    /// Prints a code, once. It is not stored anywhere: send it to the person
    /// joining by whatever means you would have used anyway, and if it is lost,
    /// issue another.
    Invite {
        /// How many accounts it may admit. One unless you say otherwise.
        #[arg(long, default_value_t = 1)]
        uses: u32,
        /// How many days it stays valid.
        #[arg(long, default_value_t = 7)]
        days: u64,
    },
    /// Register this account and device with the configured coordinator.
    Register {
        /// Also lodge a recovery container sealed under this machine's passphrase.
        ///
        /// It lets a new machine be restored with a username and a passphrase
        /// instead of 24 words. The trade is real: anybody who steals the
        /// coordinator's database can attack that passphrase offline, so it is
        /// off unless asked for, and `--withdraw-recovery` takes it back.
        #[arg(long)]
        recovery: bool,
        /// Withdraw a previously lodged recovery container.
        #[arg(long, conflicts_with = "recovery")]
        withdraw_recovery: bool,
        /// The invitation code somebody sent you.
        ///
        /// Needed only by a coordinator that admits new members by invitation,
        /// and only the first time: re-registering is how a member refreshes
        /// their keys and never needs a fresh code.
        #[arg(long, value_name = "CODE")]
        invite: Option<String>,
    },
    /// Add a peer to the configuration.
    Peer {
        #[command(subcommand)]
        action: PeerAction,
    },
    /// Check that everything this node claims to hold is actually here.
    Doctor {
        /// Also reassemble and re-hash every file. O(data), not O(metadata).
        #[arg(long)]
        deep: bool,
    },
    /// Measure this machine: how fast it chunks, seals, stores and reads.
    ///
    /// The question is not whether a laptop is fast enough — it is whether a
    /// Raspberry Pi is, and the only person who can answer that is the person
    /// holding one. Nothing here touches your account: a throwaway identity and
    /// a scratch directory are made for the run and deleted after it.
    Bench {
        /// How much data to push through each stage, e.g. `64M`, `1G`.
        ///
        /// Generated on the fly, so a large size costs no extra memory.
        #[arg(long, default_value = "256M")]
        size: String,
        /// Fewer samples: a rough answer in a fraction of the time.
        #[arg(long)]
        quick: bool,
    },
    /// Reclaim space from files that were deleted or overwritten.
    Gc {
        /// How long a chunk must have been unreferenced, in seconds.
        ///
        /// The grace period exists because "unreferenced" is a local judgement
        /// made with incomplete information: a peer may still be fetching a
        /// chunk whose file this device just deleted.
        #[arg(long, default_value_t = 86_400)]
        grace: u64,
    },
}

#[derive(Subcommand)]
enum PeerAction {
    /// Remember a peer address.
    Add { address: String },
    /// List remembered peers.
    List,
    /// Forget a peer address.
    Remove { address: String },
    /// Find another member by name and remember where their machines are.
    ///
    /// On one network the discovery beacons do this already. This is for the
    /// other case: a member somewhere else, whose address you would otherwise
    /// have to be told and type in by hand.
    Find {
        /// Their username, as registered with the coordinator.
        username: String,
    },
}

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

fn main() -> ExitCode {
    leave_quietly_when_the_pipe_closes();

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("itsanas: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let home = cli.home.unwrap_or_else(config::default_home);

    match cli.command {
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
        Command::Status => status(&home),
        Command::Whoami => whoami(&home),
        Command::Ls => list(&home),
        Command::Put { path, source } => put(&home, &path, &source),
        Command::Get { path, destination } => get(&home, &path, destination.as_deref()),
        Command::Rm { path } => remove(&home, &path),
        Command::Folder { path, confirm } => folder(&home, path.as_deref(), confirm),
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
        Command::Serve { listen } => serve(&home, listen.as_deref()),
        Command::Daemon {
            listen,
            interval,
            metered,
            no_discovery,
        } => daemon::run(
            &open(&home)?,
            listen.as_deref(),
            interval.map(|seconds| std::time::Duration::from_secs(seconds.max(1))),
            metered,
            !no_discovery,
        ),
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
        Command::Doctor { deep } => doctor(&home, deep),
        Command::Bench { size, quick } => bench::run(parse_size(&size)?, quick),
        Command::Gc { grace } => gc(&home, grace),
    }
}

// ---------------------------------------------------------------------------
// Passphrase handling
// ---------------------------------------------------------------------------

/// Obtain the passphrase, from the environment or by prompting.
fn passphrase(confirm: bool) -> Result<String> {
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

    let entered = rpassword::prompt_password("Passphrase: ").map_err(|error| CliError::Io {
        path: PathBuf::from("<terminal>"),
        source: error,
    })?;

    if confirm {
        let again =
            rpassword::prompt_password("Confirm passphrase: ").map_err(|error| CliError::Io {
                path: PathBuf::from("<terminal>"),
                source: error,
            })?;
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

fn open(home: &Path) -> Result<Node> {
    if !Node::exists(home) {
        return Err(CliError::NoNode(home.to_path_buf()));
    }
    Node::open(home, &passphrase(false)?)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn init(home: &Path, username: &str) -> Result<()> {
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

    for (index, word) in phrase.as_str().split_whitespace().enumerate() {
        print!("{:>2}. {:<12}", index + 1, word);
        if index % 4 == 3 {
            println!();
        }
    }
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
fn change_passphrase(home: &Path, recovery: bool) -> Result<()> {
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
    if new.is_empty() {
        return Err(CliError::Usage(
            "an empty passphrase protects nothing".to_owned(),
        ));
    }

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

    if let Err(error) = Node::change_passphrase(home, &current, &new) {
        if recovery {
            eprintln!(
                "the recovery container is already under the NEW passphrase, and this \
                 machine's keystore is still under the old one. Run this again."
            );
        }
        return Err(error);
    }
    println!("passphrase changed for this machine's keystore.");

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

fn login(
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
    let mut node = Node::restore(home, &passphrase(true)?, username, phrase.trim())?;
    settle_listen_port(&mut node)?;

    println!("Account restored.");
    println!("  user id : {}", node.store.owner());
    println!(
        "  device  : {} (new for this machine)",
        node.store.device_id()
    );
    println!();
    println!("Nothing has been downloaded yet. Run `itsanas sync <peer>` to pull");
    println!("your data from a peer or a host that is holding it.");

    Ok(())
}

/// Say which peers have failed a storage challenge, if any have.
///
/// Silence means every peer that has ever been audited answered, which is the
/// ordinary case and worth no words at all.
fn report_unreliable_peers(node: &Node) -> Result<()> {
    let unreliable = node.store.unreliable_devices()?;
    if unreliable.is_empty() {
        return Ok(());
    }

    println!();
    println!("peers that have failed a storage challenge");
    for (device, record) in &unreliable {
        match record.complaint(device) {
            Some(complaint) => println!("  {complaint}"),
            None => println!(
                "  {} answered {} and failed {}, and is answering now",
                device.short(),
                record.passed,
                record.failed
            ),
        }
    }
    Ok(())
}

/// Whether chunks could be spread around instead of every holder taking
/// everything -- and it is off below a threshold on purpose.
///
/// With two peers and a target of two copies, every chunk must go to both, so
/// spreading would give each chunk one holder instead of two: a privacy
/// preference turned into data loss, on the networks least able to afford it.
///
/// The capacity half is why `offered` exists and why it is `None` today. A node
/// learns its peers' pledges from the coordinator and nothing asks for them, so
/// the honest answer is "not known" rather than "fine" -- nine peers with a
/// gigabyte each cannot spread four terabytes, and a threshold in machines
/// cannot see it.
/// The other direction, and it is not the same question.
///
/// Copies are about surviving loss; this is about who could read you if the
/// sealing ever failed, and about whether this can scale at all — if the unit of
/// hosting were "a whole account", somebody offering four terabytes would need
/// peers who could each take four terabytes.
fn concentration_report(node: &Node, coverage: &itsanas_store::Coverage) -> Result<String> {
    let mut out = String::new();
    macro_rules! w {
        ($($arg:tt)*) => {{ let _ = writeln!(out, $($arg)*); }};
    }

    if coverage.someone_holds_everything() {
        w!(
            concat!(
                "  concentrated   one machine holds all {} of your chunks. ",
                "Sealed, but a whole set"
            ),
            coverage.live_chunks
        );
        w!("                 unavoidable with few peers; spread as more join");
    } else if coverage.largest_share > 0 {
        w!(
            "  spread         no machine holds more than {} of your {} chunks",
            coverage.largest_share,
            coverage.live_chunks
        );
    }

    let _ = write!(
        out,
        "{}",
        spreading_report(
            coverage.distinct_holders,
            node.store.stats()?.bytes_on_disk,
            None
        )
    );

    let short = node.store.under_replicated(REPLICATION_TARGET)?;
    if !short.is_empty() {
        w!(
            concat!(
                "  headroom       {} chunks are on fewer than {} machines, ",
                "counting this one"
            ),
            short.len(),
            REPLICATION_TARGET
        );
    }

    Ok(out)
}

fn spreading_report(candidates: usize, stored: u64, offered: Option<u64>) -> String {
    use itsanas_placement::Blocked;

    let mut out = String::new();
    macro_rules! w {
        ($($arg:tt)*) => {{ let _ = writeln!(out, $($arg)*); }};
    }

    let advice = itsanas_placement::spreading(candidates, REPLICATION_TARGET, stored, offered);
    match advice.blocked_by {
        None => w!(
            "  spreading      on: {} machines with room for {} copies",
            advice.candidates,
            REPLICATION_TARGET
        ),
        Some(Blocked::NothingStored) => {}
        Some(Blocked::TooFewHolders { have, need }) => {
            w!(
                "  spreading      off: {have} machines hold anything of yours, and {need} are needed"
            );
            w!("                 until then every holder takes everything, which is right");
        }
        Some(Blocked::NotEnoughSpace { offered, needed }) => {
            w!(
                "  spreading      off: your peers offer {}, and {} copies need {}",
                format_size(offered),
                REPLICATION_TARGET,
                format_size(needed)
            );
        }
        Some(Blocked::CapacityUnknown) => {
            w!("  spreading      off: nobody has said how much room they have");
            w!("                 the coordinator knows; nothing asks it yet");
        }
    }
    out
}

/// The part of `status` that answers the question this project exists for.
///
/// Separated because it is the headline and deserves to be readable on its own,
/// and because `render_status` went over its line budget the moment it grew.
/// Chunks this account has that this machine does not hold.
///
/// The measure of how much of the reassurance in `status` is out of scope. It
/// is zero on every machine that holds its whole account, which is why it went
/// unnoticed until one did not.
fn chunks_not_here(node: &Node) -> Result<usize> {
    let listing = itsanas_store::catalogue(&node.store, &node.vault)?;
    let absent: std::collections::BTreeSet<String> = listing
        .files
        .into_iter()
        .filter(|file| file.presence == itsanas_store::Presence::Absent)
        .map(|file| file.path)
        .collect();

    if absent.is_empty() {
        return Ok(0);
    }

    Ok(
        itsanas_store::chunks_for_all(&node.store, &node.vault, &absent)?
            .into_iter()
            .filter(|chunk| !node.store.has_chunk(chunk))
            .count(),
    )
}

fn coverage_report(node: &Node) -> Result<String> {
    let mut out = String::new();
    macro_rules! w {
        () => {{ let _ = writeln!(out); }};
        ($($arg:tt)*) => {{ let _ = writeln!(out, $($arg)*); }};
    }

    // What this question can and cannot cover, said before the answer rather
    // than after it.
    //
    // `Store::coverage` walks chunks with a live local reference -- that is,
    // the ones this machine holds. On a machine that holds its whole account
    // that is the account. On one that has released content it is a slice, and
    // the reassuring line underneath was being computed over the slice while
    // reading as though it covered everything. Worse: the released chunks are
    // exactly the ones this machine can no longer audit, because a challenge is
    // verified against a local copy.
    //
    // So the count comes first, and the headline changes with it.
    let unspoken = chunks_not_here(node)?;
    if unspoken > 0 {
        w!("could you get back what is ON THIS MACHINE, without this machine?");
    } else {
        w!("could you get it all back without this machine?");
    }

    // The headline is a minimum, not an average, and it does not count this
    // machine. A file comes back only if every one of its chunks does, so an
    // account with almost everything on three machines and one chunk on none
    // has no complete copy at all -- and an average would report that as
    // "nearly three" and read as comfortable.
    let coverage = node.store.coverage(itsanas_discover::now_unix())?;
    if coverage.live_chunks == 0 {
        w!("  nothing stored yet");
    } else {
        match coverage.complete_elsewhere {
            0 => w!(
                concat!(
                    "  NO             not one complete copy exists anywhere ",
                    "else; {} of {} chunks are only here"
                ),
                coverage.only_here,
                coverage.live_chunks
            ),
            1 => w!(concat!(
                "  one copy       every chunk is on one other machine, so ",
                "the network could rebuild all of it once"
            )),
            copies => {
                // Not "any N machines could rebuild it". They could not: N
                // machines chosen at random may hold overlapping subsets and
                // nothing else. What is true is that every chunk is on at least
                // N of them, so the set survives losing any N-1 holders of any
                // chunk -- and rebuilding draws from many peers, not from one.
                w!(
                    concat!(
                        "  {} copies      every chunk is on at least {} other ",
                        "machines; rebuilding draws on all of them"
                    ),
                    copies,
                    copies
                );
            }
        }

        if unspoken > 0 {
            w!(
                concat!(
                    "  not covered    {} more chunks belong to this account and ",
                    "are not on this machine"
                ),
                unspoken
            );
            w!("                 nothing above speaks for them, and this node cannot");
            w!("                 audit them: a challenge is checked against a local copy");
        }

        if !coverage.meets(SAFE_COPIES) {
            w!(
                concat!(
                    "  the promise    {} complete copies is what this is for; ",
                    "you have {}"
                ),
                SAFE_COPIES,
                coverage.complete_elsewhere
            );
            w!("                 run `itsanas sync`, or add a peer, to spread it");
        }

        // How much of the reassurance above is memory rather than observation.
        // A holder record says a device once acknowledged a chunk; it says
        // nothing about whether that device still exists. This fleet had a
        // destroyed machine listed as a holder until somebody read a log.
        if coverage.resting_on_memory() {
            w!(
                concat!(
                    "  unconfirmed    the ledger remembers {} copies; {} holder ",
                    "records have gone quiet and are not counted above"
                ),
                coverage.claimed_elsewhere,
                coverage.stale_records
            );
        }

        let _ = write!(out, "{}", concentration_report(node, &coverage)?);
    }

    Ok(out)
}

/// What this node is costing the disk, in the three parts a person can act on.
///
/// # Why this is measured and not implied
///
/// `keep` bounds one of these numbers and reads as though it bounds the disk.
/// On the trial device it did not, and by a factor of twenty: told to keep 200
/// KiB, the node's directory held 4.3 MiB — 907 KiB of content, and the rest
/// index and vault. Somebody deciding whether this fits on a phone needs the
/// total, and the breakdown to know which setting moves it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DiskUse {
    /// This account's own content. The number `keep` bounds.
    mine: u64,
    /// Sealed data held for other people, and this account's own log relayed
    /// between its devices. The number `pledge` bounds.
    vault: u64,
    /// The databases: the index and the vault's own store.
    ///
    /// Bounded by nothing, and proportional to the number of files and log
    /// entries rather than to their size. On a nearly empty account it is most
    /// of the total.
    indexes: u64,
}

impl DiskUse {
    const fn total(self) -> u64 {
        self.mine
            .saturating_add(self.vault)
            .saturating_add(self.indexes)
    }
}

/// Measure it.
///
/// Only the files sitting directly in the store and vault directories are
/// stat'ed — the databases. Content is already counted, and walking a blob
/// store of a million files to add up what the index already knows would make
/// `status` cost a minute on the machines that most need it.
fn disk_use(node: &Node) -> Result<DiskUse> {
    fn databases(root: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(root) else {
            return 0;
        };
        entries
            .flatten()
            .filter_map(|entry| entry.metadata().ok())
            .filter(std::fs::Metadata::is_file)
            .map(|metadata| metadata.len())
            .sum()
    }

    Ok(DiskUse {
        mine: node.store.stats()?.bytes_on_disk,
        vault: node.vault.stats()?.bytes,
        indexes: databases(node.store.root()) + databases(&node.home.join("vault")),
    })
}

/// The text `itsanas status` prints, built rather than printed.
///
/// Separated from the command so the daemon can write the same text to a
/// snapshot after every round. The store allows one writer, so with the daemon
/// up `status` cannot open it -- and answering "the node is busy" to somebody
/// asking what their node is doing is the least useful thing this program could
/// say.
fn render_status(node: &Node) -> Result<String> {
    let mut out = String::new();

    // A local macro, so the body below reads the way it did when it printed.
    // Defined after `out` because a macro_rules body resolves names where it is
    // written, and expanding to a block rather than a `let` statement because
    // half these calls sit in match arms, where a statement is not an
    // expression. Writing to a String cannot fail, which is why the result is
    // dropped rather than propagated.
    macro_rules! w {
        () => {{ let _ = writeln!(out); }};
        ($($arg:tt)*) => {{ let _ = writeln!(out, $($arg)*); }};
    }
    let store = node.store.stats()?;
    let vault = node.vault.stats()?;

    w!("account");
    w!("  username        {}", node.config.username);
    w!("  user id         {}", node.store.owner());
    w!("  device          {}", node.store.device_id());
    w!("  home            {}", node.home.display());
    match &node.config.folder {
        Some(folder) => w!("{}", describe_folder(folder)),
        None => w!("  synced folder   none (`itsanas folder <path>`)"),
    }
    w!();
    w!("your data");
    w!("  files           {}", store.files);
    w!("  live chunks     {}", store.live_chunks);
    w!("  on disk         {}", format_size(store.bytes_on_disk));
    match node.config.keep_bytes {
        None => w!("  keeping         all of it here"),
        Some(keep) if store.bytes_on_disk >= keep => w!(
            concat!(
                "  keeping         at most {} here, which is reached; new content ",
                "stays listed and is fetched when opened"
            ),
            format_size(keep)
        ),
        Some(keep) => w!(
            "  keeping         at most {} here ({} to go)",
            format_size(keep),
            format_size(keep.saturating_sub(store.bytes_on_disk))
        ),
    }
    w!("  log segments    {}", store.segments);
    if store.unsealed_entries > 0 {
        w!(
            "  unannounced     {} (run `itsanas sync` to publish)",
            store.unsealed_entries
        );
    }
    if store.pending_collection > 0 {
        w!("  awaiting gc     {} chunks", store.pending_collection);
    }

    // The question a backup tool exists to answer, and the one it is easiest
    // to leave unanswered: does this data exist anywhere other than this disk?
    // A count of files says nothing about that.
    w!();
    let _ = write!(out, "{}", coverage_report(node)?);
    w!("  placements     {} recorded", store.holder_records);

    report_unreliable_peers(node)?;
    // The vault holds two different things. Reporting them as one number
    // tells the operator they are hosting for a stranger when they are only
    // relaying their own account between their own machines.
    let own_in_vault = node.vault.stats_for(node.store.owner())?;
    let hosted_owners = vault
        .owners
        .saturating_sub(usize::from(own_in_vault.segments > 0));
    let hosted_bytes = vault.bytes.saturating_sub(own_in_vault.bytes);
    let hosted_chunks = vault.chunks.saturating_sub(own_in_vault.chunks);

    w!();
    let disk = disk_use(node)?;
    w!("disk used by this node");
    w!("  your content    {}", format_size(disk.mine));
    w!("  vault           {}", format_size(disk.vault));
    w!("  indexes         {}", format_size(disk.indexes));
    w!("  total           {}", format_size(disk.total()));
    if node.config.keep_bytes.is_some() {
        w!("  `keep` bounds the first line only; the rest is index and vault.");
    }

    w!();
    w!("hosting for other people");
    w!(
        "  pledged         {}",
        format_size(node.config.pledge_bytes)
    );
    w!("  used            {}", format_size(hosted_bytes));
    w!("  peers hosted    {hosted_owners}");
    w!("  chunks held     {hosted_chunks}");
    w!(
        "  segments held   {}",
        vault.segments.saturating_sub(own_in_vault.segments)
    );
    w!();
    w!("relaying for your own devices");
    w!(
        "  segments held   {} (so this machine can pass your other devices' \
         work along)",
        own_in_vault.segments
    );
    w!();
    w!("network");
    w!("  listen          {}", node.config.listen);
    // What other machines are told, which is the number that decides whether
    // anybody outside this LAN can reach this one -- and which differs from
    // `listen` exactly when somebody has set it up to.
    w!("  announced       {}", describe_announce(node));
    if node.config.peers.is_empty() {
        w!("  peers           none configured (`itsanas peer add <host:port>`)");
    } else {
        for peer in &node.config.peers {
            w!("  peer            {peer}");
        }
    }

    Ok(out)
}

/// How long ago, in words, for a reader who wants to know whether to trust it.
///
/// Rounded down and deliberately coarse: the question this answers is "is this
/// current enough to act on", and a snapshot four minutes old and one four
/// minutes and fifty seconds old have the same answer.
fn describe_age(seconds: u64) -> String {
    match seconds {
        0..=5 => "just now".to_owned(),
        6..=89 => format!("{seconds} seconds ago"),
        90..=5399 => format!("{} minutes ago", seconds / 60),
        5400..=86_399 => format!("{} hours ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

/// What the daemon last reported, whether or not it is still running.
///
/// Takes no passphrase, and that is the point: the snapshot is a plain file in
/// the node home, so printing it needs no key. Returning the text rather than
/// printing it is what makes the age line testable. `running` decides the
/// first line only — a snapshot from a stopped node must not read as a live
/// one.
fn snapshot_status(home: &Path, running: bool) -> Result<String> {
    let text = std::fs::read_to_string(home.join(SNAPSHOT)).map_err(|_| {
        CliError::Usage(
            "this node is running and has not written a snapshot yet. Wait for its first sync round, or stop it and ask again."
                .to_owned(),
        )
    })?;

    let (stamp, body) = text.split_once('\n').unwrap_or(("", text.as_str()));
    let taken = stamp
        .strip_prefix("snapshot ")
        .and_then(|seconds| seconds.trim().parse::<u64>().ok());
    // The two cases are not the same claim and must not read as one. A daemon
    // holding the store means the snapshot is a recent report of a live node;
    // no daemon means it is the last thing a stopped node said, which may be
    // from last week.
    let subject = if running {
        "this node is running"
    } else {
        "nothing is running this node"
    };
    let header = match taken {
        Some(taken) => format!(
            "{subject}, so what follows is what it reported {}.",
            describe_age(itsanas_discover::now_unix().saturating_sub(taken))
        ),
        // A snapshot whose first line is not a stamp is one this version did
        // not write. Print it rather than refuse, and do not put an age on it
        // that was never measured.
        None => format!("{subject}; the snapshot it left has no time on it."),
    };

    Ok(format!("{header}\n\n{body}"))
}

/// Whether a passphrase could be obtained without failing.
///
/// Mirrors the two ways [`passphrase`] can succeed: the environment variable,
/// or a terminal to prompt on. Asked before opening a node so that a command
/// which has a usable answer without keys can give it, instead of failing with
/// advice about environment variables to somebody who only wanted to know
/// whether their files are safe.
fn passphrase_available() -> bool {
    std::env::var(PASSPHRASE_ENV).is_ok() || std::io::stdin().is_terminal()
}

fn status(home: &Path) -> Result<()> {
    // Ask whether the daemon holds the store *before* asking anybody for a
    // passphrase. `open` resolves the passphrase first, so this path -- whose
    // entire purpose is to answer while the daemon is running, which is the
    // normal state of a working machine -- was unreachable exactly when it
    // applied: the command demanded the keystore secret and would then have
    // printed a plaintext file. The prompt guarded nothing a `cat` of the
    // snapshot would not bypass, and it made "is my node healthy?" a question
    // you could not ask without unsealing your keys.
    if Node::exists(home) && itsanas_store::Store::is_locked(Node::store_path(home)) {
        print!("{}", snapshot_status(home, true)?);
        return Ok(());
    }

    // No daemon, and no way to ask for a passphrase: a live read is impossible,
    // so the snapshot is strictly better than an error about environment
    // variables. This is the state a machine is in for the whole window
    // between installing and starting the daemon -- which is exactly when
    // somebody asks whether the thing works.
    if Node::exists(home) && !passphrase_available() {
        if home.join(SNAPSHOT).exists() {
            print!("{}", snapshot_status(home, false)?);
            return Ok(());
        }
        // A node that exists and has never finished a sync round. Answering
        // with advice about environment variables tells somebody who just
        // installed this that they have done something wrong, when the real
        // answer is that nothing has run yet and they should start it.
        return Err(CliError::Usage(format!(
            "this node has never finished a sync round, so it has nothing to \
             report yet. Start it with `itsanas daemon`, or set {PASSPHRASE_ENV} \
             to open the node and read its state directly."
        )));
    }

    match open(home) {
        Ok(node) => {
            print!("{}", render_status(&node)?);
            Ok(())
        }
        // Still handled, because the daemon can take the lock between the probe
        // above and this open -- while the passphrase is being typed, which is
        // exactly when it is slowest. Rare, and the original bug if it were
        // dropped.
        Err(CliError::Store(itsanas_store::StoreError::Locked(_))) => {
            print!("{}", snapshot_status(home, true)?);
            Ok(())
        }
        Err(other) => Err(other),
    }
}

/// Restore an account from a coordinator, using a passphrase alone.
///
/// The machine has nothing: no device key, no account, no store. It fetches the
/// sealed container by name, opens it with the passphrase, and writes a local
/// keystore from what was inside — with a **new device key**, because the
/// container carries the account's identity and this is a different machine.
fn login_from_coordinator(
    home: &Path,
    username: &str,
    address: &str,
    device: Option<&str>,
) -> Result<()> {
    let expect = device.map(coordinator::parse_device).transpose()?;

    println!("Recovering {username:?} from {address}.");
    println!("This needs the passphrase the container was sealed with, which is");
    println!("the passphrase of whichever machine lodged it — not necessarily one");
    println!("you have used on this machine.");
    let secret = passphrase(false)?;

    let secrets = coordinator::fetch_escrow(address, expect, username, &secret)?;
    let mut node = Node::restore_from_secrets(home, &secret, username, &secrets)?;

    // The coordinator that just proved it holds this account is the one to
    // keep. This used to be forgotten: the message below told the reader to
    // run `itsanas register`, which then failed for want of a coordinator, and
    // `sync` found only machines on the same network -- so a recovery on a
    // network away from the others restored an identity and nothing else.
    node.config.coordinator = Some(address.to_owned());
    node.config.coordinator_device = device.map(str::to_owned);
    node.save_config()?;
    settle_listen_port(&mut node)?;

    println!();
    println!("Account restored.");
    println!("  user id     : {}", node.store.owner());
    println!(
        "  device      : {} (new for this machine)",
        node.store.device_id()
    );
    println!("  coordinator : {address} (kept for this machine)");
    println!();
    println!("Your 24-word phrase is unchanged and still the ultimate backup:");
    println!("this recovered the same identity, it did not create a new one.");
    println!();
    println!("Nothing has been downloaded yet. Next, on this machine:");
    println!("  itsanas register           enrol it, so your other machines find it");
    println!("  itsanas pledge <size>      a machine that pledges nothing relays nothing");
    println!("  itsanas folder <directory>");
    println!("  itsanas daemon");

    Ok(())
}

/// Set, show, or forget the coordinator this node uses.
fn coordinator_setting(
    home: &Path,
    address: Option<&str>,
    device: Option<&str>,
    forget: bool,
) -> Result<()> {
    let mut config = config::Config::load(&Node::config_path(home))?;

    if forget {
        config.coordinator = None;
        config.coordinator_device = None;
        config.save(&Node::config_path(home))?;
        println!("no coordinator configured. Machines on this network still find");
        println!("each other; machines elsewhere now need `itsanas peer add`.");
        return Ok(());
    }

    if let Some(address) = address {
        if let Some(device) = device {
            // Parsed now rather than at first use, so a mistyped id fails while
            // the person who typed it is still looking at it.
            coordinator::parse_device(device)?;
        }
        config.coordinator = Some(address.to_owned());
        config.coordinator_device = device.map(str::to_owned);
        config.save(&Node::config_path(home))?;
        println!("coordinator set to {address}");
        if let Some(device) = device {
            println!("  pinned to device {device}");
        } else {
            println!("  not pinned. Anything answering at that address is trusted to");
            println!("  be the coordinator. Pin it with --device <id> from");
            println!("  `itsanas-coordinator --identity`.");
        }
        return Ok(());
    }

    if let Some(address) = &config.coordinator {
        println!("{address}");
        match &config.coordinator_device {
            Some(device) => println!("  pinned to device {device}"),
            None => println!("  not pinned"),
        }
    } else {
        println!("no coordinator configured");
    }
    Ok(())
}

/// Draw an invitation and print it once.
fn invite(home: &Path, uses: u32, days: u64) -> Result<()> {
    let node = open(home)?;
    let validity = days.saturating_mul(24 * 60 * 60);
    let secret = coordinator::invite(&node, uses, validity, itsanas_discover::now_unix())?;

    println!("invitation code");
    println!();
    println!("  {}", coordinator::encode_secret(&secret));
    println!();
    println!("Send it to whoever is joining. They run:");
    println!();
    println!("  itsanas init --username <their-name>");
    println!("  itsanas coordinator {}", coordinator_address(&node));
    println!("  itsanas register --invite <the code above>");
    println!();
    if uses == 1 {
        println!("Good for one account, for {days} day(s).");
    } else {
        println!("Good for {uses} accounts, for {days} day(s).");
    }
    println!("It is not stored anywhere. Lose it and issue another.");
    Ok(())
}

/// What to tell an invitee to point at.
fn coordinator_address(node: &Node) -> String {
    node.config
        .coordinator
        .clone()
        .unwrap_or_else(|| "<host:port>".to_owned())
}

/// Register this account and device, and optionally lodge a recovery container.
fn register(home: &Path, recovery: bool, withdraw: bool, invite: Option<&str>) -> Result<()> {
    let node = open(home)?;
    let now = itsanas_discover::now_unix();

    let secret = invite.map(coordinator::decode_secret).transpose()?;
    coordinator::register_with(&node, secret.as_ref(), now)?;
    println!(
        "registered {:?} and enrolled this device",
        node.config.username
    );

    // Publishing the address is part of registering, not a separate step: a
    // device nobody can reach has not really joined anything.
    let listen = node.config.listen.clone();
    match coordinator::announce(&node, &listen, now) {
        // What was published, not what was configured. With `listen` set to
        // every interface — the default — those differ, and printing the
        // configured value told the reader an address no peer can dial.
        Ok(published) => println!("announced {published}"),
        Err(error) => println!("could not announce an address: {error}"),
    }

    if withdraw {
        coordinator::set_escrow(&node, None, &[])?;
        println!("recovery container withdrawn. This account can now only be");
        println!("restored with its 24-word phrase.");
        return Ok(());
    }

    if recovery {
        println!();
        println!("Lodging a recovery container. Enter this machine's passphrase again:");
        let secret = passphrase(false)?;
        coordinator::set_escrow(&node, Some(&secret), &node.secrets)?;
        println!("recovery container lodged.");
        println!(
            "  A new machine can now run `itsanas login --username {} \\",
            node.config.username
        );
        println!(
            "    --from {}`",
            node.config
                .coordinator
                .as_deref()
                .unwrap_or("<coordinator>")
        );
        println!("  Anybody who steals the coordinator's database can attack that");
        println!("  passphrase offline. Withdraw it with `itsanas register --withdraw-recovery`.");
    }

    Ok(())
}

fn whoami(home: &Path) -> Result<()> {
    let node = open(home)?;
    println!("{}", node.store.owner());
    Ok(())
}

fn list(home: &Path) -> Result<()> {
    let node = open(home)?;

    // Everything this account has, not everything this machine downloaded. A
    // node that synced on a metered connection knows about files whose contents
    // it never fetched, and listing only what is local would tell somebody
    // their files were gone.
    let known = itsanas_store::catalogue(&node.store, &node.vault)?;

    if known.files.is_empty() {
        println!("(no files)");
        return Ok(());
    }

    let mut absent = 0usize;
    for entry in &known.files {
        match entry.presence {
            itsanas_store::Presence::Local => {
                println!("{:>12}            {}", format_size(entry.size), entry.path);
            }
            itsanas_store::Presence::Absent => {
                absent += 1;
                println!("{:>12}  not here  {}", format_size(entry.size), entry.path);
            }
        }
    }

    if absent > 0 {
        println!();
        // What is actually true, which is not the same as what this said in
        // either of its two earlier versions. On a device with a `keep` limit,
        // sync fetches what the limit and the order choose and lets go of the
        // rest, so "run sync" is not advice that brings a particular file down.
        // Opening one always works, because an explicit request beats a
        // background choice.
        println!("{absent} file(s) are known and not on this device.");
        println!("  `itsanas get <path>` fetches one, whatever the limit.");
        if node.config.keep_bytes.is_some() || !node.config.keep_only.is_empty() {
            println!("  `itsanas sync` fetches whatever this device's limit chooses to hold.");
            println!("  `itsanas keep` shows and changes that choice.");
        } else {
            println!("  `itsanas sync` fetches them.");
        }
    }

    Ok(())
}

fn put(home: &Path, path: &str, source: &std::path::Path) -> Result<()> {
    let node = open(home)?;

    let content = if source == std::path::Path::new("-") {
        let mut buffer = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buffer)
            .map_err(|error| CliError::Io {
                path: PathBuf::from("<stdin>"),
                source: error,
            })?;
        buffer
    } else {
        std::fs::read(source).map_err(|error| CliError::Io {
            path: source.to_owned(),
            source: error,
        })?
    };

    let entry = node.store.write_file(path, &content)?;
    node.store.flush_segment()?;

    println!(
        "stored {} as {path} ({} chunks)",
        format_size(entry.size),
        entry.chunks.len()
    );
    Ok(())
}

/// Go and get a file this device knows about and has not downloaded.
///
/// Fetches exactly that file's chunks from the first peer that has them, rather
/// than syncing the account: opening one document on a phone must not pull
/// somebody's photo library.
fn fetch_absent(node: &Node, path: &str) -> Result<Vec<u8>> {
    let Some(chunks) = itsanas_store::chunks_for(&node.store, &node.vault, path)? else {
        return Err(CliError::Usage(format!("no such file: {path}")));
    };

    let wanted: std::collections::BTreeSet<_> = chunks.into_iter().collect();

    if node.config.peers.is_empty() {
        return Err(CliError::Usage(format!(
            "{path} is in this account and not on this device, and there is no peer to fetch it from. Try `itsanas peer add <host:port>` or `itsanas peer find <username>`."
        )));
    }

    for target in &node.config.peers {
        let Ok(mut client) =
            PeerClient::connect(target.as_str(), &node.device, node.store.owner(), None)
        else {
            continue;
        };

        if session::fetch_only(&node.store, &node.vault, &mut client, &wanted).is_err() {
            continue;
        }

        if let Some(content) = node.store.read_file(path)? {
            println!("fetched {path} from {target}");
            return Ok(content);
        }
    }

    Err(CliError::Usage(format!(
        "{path} is in this account and no reachable peer would serve it. It is still listed; try again when one is up."
    )))
}

fn get(home: &Path, path: &str, destination: Option<&std::path::Path>) -> Result<()> {
    let node = open(home)?;

    let content = match node.store.read_file(path)? {
        Some(content) => content,
        // Not here does not mean not yours. A device with a storage budget, or
        // one that synced over a metered link, knows about files it has not
        // downloaded -- `itsanas ls` shows them. This used to answer "no such
        // file" for a file the account plainly had, which was both a lie and
        // the reason the budget setting had nothing behind it.
        None => fetch_absent(&node, path)?,
    };

    match destination {
        Some(destination) => {
            std::fs::write(destination, &content).map_err(|error| CliError::Io {
                path: destination.to_owned(),
                source: error,
            })?;
            println!(
                "wrote {} to {}",
                format_size(content.len() as u64),
                destination.display()
            );
        }
        None => {
            std::io::stdout()
                .write_all(&content)
                .map_err(|error| CliError::Io {
                    path: PathBuf::from("<stdout>"),
                    source: error,
                })?;
        }
    }

    Ok(())
}

fn remove(home: &Path, path: &str) -> Result<()> {
    let node = open(home)?;

    if node.store.remove_file(path)? {
        node.store.flush_segment()?;
        println!("deleted {path}");
    } else {
        println!("no such file: {path}");
    }

    Ok(())
}

fn folder(home: &Path, path: Option<&Path>, confirm: bool) -> Result<()> {
    let mut node = open(home)?;

    let Some(path) = path else {
        let Some(configured) = node.config.folder.clone() else {
            println!("(no folder configured — `itsanas folder <path>`)");
            return Ok(());
        };
        println!("{}", configured.display());

        if confirm {
            // Only here, and only because somebody typed it: this is the one
            // path that writes deletions a pass refused to write on its own.
            let folder = itsanas_folder::Folder::open(&configured)?;
            let report = folder.reconcile_confirmed(&node.store, false)?;
            println!();
            println!("{}", report.summary());
            if report.removed_from_store.is_empty() {
                println!("  nothing was waiting to be deleted.");
            } else {
                println!(
                    "  {} file(s) removed from the account, as confirmed.",
                    report.removed_from_store.len()
                );
            }
        }
        return Ok(());
    };

    // Store it absolute. A relative path would mean something different
    // depending on where the daemon happened to be started from, which is the
    // sort of thing that quietly syncs the wrong directory.
    let absolute = std::path::absolute(path).map_err(|error| CliError::Io {
        path: path.to_owned(),
        source: error,
    })?;

    let folder = itsanas_folder::Folder::open(&absolute)?;

    node.config.folder = Some(absolute.clone());
    node.save_config()?;

    println!("synced folder set to {}", absolute.display());

    // Show what the first pass would do rather than doing it silently. Pointing
    // this at an existing directory full of files is a big action, and the user
    // should see the size of it.
    let report = folder.reconcile(&node.store, false)?;
    if report.changed_anything() {
        println!("first pass: {}", report.summary());
    } else {
        println!("the folder and the store already agree.");
    }

    Ok(())
}

fn device(home: &Path, what: &DeviceCommand) -> Result<()> {
    let node = open(home)?;
    let mine = node.store.device_id();

    // Every enrolled device where the coordinator can say so, and the
    // reachable ones where it is too old to. The reachable list leaves out a
    // machine silent for a week, which is the lost laptop somebody came here
    // to find, so falling back is said out loud rather than done quietly.
    let enrolled = coordinator::enrolled(&node)?;
    let listed: Vec<(DeviceId, String)> = if let Some(list) = &enrolled {
        list.iter()
            .map(|entry| (entry.device, entry.address.clone().unwrap_or_default()))
            .collect()
    } else {
        println!(
            "this coordinator cannot list devices that have gone quiet (it is older than this \
             client, or the connection dropped); showing only those seen in the last week."
        );
        coordinator::devices(&node, node.store.owner())?
    };

    match what {
        DeviceCommand::List => {
            if listed.is_empty() {
                println!("the coordinator lists no devices for this account");
                return Ok(());
            }
            match &enrolled {
                Some(list) => {
                    for entry in list {
                        println!("{}", describe_enrolled(entry, entry.device == mine));
                    }
                    // The coordinator stops at this many. Saying so is the
                    // difference between "the machine is not enrolled" and
                    // "the machine is past the end of the list".
                    if list.len() >= itsanas_coord::protocol::MAX_PEERS_RETURNED {
                        println!(
                            "(the coordinator lists at most {} devices; there may be more, silent longest)",
                            itsanas_coord::protocol::MAX_PEERS_RETURNED
                        );
                    }
                }
                None => {
                    for (device, address) in &listed {
                        let here = if *device == mine {
                            "  (this machine)"
                        } else {
                            ""
                        };
                        println!("{device}  {address}{here}");
                    }
                }
            }
            Ok(())
        }

        DeviceCommand::Forget { device } => {
            let wanted = resolve_device(device, &listed)?;

            if wanted == mine {
                return Err(CliError::Usage(
                    "that is this machine. Withdrawing it from here would leave a node running and unlisted; run this from another device of the account.".to_owned(),
                ));
            }

            coordinator::forget_device(&node, wanted, itsanas_discover::now_unix())?;
            println!("withdrew {wanted}");
            println!("  Nothing will dial it through the coordinator again, and the");
            println!("  withdrawal is final for that device id. To use that machine");
            println!("  again, remove its node directory and `itsanas login` on it:");
            println!("  it comes back as a new device.");
            println!("  A thief who also has its passphrase holds the account's master");
            println!("  key and can read what it stores; withdrawing cannot undo that.");
            Ok(())
        }
    }
}

/// What `status` says this node publishes.
fn describe_announce(node: &Node) -> String {
    node.config
        .announce
        .clone()
        .unwrap_or_else(|| "this machine's address on the network it is on".to_owned())
}

/// The synced folder's line in `status`, and the warning when it is not there.
///
/// A folder whose marker is gone is either a disk that is not mounted or a
/// directory somebody emptied. Both stop syncing, and neither says so anywhere
/// else, which is how a disk stays unmounted for a week.
fn describe_folder(folder: &std::path::Path) -> String {
    let mut out = format!("  synced folder   {}", folder.display());
    if !folder.join(itsanas_folder::scan::MARKER).exists() {
        out.push_str(
            "
                  STORAGE UNREACHABLE: no marker there.",
        );
        out.push_str(
            "
                  A disk or share that is not mounted leaves an empty",
        );
        out.push_str(
            "
                  directory behind. Nothing has been deleted.",
        );
    }
    out
}

/// One line of `itsanas device list`.
///
/// The silence is the coordinator's own measurement, so it is what decides
/// whether a device is "lost"; this machine's clock never enters it.
fn describe_enrolled(entry: &itsanas_coord::protocol::EnrolledDevice, here: bool) -> String {
    let heard = match entry.silent_for {
        Some(seconds) => format!("heard from {}", describe_age(seconds)),
        None => "never announced".to_owned(),
    };
    let address = entry.address.as_deref().unwrap_or("no address");
    let this = if here { "  (this machine)" } else { "" };
    format!(
        "{}  {address}  pledges {}  {heard}{this}",
        entry.device,
        config::format_size(entry.pledged_bytes)
    )
}

/// Turn what somebody typed into a device on this account.
///
/// Accepts the full identifier, and the short form the logs print. A short form
/// is matched against the account's own devices rather than parsed, so a string
/// that matches nothing says so -- instead of becoming an identifier for a
/// device that does not exist, which the coordinator would accept and file a
/// revocation against, silently, for ever.
fn resolve_device(typed: &str, listed: &[(DeviceId, String)]) -> Result<DeviceId> {
    if let Ok(parsed) = typed.parse::<DeviceId>() {
        return Ok(parsed);
    }

    let matches: Vec<DeviceId> = listed
        .iter()
        .map(|(device, _)| *device)
        .filter(|device| device.to_string().starts_with(typed))
        .collect();

    match matches.as_slice() {
        [one] => Ok(*one),
        [] => Err(CliError::Usage(format!(
            "no device on this account starts with {typed:?}. `itsanas device list` shows them."
        ))),
        several => Err(CliError::Usage(format!(
            "{} devices start with {typed:?}; give more of it.",
            several.len()
        ))),
    }
}

/// The first port a new node may serve on: from 9797, and no further than this.
const PORT_SEARCH: std::ops::Range<u16> = 9797..9897;

/// Ports the other nodes on this machine are configured to serve on.
///
/// A sibling is any directory beside `home` holding a keystore, which is what
/// "there is a node here" means everywhere else in this program. Asking the
/// kernel alone is not enough: a node whose daemon is stopped holds no socket,
/// so a second account created while the first is down would be handed the
/// same port and the two daemons would fight over it at the next boot.
fn sibling_ports(home: &Path) -> std::collections::BTreeSet<u16> {
    let mut taken = std::collections::BTreeSet::new();
    let Some(parent) = home.parent() else {
        return taken;
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return taken;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == home || !path.join("keystore.bin").is_file() {
            continue;
        }
        if let Ok(other) = config::Config::load(&Node::config_path(&path))
            && let Ok(address) = crate::config::parse_listen(&other.listen)
        {
            taken.insert(address.port());
        }
    }
    taken
}

/// The first port in [`PORT_SEARCH`] nobody claims and this machine can bind.
/// Why this node did not get the port it asked for.
///
/// The second account on a machine skipped 9797 *and* 9798 and was told only
/// that "9797 is used by another node", which reads as one sibling when there
/// are two. Somebody counting their instances from this line counts wrong, and
/// the line exists precisely because ports here are allocated behind the
/// person's back.
fn ports_skipped(chosen: u16) -> String {
    match chosen.saturating_sub(PORT_SEARCH.start) {
        0 => "chosen for this node".to_owned(),
        1 => format!(
            "{} is used by another node on this machine",
            PORT_SEARCH.start
        ),
        _ => format!(
            "{}-{} are used by other nodes on this machine",
            PORT_SEARCH.start,
            chosen - 1
        ),
    }
}

fn first_free_port(
    taken: &std::collections::BTreeSet<u16>,
    bindable: impl Fn(u16) -> bool,
) -> Option<u16> {
    PORT_SEARCH
        .clone()
        .find(|port| !taken.contains(port) && bindable(*port))
}

/// Give a new node a port no other node on this machine serves on.
///
/// Two accounts on one machine are two daemons, and every node used to be
/// created listening on 9797: the second daemon failed to bind, exited, and
/// under systemd restarted every thirty seconds with the reason in a journal.
/// Only the default is moved -- an address somebody chose with `itsanas listen`
/// is left alone.
fn settle_listen_port(node: &mut Node) -> Result<()> {
    let Ok(current) = crate::config::parse_listen(&node.config.listen) else {
        return Ok(());
    };
    if !current.ip().is_unspecified() {
        return Ok(());
    }
    let taken = sibling_ports(&node.home);
    let bindable = |port: u16| std::net::TcpListener::bind(("0.0.0.0", port)).is_ok();
    if !taken.contains(&current.port()) && bindable(current.port()) {
        return Ok(());
    }
    match first_free_port(&taken, bindable) {
        Some(port) => {
            let chosen = SocketAddr::new(current.ip(), port);
            node.config.listen = chosen.to_string();
            node.save_config()?;
            println!("  listen   : {chosen} ({})", ports_skipped(port));
        }
        None => println!(
            "warning: every port from {} to {} is taken here; choose one with `itsanas listen`",
            PORT_SEARCH.start,
            PORT_SEARCH.end - 1
        ),
    }
    Ok(())
}

/// The line a round prints when a peer refused what it was offered, if it did.
///
/// Without it a host refusing everything produced the line an idle round
/// produces, `sent 0 B`, and the owner believed nothing was pending.
pub(crate) fn describe_refusal(push: &itsanas_net::PushReport) -> Option<String> {
    let why = match push.refusal? {
        itsanas_net::Refusal::PledgeFull => {
            "its pledge is full or zero, so it hosts nothing more; on that machine, `itsanas pledge <size>`"
        }
        // Not "its log says why": a peer logs nothing about what it refuses,
        // and the first version of this line sent people to read a journal
        // that had nothing in it.
        itsanas_net::Refusal::Rejected => {
            "it rejected them: a segment that does not verify, or does not follow its chain"
        }
    };
    Some(format!("refused {} offer(s): {why}", push.refused))
}

fn listen_on(home: &Path, address: Option<&str>) -> Result<()> {
    let mut node = open(home)?;

    let Some(address) = address else {
        println!("{}", node.config.listen);
        return Ok(());
    };

    // Parsed, not merely stored. An address that does not parse is not found
    // out here but at `serve`, which on a machine running the daemon under
    // systemd means a unit that restarts every thirty seconds with the reason
    // in a journal nobody is reading.
    let parsed: SocketAddr = crate::config::parse_listen(address)?;

    let previous = std::mem::replace(&mut node.config.listen, parsed.to_string());
    node.save_config()?;

    println!("serving on {parsed} from now on (was {previous})");

    // The coordinator is still handing out the old one until it is told. Say
    // so, rather than leaving a node that is reachable and unreachable at the
    // same time depending on who you ask.
    if node.config.coordinator.is_some() {
        println!("  the coordinator still publishes the old address; refresh it with:");
        println!("    itsanas register");
    }

    Ok(())
}

fn announce_as(home: &Path, address: Option<&str>, forget: bool) -> Result<()> {
    let mut node = open(home)?;

    if forget {
        match node.config.announce.take() {
            Some(previous) => {
                node.save_config()?;
                println!("no longer announcing {previous}");
                println!("  this node now publishes the address it reaches the");
                println!("  coordinator from, which only its own network can dial");
            }
            None => println!("nothing was being announced"),
        }
        refresh_hint(&node);
        return Ok(());
    }

    let Some(address) = address else {
        match node.config.announce.as_deref() {
            Some(announce) => println!("{announce}"),
            None => println!(
                "nothing announced: this node publishes the address it reaches the coordinator from"
            ),
        }
        return Ok(());
    };

    // Validated here and not merely stored, for the same reason `listen` is:
    // a value that cannot work must fail in front of the person who typed it,
    // not in a daemon log after the next restart.
    let announce = crate::config::parse_announce(address)?;
    let previous = node.config.announce.replace(announce.clone());
    node.save_config()?;

    match previous {
        Some(previous) if previous != announce => {
            println!("announcing {announce} from now on (was {previous})");
        }
        _ => println!("announcing {announce} from now on"),
    }
    println!("  nothing checks that this address reaches this machine: that is");
    println!("  your router's forward, or your IPv6 firewall, and a wrong one");
    println!("  makes this node unreachable rather than noisy");

    refresh_hint(&node);
    Ok(())
}

/// The coordinator keeps handing out the previous address until it is told.
///
/// Said rather than done: refreshing needs the passphrase and the coordinator
/// to be up, and a command that half-worked silently is worse than one that
/// says what is left.
fn refresh_hint(node: &Node) {
    if node.config.coordinator.is_some() {
        println!("  the coordinator still publishes the old address; refresh it with:");
        println!("    itsanas register");
    }
}

fn scan(home: &Path, deep: bool) -> Result<()> {
    let node = open(home)?;

    let Some(path) = node.config.folder.clone() else {
        return Err(CliError::Usage(
            "no synced folder configured. Try `itsanas folder <path>`.".to_owned(),
        ));
    };

    let folder = itsanas_folder::Folder::open(&path)?;
    let report = folder.reconcile(&node.store, deep)?;

    println!("{}", report.summary());
    for path in &report.imported {
        println!("  in   {path}");
    }
    for path in &report.exported {
        println!("  out  {path}");
    }
    for path in &report.removed_from_store {
        println!("  del  {path} (deleted here, will be deleted everywhere)");
    }
    for path in &report.deleted_from_disk {
        println!("  rm   {path} (deleted elsewhere, removed from this folder)");
    }
    for (original, sibling) in &report.kept_both {
        println!("  !!   {original} conflicted — your version kept as {sibling}");
    }
    for (path, why) in &report.failed {
        eprintln!("  err  {path}: {why}");
    }

    Ok(())
}

fn keep(home: &Path, size: Option<&str>, order: Option<&str>, only: &[String]) -> Result<()> {
    let mut node = open(home)?;

    if size.is_none() && order.is_none() && only.is_empty() {
        report_keeping_settings(&node);
        return Ok(());
    }

    if let Some(order) = order {
        node.config.keep_order = crate::config::parse_order(order).ok_or_else(|| {
            CliError::Usage(format!(
                "unknown order {order:?}. Try newest, oldest or smallest."
            ))
        })?;
    }

    // Given at all, `--only` replaces the whole list rather than adding to it.
    // Appending would make the setting impossible to narrow without editing the
    // file by hand, and a filter that can only ever grow is one that quietly
    // stops filtering.
    if !only.is_empty() {
        node.config.keep_only = if only.iter().any(|prefix| prefix == "all") {
            Vec::new()
        } else {
            only.to_vec()
        };
    }

    if let Some(size) = size {
        if size.eq_ignore_ascii_case("all") || size.eq_ignore_ascii_case("none") {
            node.config.keep_bytes = None;
        } else {
            let bytes = parse_size(size)?;

            // The bargain, enforced where the number is typed.
            //
            // This network gives storage in proportion to storage provided, and
            // the ratio lives in `itsanas-coord`. Checking it only there would
            // mean somebody sets a limit, fills it over a fortnight, and is
            // then told it was never theirs to set -- with the data already on
            // the machine. Refused here, with the number that would make it
            // legal.
            let split = node.config.split;
            let allowed = split
                .room_earned(node.config.pledge_bytes)
                .max(itsanas_coord::accounting::JOINING_ALLOWANCE);
            if bytes > allowed {
                return Err(CliError::Usage(format!(
                    concat!(
                        "keeping {} needs {} pledged, and this node offers {}. ",
                        "`itsanas space --pledge {} --keep {} --apply` sets both, ",
                        "or ask for less."
                    ),
                    format_size(bytes),
                    itsanas_node::config::size_argument(split.pledge_needed_for(bytes)),
                    format_size(node.config.pledge_bytes),
                    itsanas_node::config::size_argument(split.pledge_needed_for(bytes)),
                    itsanas_node::config::size_argument(bytes),
                )));
            }

            node.config.keep_bytes = Some(bytes);
        }
    }

    node.save_config()?;
    report_keeping_settings(&node);

    // The consequence nobody would guess from the command they just typed.
    //
    // On a machine with a synced folder, letting go of content removes the file
    // from that folder: the store loses the entry, and the folder layer's next
    // pass sees content in its ledger and none in the store and deletes it from
    // disk. That is what a limit smaller than the account has to mean without a
    // placeholder filesystem, and it is what every selective-sync product did
    // before placeholders existed -- but it is a file disappearing from
    // somebody's Explorer window, and the fact that `itsanas get` brings it
    // back is knowledge only the author of this system has.
    //
    // `keep` and `folder` arm independently, so the destructive combination can
    // be reached without either command mentioning it. This is the mention.
    if node.config.folder.is_some() && node.config.keep_bytes.is_some() {
        println!();
        println!("  This machine syncs a folder, so a limit smaller than the account");
        println!("  will REMOVE files from it. They stay in the account and");
        println!("  `itsanas get <path>` brings one back, but they leave the folder.");
        println!("  `itsanas keep all` undoes the limit.");
    }

    // Said plainly rather than dressed up. A device over its limit comes back
    // down on the next sync, by letting go of what the order ranks lowest --
    // and only of content two other live machines are known to hold, at least
    // one of which has said so about that very chunk. Until then, the device
    // stays over its limit and says so.
    let held = node.store.stats()?.bytes_on_disk;
    if let Some(limit) = node.config.keep_bytes
        && held > limit
    {
        println!(
            "  {} is stored now, over that limit. The next sync lets go of what",
            format_size(held)
        );
        println!("  the order ranks lowest, keeping anything no other machine holds.");
    }

    Ok(())
}

/// Print what this device has been told to hold, in one place.
///
/// One function rather than a line at each call site, because the three
/// settings answer one question and a device that printed only the one just
/// changed would keep leaving out the one that explains the result.
fn report_keeping_settings(node: &Node) {
    match node.config.keep_bytes {
        Some(bytes) => println!(
            "keeping at most {} of your own data here",
            format_size(bytes)
        ),
        None => println!("keeping all of your own data here (`itsanas keep 2G` to limit it)"),
    }

    if node.config.keep_bytes.is_some() {
        println!(
            "  when it does not all fit: {} first",
            match node.config.keep_order {
                itsanas_policy::keeping::Order::Newest => "most recently changed",
                itsanas_policy::keeping::Order::Oldest => "least recently changed",
                itsanas_policy::keeping::Order::Smallest => "smallest",
            }
        );
    }

    if node.config.keep_only.is_empty() {
        return;
    }
    println!("  only these paths:");
    for prefix in &node.config.keep_only {
        println!("    {prefix}");
    }
}

/// What this machine can offer, what that earns, and which limit is binding.
///
/// # Why this is one command and not three questions in an installer
///
/// Two limits decide how much of your own data a device may hold, and they come
/// from different places: **the disk**, which is a fact about the machine, and
/// **what you have offered other people**, which is the bargain this network
/// runs on. A person choosing numbers needs both, together, before anything is
/// installed — otherwise the first they hear of the second is a coordinator
/// disagreeing with them a fortnight later.
///
/// Every installer asks this program rather than reimplementing the arithmetic
/// in shell, PowerShell and again in the Android settings screen. Three copies
/// of a rule is three answers to one question.
fn space(home: &Path, pledge: Option<&str>, keep: Option<&str>, apply: bool) -> Result<()> {
    let mut node = open(home)?;
    let split = node.config.split;

    let wanted_pledge = match pledge {
        Some(size) => parse_size(size)?,
        None => node.config.pledge_bytes,
    };
    let wanted_keep = match keep {
        Some(size) if size.eq_ignore_ascii_case("all") => None,
        Some(size) => Some(parse_size(size)?),
        None => node.config.keep_bytes,
    };

    // Free space where this node actually lives, not on some default drive: a
    // laptop with a small C: and a large D: is the ordinary case, and telling
    // somebody they have room they do not have is the one answer this must
    // never give.
    let free = fs4::available_space(&node.home).unwrap_or(0);
    let held = node.store.stats()?.bytes_on_disk;
    let hosted = node.vault.stats()?.bytes;

    println!("this machine");
    println!("  node at         {}", node.home.display());
    if free == 0 {
        println!("  free space      unknown (the filesystem would not say)");
    } else {
        println!("  free space      {}", format_size(free));
    }
    println!("  your data here  {}", format_size(held));
    println!("  held for others {}", format_size(hosted));

    let earned = split.room_earned(wanted_pledge);
    println!();
    println!("the bargain");
    println!("  you offer       {}", format_size(wanted_pledge));
    println!(
        "  that earns you  {} (a {split} split: {} of your own for every {} you lend)",
        format_size(earned),
        split.own,
        split.network
    );
    println!(
        "  first {} days    at least {}, whatever you pledge",
        itsanas_coord::accounting::JOINING_PERIOD_SECONDS / 86_400,
        format_size(itsanas_coord::accounting::JOINING_ALLOWANCE)
    );

    // Everything this machine would be committing to, together. The pledge is
    // room for other people's data and `keep` is room for yours; a disk has to
    // hold both, and neither setting knows about the other.
    let committed = wanted_pledge.saturating_add(wanted_keep.unwrap_or(0));
    let mut refusals: Vec<String> = Vec::new();

    if free > 0 && committed > free.saturating_add(held).saturating_add(hosted) {
        refusals.push(format!(
            "offering {} and keeping {} needs {} on a disk with {} free",
            format_size(wanted_pledge),
            wanted_keep.map_or_else(|| "everything".to_owned(), format_size),
            format_size(committed),
            format_size(free)
        ));
    }

    let allowed = earned.max(itsanas_coord::accounting::JOINING_ALLOWANCE);
    if let Some(keep) = wanted_keep
        && keep > allowed
    {
        refusals.push(format!(
            "keeping {} needs {} pledged; you are offering {}",
            format_size(keep),
            itsanas_node::config::size_argument(split.pledge_needed_for(keep)),
            format_size(wanted_pledge)
        ));
    }

    println!();
    if refusals.is_empty() {
        match wanted_keep {
            Some(keep) => println!(
                "keeping {} of your own here is within both limits",
                format_size(keep)
            ),
            None => println!("keeping all of your own data here is within both limits"),
        }
    } else {
        println!("that does not fit:");
        for refusal in &refusals {
            println!("  {refusal}");
        }
    }

    if !apply {
        if pledge.is_some() || keep.is_some() {
            println!();
            println!("Nothing was changed. Add `--apply` to set these.");
        }
        return Ok(());
    }

    if !refusals.is_empty() {
        return Err(CliError::Usage(
            "refusing to set numbers this machine cannot honour".to_owned(),
        ));
    }

    node.config.pledge_bytes = wanted_pledge;
    node.config.keep_bytes = wanted_keep;
    node.save_config()?;
    println!();
    println!("set.");
    Ok(())
}

fn pledge(home: &Path, size: &str) -> Result<()> {
    let bytes = parse_size(size)?;
    let mut node = open(home)?;

    let held = node.vault.stats()?.bytes;
    if bytes < held {
        // Lowering below what is already stored is allowed — the operator may
        // be reclaiming a disk — but it must be said out loud, because the node
        // will keep serving what it already took rather than silently dropping
        // a peer's data.
        println!(
            "warning: {} is already held for other people, which is more than \
             the new pledge of {}. Nothing will be deleted, and what is already \
             stored will still be served; this node simply will not accept more.",
            format_size(held),
            format_size(bytes)
        );
    }

    // Refusing to promise a disk this machine has not got. A host that accepts
    // data and then runs out has failed the person who trusted it, and "I
    // offered more than I had" is not a failure anybody discovers until it
    // matters.
    let free = fs4::available_space(&node.home).unwrap_or(0);
    if free > 0 && bytes > free.saturating_add(held) {
        return Err(CliError::Usage(format!(
            "offering {} on a disk with {} free, already holding {} for others",
            format_size(bytes),
            format_size(free),
            format_size(held)
        )));
    }

    node.config.pledge_bytes = bytes;
    node.save_config()?;

    println!("pledged {} to the network", format_size(bytes));
    Ok(())
}

fn serve(home: &Path, listen: Option<&str>) -> Result<()> {
    let node = open(home)?;
    let address = listen.unwrap_or(&node.config.listen);

    let server = PeerServer::bind(address)?;
    let bound = server.local_addr()?;

    let service = PeerService::new(
        &node.store,
        &node.vault,
        Pledge {
            bytes: node.config.pledge_bytes,
        },
    );

    println!("serving on {bound}");
    println!("  user id  {}", node.store.owner());
    println!("  device   {}", node.store.device_id());
    println!("  pledged  {}", format_size(node.config.pledge_bytes));
    println!();
    println!("Press Ctrl-C to stop.");

    // Never set: there is no signal handler yet, so Ctrl-C terminates the
    // process directly. Every write is committed before its command returns, so
    // an abrupt stop loses nothing.
    let shutdown = AtomicBool::new(false);
    server.serve_until(&service, &node.device, &shutdown)?;

    Ok(())
}

fn sync(home: &Path, address: Option<&str>, scope: session::Scope) -> Result<()> {
    let node = open(home)?;

    // Configured peers unpinned, as the daemon dials them; the account's other
    // devices from the coordinator pinned, because the coordinator supplies
    // addresses and is not trusted to say who lives at one.
    //
    // Without the second half, a machine just restored with `login --from` had
    // nothing to sync with: `sync` read only `peer add` entries, so the command
    // a person runs after recovering asked them to type an address -- which is
    // what acceptance test A says must never be needed. Only the daemon asked
    // the coordinator.
    let mut targets: Vec<(String, Option<DeviceId>)> = match address {
        Some(address) => vec![(address.to_owned(), None)],
        None => node
            .config
            .peers
            .iter()
            .map(|peer| (peer.clone(), None))
            .collect(),
    };
    if address.is_none() && node.config.coordinator.is_some() {
        match coordinator::peers(&node, node.store.owner()) {
            Ok(found) => {
                for (device, found_at) in found {
                    if !targets.iter().any(|(known, _)| *known == found_at) {
                        targets.push((found_at, Some(device)));
                    }
                }
            }
            // Not fatal: configured peers may still answer, and the error
            // below names the whole situation if nothing does.
            Err(error) => println!("coordinator: unreachable ({error})"),
        }
    }

    if targets.is_empty() {
        return Err(CliError::Usage(
            "no peer given, none configured, and no other device of this account \
             known to the coordinator. Try `itsanas sync <host:port>`, \
             `itsanas peer add <host:port>`, or `itsanas daemon`, which also finds \
             machines on this network."
                .to_owned(),
        ));
    }

    let mut any_succeeded = false;

    for (target, pinned) in &targets {
        print!("{target}: ");
        let _ = std::io::stdout().flush();

        let mut client =
            match PeerClient::connect(target.as_str(), &node.device, node.store.owner(), *pinned) {
                Ok(client) => client,
                Err(error) => {
                    // One unreachable peer must not abort the others: the whole
                    // point is that peers come and go.
                    println!("unreachable ({error})");
                    continue;
                }
            };

        // The same path the daemon takes, including the choice of what a
        // device short of room keeps. It used to be the daemon's alone, so
        // `itsanas sync` downloaded the whole account on a device that had
        // asked to hold two hundred kilobytes of it -- the mechanism existed
        // and the path a person actually takes did not use it. Found by running
        // it on a real machine, not by reading it.
        match crate::keeping::round(
            &node.store,
            &node.vault,
            &node.config.keeping(),
            &mut client,
            scope,
        ) {
            Ok((report, keeping)) => {
                any_succeeded = true;
                println!(
                    "sent {} in {} chunks, {} segments; received {} files, {} conflicts{}",
                    format_size(report.push.bytes_sent),
                    report.push.chunks_accepted,
                    report.push.segments_accepted,
                    report.pull.adopted,
                    report.pull.conflicted,
                    if report.pull.deferred > 0 {
                        format!(", {} deferred", report.pull.deferred)
                    } else {
                        String::new()
                    }
                );
                if let Some(refused) = describe_refusal(&report.push) {
                    println!("  {refused}");
                }
                if keeping.released > 0 {
                    println!(
                        "  let go of {} file(s), freeing {}",
                        keeping.released,
                        format_size(keeping.freed)
                    );
                }
                if keeping.not_safe_yet > 0 {
                    println!(
                        concat!(
                            "  {} file(s) stayed: letting go needs {} other live ",
                            "machines, one of which has confirmed that very chunk"
                        ),
                        keeping.not_safe_yet,
                        itsanas_store::holders::SAFE_TO_RELEASE
                    );
                }
            }
            Err(error) => println!("failed ({error})"),
        }
    }

    if !any_succeeded {
        return Err(CliError::Usage("no peer could be reached".to_owned()));
    }

    Ok(())
}

fn peer(home: &Path, action: PeerAction) -> Result<()> {
    let mut node = open(home)?;

    match action {
        PeerAction::Add { address } => {
            if node.config.peers.contains(&address) {
                println!("{address} is already configured");
                return Ok(());
            }
            node.config.peers.push(address.clone());
            node.save_config()?;
            println!("added {address}");
        }
        PeerAction::Find { username } => {
            let (user, found) = coordinator::find_member(&node, &username)?;
            println!("{username} is {user}");

            if found.is_empty() {
                println!("  ...and has published no address. They have registered but");
                println!("  no machine of theirs has announced itself yet.");
                return Ok(());
            }

            let mut added = 0;
            for (device, address) in found {
                if node.config.peers.contains(&address) {
                    println!("  {device}  {address}  (already configured)");
                } else {
                    node.config.peers.push(address.clone());
                    added += 1;
                    println!("  {device}  {address}");
                }
            }

            if added > 0 {
                node.save_config()?;
                println!("added {added} address(es)");
            }
        }
        PeerAction::Remove { address } => {
            let before = node.config.peers.len();
            node.config.peers.retain(|peer| peer != &address);
            if node.config.peers.len() == before {
                println!("{address} was not configured");
            } else {
                node.save_config()?;
                println!("removed {address}");
            }
        }
        PeerAction::List => {
            if node.config.peers.is_empty() {
                println!("(no peers configured)");
            }
            for peer in &node.config.peers {
                println!("{peer}");
            }
        }
    }

    Ok(())
}

/// Say, in order, everything this machine can find out about its connectivity.
///
/// The question a member actually has when nothing is syncing is "whose fault
/// is this", and until this existed the answer was a log line saying the
/// coordinator was unreachable, or -- worse -- silence, because a node with a
/// broken forward looks exactly like a node whose peers are all switched off.
///
/// Three questions, in the order that makes the next one worth asking:
///
/// 1. **Can I get out?** Dial the coordinator. The failure carries the reason,
///    and the reasons are different problems: a name that does not resolve is
///    DNS, a refused connection is a port, a timeout is usually a firewall.
/// 2. **Can anybody get in?** Only somebody outside can answer, so ask the
///    coordinator to try. Costs it one connection, which is why this is a
///    command a person runs rather than something on a timer.
/// 3. **What would I dial?** The account's other machines, and whether their
///    addresses are ones this machine could use from where it is standing.
fn network_report(node: &Node) {
    println!();
    println!("network");

    let Some(address) = node.config.coordinator.as_deref() else {
        println!("  no coordinator configured, so this machine can only meet peers on");
        println!("  its own network, or ones added by hand with `itsanas peer add`.");
        return;
    };

    match coordinator::devices(node, node.store.owner()) {
        Ok(found) => {
            println!("  out    the coordinator at {address} answered");
            let elsewhere = found
                .iter()
                .filter(|(device, _)| *device != node.store.device_id())
                .count();
            let dialable = found
                .iter()
                .filter(|(device, candidate)| {
                    *device != node.store.device_id() && !coordinator::is_private_address(candidate)
                })
                .count();
            println!(
                "  peers  {elsewhere} other machine(s) of this account have published an address"
            );
            if elsewhere > 0 && dialable == 0 {
                println!("         none of them is an address this machine could dial from");
                println!("         another network. On one LAN that is right and costs nothing;");
                println!("         from anywhere else nothing of this account can be reached.");
            }
        }
        Err(error) => {
            println!("  out    the coordinator at {address} could NOT be reached");
            println!("         {error}");
            println!("         A name that does not resolve is DNS; a refused connection is a");
            println!("         port that is closed or forwarded nowhere; a timeout is usually a");
            println!("         firewall. Syncing continues with peers already known.");
            return;
        }
    }

    match coordinator::check_me(node) {
        Ok(Some(coordinator::Reachability::Reachable(detail))) => println!("  in     {detail}"),
        Ok(Some(coordinator::Reachability::Unreachable(detail))) => {
            println!("  in     NOTHING can reach this machine: {detail}");
            if let Some(announce) = node.config.announce.as_deref() {
                println!("         This machine announces {announce}, so something was meant");
                println!("         to reach it there: check the forward, and that it points at");
                println!("         this machine's listening port.");
            } else {
                println!("         This machine announces nothing, so this is expected: it");
                println!("         takes part by dialling out, and one reachable side per");
                println!("         pair is enough. Set `itsanas announce` only if a forward");
                println!("         or an IPv6 route really does reach this machine.");
            }
        }
        // Nothing was tried, so nothing is known. Printing this as a verdict is
        // what sends somebody to rewire a router that works.
        Ok(Some(coordinator::Reachability::Unknown(why))) => {
            println!("  in     not checked this time: {why}");
        }
        Ok(None) => {
            println!("  in     not checked: this coordinator is too old to try reaching back");
        }
        Err(error) => println!("  in     not checked: {error}"),
    }
}

fn doctor(home: &Path, deep: bool) -> Result<()> {
    let node = open(home)?;
    let report = node.store.verify_integrity(deep)?;

    println!(
        "checked {} files{}",
        report.files_checked,
        if deep { " (deep)" } else { "" }
    );

    if report.is_healthy() && report.orphan_blobs.is_empty() {
        println!("the stored data checks out.");
        // Asked here and not in `status`: it costs the coordinator a real
        // connection to another machine, and `status` is run in loops by
        // scripts. `doctor` is what somebody runs because something is wrong.
        network_report(&node);
        return Ok(());
    }

    if !report.missing_chunks.is_empty() {
        println!();
        println!(
            "{} chunks are referenced but missing from disk:",
            report.missing_chunks.len()
        );
        for (path, chunk) in report.missing_chunks.iter().take(20) {
            println!("  {path} needs {}", chunk.short());
        }
        println!("  These files cannot be read until the chunks are refetched from a peer.");
    }

    if !report.corrupt_files.is_empty() {
        println!();
        println!("{} files failed verification:", report.corrupt_files.len());
        for path in report.corrupt_files.iter().take(20) {
            println!("  {path}");
        }
    }

    if !report.chain_intact {
        println!();
        println!("the operation log has a gap. Peers may not have the full history.");
    }

    if !report.orphan_blobs.is_empty() {
        println!();
        println!(
            "{} chunks on disk are not accounted for. These are leaked, not \
             dangerous — usually a crash between writing a chunk and committing \
             its index entry. `itsanas gc` reclaims them.",
            report.orphan_blobs.len()
        );
    }

    // Orphans alone are not a failure, and saying they are is worse than
    // saying nothing. A machine that lost power mid-write leaves them every
    // time; exiting non-zero for that means a monitoring wrapper reports a
    // healthy node as broken until somebody runs garbage collection by hand —
    // and a check that cries wolf after every power cut stops being read.
    //
    // Found by the crash test, which killed the process mid-write and then
    // could not tell "the store is damaged" from "the store is exactly as
    // expected after a crash".
    if report.is_healthy() {
        println!();
        println!("nothing is damaged. Those chunks are waiting for `itsanas gc`.");
        network_report(&node);
        return Ok(());
    }

    // The network section prints even when the store is damaged: the two
    // failures are unrelated, and somebody whose disk is hurt still wants to
    // know whether the machines that could repair it can be reached.
    network_report(&node);

    // A report is information, not a crash. Exit non-zero so a monitoring
    // system notices, but say everything first.
    Err(CliError::Usage(
        "integrity problems found (see above)".to_owned(),
    ))
}

fn gc(home: &Path, grace: u64) -> Result<()> {
    let node = open(home)?;
    let report = node
        .store
        .collect_garbage(std::time::Duration::from_secs(grace))?;

    println!(
        "reclaimed {} from {} chunks",
        format_size(report.bytes_reclaimed),
        report.blobs_removed
    );
    if report.retained_in_grace > 0 {
        println!(
            "{} chunks are unreferenced but still inside the {grace}s grace period",
            report.retained_in_grace
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        DeviceId, SNAPSHOT, describe_age, first_free_port, looks_like_a_closed_pipe,
        resolve_device, sibling_ports, snapshot_status,
    };

    /// The snapshot is read and dated without a passphrase anywhere near it.
    ///
    /// The function takes a path and nothing else, which is the guarantee: it
    /// cannot prompt, so `status` cannot be made to ask for a key on the path
    /// whose purpose is to answer while the daemon holds the store.
    #[test]
    fn red_team_a_running_node_is_reported_with_its_age_and_no_passphrase() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path();
        let taken = itsanas_discover::now_unix().saturating_sub(4 * 3600);
        std::fs::write(
            home.join(SNAPSHOT),
            format!("snapshot {taken}\n  files          3\n"),
        )
        .expect("write snapshot");

        let out = snapshot_status(home, true).expect("a stamped snapshot is readable");
        assert!(
            out.contains("4 hours ago"),
            "the snapshot's age was not reported, so a reader cannot tell a \
             live answer from one left by a daemon that died last week: {out}"
        );
        assert!(
            out.contains("files          3"),
            "the snapshot body was dropped: {out}"
        );
    }

    /// A stopped node's report never reads as a live one.
    ///
    /// `status` prints the snapshot in two quite different situations: the
    /// daemon is holding the store, so the file is a recent report of a
    /// running node; or nothing is running and it is the last thing a stopped
    /// node said, possibly last week. Printing one sentence for both would
    /// make "this node is running" a claim the command cannot support -- and
    /// that sentence is the one a person uses to decide whether to trust the
    /// numbers under it.
    #[test]
    fn red_team_a_stopped_node_is_never_reported_as_a_running_one() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path();
        let taken = itsanas_discover::now_unix().saturating_sub(6 * 86_400);
        std::fs::write(
            home.join(SNAPSHOT),
            format!("snapshot {taken}\n  files          3\n"),
        )
        .expect("write snapshot");

        let stopped = snapshot_status(home, false).expect("readable");
        assert!(
            !stopped.contains("this node is running"),
            "a node nothing is running was reported as running: {stopped}"
        );
        assert!(
            stopped.contains("6 days ago"),
            "a six-day-old snapshot did not say so: {stopped}"
        );

        let running = snapshot_status(home, true).expect("readable");
        assert!(
            running.contains("this node is running"),
            "a held store should say the node is running: {running}"
        );
    }

    /// An undated snapshot is printed, and never given an age it never had.
    #[test]
    fn a_snapshot_without_a_stamp_is_printed_but_not_dated() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path();
        std::fs::write(home.join(SNAPSHOT), "  files          3\n").expect("write snapshot");

        let out = snapshot_status(home, false).expect("an unstamped snapshot is still readable");
        assert!(
            out.contains("no time on it"),
            "an undated snapshot was passed off as current: {out}"
        );
        assert!(
            !out.contains(" ago"),
            "an age was invented for a snapshot that carried no stamp: {out}"
        );
    }

    /// A node with no snapshot yet says so, rather than reporting nothing.
    #[test]
    fn a_node_that_has_never_synced_says_so_rather_than_printing_nothing() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert!(
            snapshot_status(dir.path(), true).is_err(),
            "a missing snapshot produced a successful, empty status, which \
             reads as a healthy node that has simply nothing to report"
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
    fn a_port_another_node_on_this_machine_is_configured_for_is_not_chosen() {
        // The case the kernel cannot see: the first account's daemon is
        // stopped, so 9797 binds, and handing it to the second account puts
        // two daemons on one port at the next boot.
        let taken = std::collections::BTreeSet::from([9797]);
        assert_eq!(first_free_port(&taken, |_| true), Some(9798));
    }

    /// The line says how many siblings there are, because that is what it is for.
    ///
    /// The second account on this machine skipped 9797 and 9798 and was told
    /// "9797 is used by another node on this machine" -- singular, and naming
    /// one of the two. Ports here are handed out without asking, so this line
    /// is the only place the person learns what happened.
    #[test]
    fn the_ports_a_node_had_to_skip_are_all_named_not_just_the_first() {
        assert!(
            super::ports_skipped(9798).contains("9797 is used"),
            "one skipped port should be named in the singular"
        );
        let two = super::ports_skipped(9799);
        assert!(
            two.contains("9797-9798") && two.contains("are used"),
            "two skipped ports were reported as one, which undercounts the \
             instances on this machine: {two}"
        );
    }

    #[test]
    fn a_port_something_already_holds_is_skipped_and_exhaustion_says_so() {
        let taken = std::collections::BTreeSet::new();
        assert_eq!(first_free_port(&taken, |port| port > 9799), Some(9800));
        assert_eq!(
            first_free_port(&taken, |_| false),
            None,
            "a port nothing can bind was offered as free"
        );
    }

    #[test]
    fn the_ports_of_the_other_nodes_beside_this_one_are_found_and_its_own_is_not() {
        // A sibling is a directory holding a keystore. A directory without one
        // is not a node, and this node's own configuration must not count
        // against it, or re-running `init` logic would move it off its port.
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, listen: &str, keystore: bool| {
            let home = dir.path().join(name);
            std::fs::create_dir_all(&home).unwrap();
            if keystore {
                std::fs::write(home.join("keystore.bin"), b"sealed").unwrap();
            }
            let config = crate::config::Config {
                listen: listen.to_owned(),
                ..crate::config::Config::default()
            };
            config.save(&crate::node::Node::config_path(&home)).unwrap();
            home
        };
        write(".itsanas", "0.0.0.0:9797", true);
        write("not-a-node", "0.0.0.0:9799", false);
        let this = write(".itsanas-bob", "0.0.0.0:9798", true);

        assert_eq!(
            sibling_ports(&this),
            std::collections::BTreeSet::from([9797])
        );
    }

    #[test]
    fn a_taken_listen_port_is_answered_with_a_free_one_and_the_commands_to_move() {
        // Nodes made before `init` chose ports all sit on 9797. The daemon of
        // the second one used to exit with "address in use" -- under systemd,
        // every thirty seconds -- and nothing said which port would work or
        // how to move there.
        let told = crate::daemon::taken_port_message("0.0.0.0:9797", &"address in use", Some(9798));
        assert!(told.contains("itsanas listen 0.0.0.0:9798"), "{told}");
        assert!(told.contains("itsanas register"), "{told}");
        let none = crate::daemon::taken_port_message("0.0.0.0:9797", &"address in use", None);
        assert!(
            none.contains("itsanas listen") && !none.contains("0.0.0.0:9798"),
            "a port was offered where none is free: {none}"
        );
    }

    #[test]
    fn a_refusal_is_reported_once_and_then_only_after_a_quiet_period() {
        // A pledge-0 host refuses every round. The line that fixed a silent
        // round must not turn into one line every five minutes per peer.
        let now = std::time::Instant::now();
        assert!(
            crate::daemon::refusal_due(None, now),
            "the first refusal from a peer was not reported"
        );
        assert!(
            !crate::daemon::refusal_due(Some(now), now + std::time::Duration::from_secs(300)),
            "a refusal was repeated at the next round"
        );
        assert!(
            crate::daemon::refusal_due(Some(now), now + crate::daemon::OUTAGE_QUIET_FOR_TESTS),
            "a refusal that is still going on was never reported again"
        );
    }

    fn listed() -> Vec<(DeviceId, String)> {
        vec![
            (DeviceId::from_bytes([0xab; 32]), "a:1".to_owned()),
            (DeviceId::from_bytes([0xcd; 32]), "b:2".to_owned()),
        ]
    }

    #[test]
    fn a_prefix_that_names_no_device_is_refused_rather_than_invented() {
        // This is the half that can do damage. A revocation is a signed record
        // the coordinator files and honours; one written against an identifier
        // nobody holds is silent, permanent, and impossible to notice.
        let devices = listed();
        for typed in ["ffff", "0", "not-hex", "abcdef01", ""] {
            assert!(
                resolve_device(typed, &devices).is_err(),
                concat!(
                    "{:?} was resolved to a device, and no device on the ",
                    "account starts with it"
                ),
                typed
            );
        }
    }

    #[test]
    fn the_short_form_the_logs_print_is_enough_to_name_a_device() {
        // The error a person is reacting to prints twelve characters. Making
        // them go and find the other fifty-two is asking them to do work the
        // program can do -- but only when the answer is unambiguous.
        let devices = listed();
        let full = DeviceId::from_bytes([0xab; 32]);

        assert_eq!(resolve_device(&full.to_string(), &devices).unwrap(), full);
        assert_eq!(resolve_device(&full.short(), &devices).unwrap(), full);

        // "ab" is unique here; a prefix shared by both must refuse rather than
        // pick one.
        assert_eq!(resolve_device("ab", &devices).unwrap(), full);
        assert!(resolve_device("", &devices).is_err());
    }

    #[test]
    fn an_age_never_reads_as_fresher_than_it_is() {
        // The number this describes decides whether somebody trusts what they
        // are looking at, so every boundary rounds *down* -- towards admitting
        // the snapshot is older -- and nothing below a minute is allowed to
        // call itself "just now" except the few seconds where it is true.
        assert_eq!(describe_age(0), "just now");
        assert_eq!(describe_age(5), "just now");
        assert_eq!(describe_age(6), "6 seconds ago");
        assert_eq!(describe_age(89), "89 seconds ago");
        assert_eq!(describe_age(90), "1 minutes ago");
        assert_eq!(describe_age(3599), "59 minutes ago");
        assert_eq!(describe_age(5399), "89 minutes ago");
        assert_eq!(describe_age(5400), "1 hours ago");
        assert_eq!(describe_age(86_399), "23 hours ago");
        assert_eq!(describe_age(86_400), "1 days ago");

        // The case that matters most: a daemon that died three days ago must
        // not leave a snapshot that reads as if it were current.
        assert_eq!(describe_age(3 * 86_400 + 7), "3 days ago");
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
