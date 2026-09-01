//! `itsanas-coordinator` — the address book, as a service.
//!
//! # What this process is worth stealing
//!
//! Nothing, and that is the design rather than a boast. It holds usernames
//! mapped to public keys, signed device enrolments, addresses, and escrow blobs
//! sealed under passphrases it never sees. Every one of those is signed by a key
//! it does not have or opaque to it.
//!
//! Someone who takes this machine can refuse to answer and lie about who is
//! online. They cannot read a file, forge a log entry, delete anything, or
//! recover an account without also guessing its passphrase. The reasoning is in
//! `docs/DESIGN.md` §8.
//!
//! # Running it where it is meant to run
//!
//! A small always-on machine with a public address — a VPS, or a VM on a Freebox
//! Delta. It is the only publicly reachable component of ITSaNAS, so hostile
//! traffic is its normal condition and the limits are in `itsanas-coord::server`
//! rather than in a firewall somebody has to remember to configure.
//!
//! ```text
//! itsanas-coordinator --state /var/lib/itsanas-coordinator --listen 0.0.0.0:9898
//! ```
//!
//! Its device key is generated on first start and kept beside the directory, so
//! peers that pinned it keep reaching the same coordinator across restarts.

use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::atomic::{AtomicBool, Ordering},
};

use clap::Parser;
use itsanas_coord::{Admission, DEFAULT_COORD_PORT, Directory, server::CoordServer};
use itsanas_crypto::{DeviceKeys, SecretBytes, SymmetricKey};

/// Filename of the coordinator's own device key, beside its directory.
const DEVICE_KEY: &str = "device.key";

/// Set by the interrupt handler.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

#[derive(Parser)]
#[command(
    name = "itsanas-coordinator",
    version,
    about = "An ITSaNAS coordinator: usernames, addresses and sealed escrow blobs. No keys, no data."
)]
struct Cli {
    /// Where to keep the directory and this server's own device key.
    #[arg(long, env = "ITSANAS_COORD_STATE", default_value = "./coordinator")]
    state: PathBuf,

    /// Address to listen on.
    #[arg(long, default_value_t = format!("0.0.0.0:{DEFAULT_COORD_PORT}"))]
    listen: String,

    /// Print the device id and exit.
    ///
    /// Members pin this when they configure the coordinator, so that an address
    /// resolving elsewhere is refused rather than trusted.
    #[arg(long)]
    identity: bool,

    /// Admit new members only if an existing one invited them.
    ///
    /// Off by default, which is right for a household: the operator is the only
    /// person who knows the address, and an invitation to admit the *first*
    /// member has no author. Turn it on once the address is somewhere a stranger
    /// could find it.
    ///
    /// A keypair costs nothing, so without this the answer to "who is a member"
    /// is "anyone who can open a socket" — and every other defence in this
    /// project is aimed at a hostile host, which is somebody who joined.
    ///
    /// Members already registered are unaffected: re-registering is how a member
    /// refreshes their keys, and demanding a fresh invitation for it would lock
    /// people out of their own accounts.
    #[arg(long)]
    invite_only: bool,

