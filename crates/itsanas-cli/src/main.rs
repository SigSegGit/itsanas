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

mod config;
mod daemon;
mod discovery;
mod error;
mod node;

use std::{
    io::{IsTerminal as _, Read as _, Write as _},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::atomic::AtomicBool,
};

use clap::{Parser, Subcommand};
use itsanas_net::{PeerClient, PeerServer, PeerService, Pledge, session};

use crate::{
    config::{format_size, parse_size},
    error::{CliError, Result},
    node::Node,
};

/// Environment variable that supplies the passphrase non-interactively.
///
/// For cron jobs and systemd units, which have no terminal to prompt at. A
/// passphrase in the environment is visible to anything that can read the
/// process's environment, so this is a deliberate trade the operator makes,
/// not a default.
const PASSPHRASE_ENV: &str = "ITSANAS_PASSPHRASE";

/// How many machines should hold each chunk, this one included.
///
/// Three is the smallest number where losing one machine is not an emergency
/// and two have to fail at once to lose anything. The reasoning, and why the
/// contribution ratio is the same number, is in `docs/ECONOMICS.md` §1.
const REPLICATION_TARGET: usize = 3;

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

#[derive(Subcommand)]
enum Command {
    /// Create a new account on this machine and print its recovery phrase.
    Init {
        /// Account name, as it would be registered with a coordinator.
        #[arg(long)]
        username: String,
    },
    /// Restore an existing account on this machine from its recovery phrase.
    Login {
        #[arg(long)]
        username: String,
        /// Read the 24-word phrase from this file instead of prompting.
        #[arg(long)]
        phrase_file: Option<PathBuf>,
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
        /// The directory. Omit to show the current setting.
        path: Option<PathBuf>,
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
    /// Say how much space this node offers to other people.
    Pledge {
        /// e.g. `500M`, `10G`, `1T`.
        size: String,
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
        /// Seconds between sync rounds.
        #[arg(long, default_value_t = daemon::DEFAULT_INTERVAL.as_secs())]
        interval: u64,
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
}

fn main() -> ExitCode {
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
        } => login(&home, &username, phrase_file.as_deref()),
        Command::Status => status(&home),
        Command::Whoami => whoami(&home),
        Command::Ls => list(&home),
        Command::Put { path, source } => put(&home, &path, &source),
        Command::Get { path, destination } => get(&home, &path, destination.as_deref()),
        Command::Rm { path } => remove(&home, &path),
        Command::Folder { path } => folder(&home, path.as_deref()),
        Command::Scan { deep } => scan(&home, deep),
        Command::Pledge { size } => pledge(&home, &size),
        Command::Serve { listen } => serve(&home, listen.as_deref()),
        Command::Daemon {
            listen,
            interval,
            no_discovery,
        } => daemon::run(
            &open(&home)?,
            listen.as_deref(),
            std::time::Duration::from_secs(interval.max(1)),
            !no_discovery,
        ),
        Command::Sync { address } => sync(&home, address.as_deref()),
        Command::Peer { action } => peer(&home, action),
        Command::Doctor { deep } => doctor(&home, deep),
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

    let (node, phrase) = Node::create(home, &passphrase(true)?, username)?;

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
    println!("Next: `itsanas pledge 10G` to offer space, then `itsanas serve`.");

    Ok(())
}

fn login(home: &Path, username: &str, phrase_file: Option<&std::path::Path>) -> Result<()> {
    if Node::exists(home) {
        return Err(CliError::NodeExists(home.to_path_buf()));
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
    let node = Node::restore(home, &passphrase(true)?, username, phrase.trim())?;

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

fn status(home: &Path) -> Result<()> {
    let node = open(home)?;
    let store = node.store.stats()?;
    let vault = node.vault.stats()?;

    println!("account");
    println!("  username        {}", node.config.username);
    println!("  user id         {}", node.store.owner());
    println!("  device          {}", node.store.device_id());
    println!("  home            {}", node.home.display());
    match &node.config.folder {
        Some(folder) => println!("  synced folder   {}", folder.display()),
        None => println!("  synced folder   none (`itsanas folder <path>`)"),
    }
    println!();
    println!("your data");
    println!("  files           {}", store.files);
    println!("  live chunks     {}", store.live_chunks);
    println!("  on disk         {}", format_size(store.bytes_on_disk));
    println!("  log segments    {}", store.segments);
    if store.unsealed_entries > 0 {
        println!(
            "  unannounced     {} (run `itsanas sync` to publish)",
            store.unsealed_entries
        );
    }
    if store.pending_collection > 0 {
        println!("  awaiting gc     {} chunks", store.pending_collection);
    }

    // The question a backup tool exists to answer, and the one it is easiest
    // to leave unanswered: does this data exist anywhere other than this disk?
    // A count of files says nothing about that.
    println!();
    println!("is it anywhere else?");
    let alone = node.store.under_replicated(2)?;
    let short = node.store.under_replicated(REPLICATION_TARGET)?;
    if store.live_chunks == 0 {
        println!("  nothing stored yet");
    } else if alone.is_empty() && short.is_empty() {
        println!("  yes            every chunk is on at least {REPLICATION_TARGET} machines");
    } else {
        if alone.is_empty() {
            println!("  partly         every chunk is on at least one other machine");
        } else {
            println!(
                "  NO             {} of {} chunks exist only on this machine",
                alone.len(),
                store.live_chunks
            );
        }
        if !short.is_empty() {
            println!(
                "  below target   {} chunks are on fewer than {REPLICATION_TARGET} machines",
                short.len()
            );
        }
        println!("                 run `itsanas sync`, or add a peer, to spread it");
    }
    println!("  placements     {} recorded", store.holder_records);
    // The vault holds two different things. Reporting them as one number
    // tells the operator they are hosting for a stranger when they are only
    // relaying their own account between their own machines.
    let own_in_vault = node.vault.stats_for(node.store.owner())?;
    let hosted_owners = vault
        .owners
        .saturating_sub(usize::from(own_in_vault.segments > 0));
    let hosted_bytes = vault.bytes.saturating_sub(own_in_vault.bytes);
    let hosted_chunks = vault.chunks.saturating_sub(own_in_vault.chunks);

    println!();
    println!("hosting for other people");
    println!(
        "  pledged         {}",
        format_size(node.config.pledge_bytes)
    );
    println!("  used            {}", format_size(hosted_bytes));
    println!("  peers hosted    {hosted_owners}");
    println!("  chunks held     {hosted_chunks}");
    println!(
        "  segments held   {}",
        vault.segments.saturating_sub(own_in_vault.segments)
    );
    println!();
    println!("relaying for your own devices");
    println!(
        "  segments held   {} (so this machine can pass your other devices' \
         work along)",
        own_in_vault.segments
    );
    println!();
    println!("network");
    println!("  listen          {}", node.config.listen);
    if node.config.peers.is_empty() {
        println!("  peers           none configured (`itsanas peer add <host:port>`)");
    } else {
        for peer in &node.config.peers {
            println!("  peer            {peer}");
        }
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
    let entries = node.store.entries()?;

    if entries.is_empty() {
        println!("(no files)");
        return Ok(());
    }

    for (path, entry) in entries {
        println!(
            "{:>12}  {:>5} chunks  {}",
            format_size(entry.size),
            entry.chunks.len(),
            path
        );
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

fn get(home: &Path, path: &str, destination: Option<&std::path::Path>) -> Result<()> {
    let node = open(home)?;

    let Some(content) = node.store.read_file(path)? else {
        return Err(CliError::Usage(format!("no such file: {path}")));
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

fn folder(home: &Path, path: Option<&Path>) -> Result<()> {
    let mut node = open(home)?;

    let Some(path) = path else {
        match &node.config.folder {
            Some(folder) => println!("{}", folder.display()),
            None => println!("(no folder configured — `itsanas folder <path>`)"),
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

fn sync(home: &Path, address: Option<&str>) -> Result<()> {
    let node = open(home)?;

    let targets: Vec<String> = match address {
        Some(address) => vec![address.to_owned()],
        None => node.config.peers.clone(),
    };

    if targets.is_empty() {
        return Err(CliError::Usage(
            "no peer given and none configured. Try `itsanas sync <host:port>` \
             or `itsanas peer add <host:port>`."
                .to_owned(),
        ));
    }

    let mut any_succeeded = false;

    for target in &targets {
        print!("{target}: ");
        let _ = std::io::stdout().flush();

        let mut client =
            match PeerClient::connect(target.as_str(), &node.device, node.store.owner(), None) {
                Ok(client) => client,
                Err(error) => {
                    // One unreachable peer must not abort the others: the whole
                    // point is that peers come and go.
                    println!("unreachable ({error})");
                    continue;
                }
            };

        match session::round(&node.store, &node.vault, &mut client) {
            Ok(report) => {
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

fn doctor(home: &Path, deep: bool) -> Result<()> {
    let node = open(home)?;
    let report = node.store.verify_integrity(deep)?;

    println!(
        "checked {} files{}",
        report.files_checked,
        if deep { " (deep)" } else { "" }
    );

    if report.is_healthy() && report.orphan_blobs.is_empty() {
        println!("everything checks out.");
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