    /// Admit one account without an invitation, to create the first member.
    ///
    /// An invitation to admit the first member has no author, so an invite-only
    /// coordinator with nobody in it can never be joined. This opens the door
    /// once.
    ///
    /// It is a flag rather than automatic because the difference is a race: an
    /// empty directory that always admitted its first caller would, on a public
    /// address, make the founder whoever finds the port first — and the
    /// operator would learn this by being refused from their own coordinator.
    /// Pass it, register, and restart without it.
    #[arg(long, requires = "invite_only")]
    admit_first: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("itsanas-coordinator: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();

    fs::create_dir_all(&cli.state)
        .map_err(|error| format!("could not create {}: {error}", cli.state.display()))?;

    let device = load_or_create_device(&cli.state)?;

    if cli.identity {
        println!("{}", device.device_id());
        return Ok(());
    }

    let admission = match (cli.invite_only, cli.admit_first) {
        (true, true) => Admission::Founding,
        (true, false) => Admission::ByInvitation,
        _ => Admission::Open,
    };

    let directory = Directory::open(cli.state.join("directory.redb"))
        .map_err(|error| format!("could not open the directory: {error}"))?;

    let server = CoordServer::bind(&cli.listen)
        .map_err(|error| format!("could not listen on {}: {error}", cli.listen))?;
    let bound = server
        .local_addr()
        .map_err(|error| format!("bound to nothing: {error}"))?;

    ctrlc::set_handler(|| {
        SHUTDOWN.store(true, Ordering::Relaxed);
        println!();
        println!("stopping…");
    })
    .map_err(|error| format!("could not install a signal handler: {error}"))?;

    println!("itsanas-coordinator");
    println!("  listening {bound}");
    println!("  device    {}", device.device_id());
    println!("  state     {}", cli.state.display());
    println!(
        "  admits    {}",
        if cli.invite_only {
            "invited strangers only (existing members are unaffected)"
        } else {
            "anybody who can reach this address"
        }
    );
    if cli.invite_only && directory.is_empty().unwrap_or(false) {
        println!();
        if cli.admit_first {
            println!("  --admit-first: the NEXT registration is let in without an invitation.");
            println!("  Register your own account now. The window shuts after one account,");
            println!("  and restarting without the flag closes it whatever happens.");
        } else {
            println!("  No accounts yet, and no invitation can admit the first member because");
            println!("  it would have no author. Restart once with --admit-first, register,");
            println!("  then restart without it.");
        }
    }
    println!();
    println!("Members should pin that device id: `itsanas coordinator <address> --device <id>`.");
    println!("An address that answers as anything else is then refused rather than trusted.");
    println!();
    println!("Ctrl-C to stop.");
    println!();

    server
        .serve_admitting(&directory, &device, admission, &SHUTDOWN, |event| {
            eprintln!("itsanas-coordinator: {event}");
        })
        .map_err(|error| format!("the listener stopped: {error}"))?;

    println!("stopped.");
    Ok(())
}

/// Load this coordinator's device key, generating it once on first start.
///
/// Stored in the clear beside the directory, deliberately. Encrypting it would
/// need a passphrase at every boot, which means either a human present when the
/// VM restarts or the passphrase on the same disk — and the key protects
/// nothing worth a passphrase: it identifies the coordinator so that peers can
/// pin it, and grants no access to anything.
fn load_or_create_device(state: &Path) -> Result<DeviceKeys, String> {
    let path = state.join(DEVICE_KEY);

    match fs::read(&path) {
        Ok(bytes) => {
            let seed: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| format!("{} is not a device key", path.display()))?;
            Ok(DeviceKeys::from_seed(&SymmetricKey::new(seed)))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let device = DeviceKeys::generate()
                .map_err(|error| format!("could not generate a device key: {error}"))?;
            let seed: SecretBytes<32> = device.seed();
            fs::write(&path, seed.expose())
                .map_err(|error| format!("could not write {}: {error}", path.display()))?;
            restrict(&path);
            // stderr, not stdout. `--identity` exists so that a script can read
            // the device id, and `install/coordinator.sh` does exactly that:
            // members have to pin this value, so the installer prints it for
            // them. On a fresh machine — the only time this branch runs, and
            // the only time the installer runs — that put a sentence of prose
            // on stdout ahead of the id, and the installer died with "could not
            // read the coordinator's device id" while quoting the id back.
            //
            // A command whose output is meant to be parsed says one thing on
            // stdout. Anything for a person goes to stderr, where they still
            // see it and no parser has to know about it.
            eprintln!(
                "Generated a new coordinator identity: {}",
                device.device_id()
            );
            Ok(device)
        }
        Err(error) => Err(format!("could not read {}: {error}", path.display())),
    }
}

/// Make the key file readable only by its owner, where the platform allows.
#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

/// Windows inherits the directory's access control list; nothing to narrow
/// without pulling in a platform crate, and the coordinator is meant to run on
/// a Linux VM.
#[cfg(not(unix))]
fn restrict(_path: &Path) {}
