//! `itsanas-coordinator` — the address book, as a service.
//!
//! # What this process is worth stealing
//!
//! Nothing, and that is the design rather than a boast. It holds usernames
//! mapped to public keys, signed device enrolments, addresses, and escrow blobs
//! sealed under passphrases it never sees. Every one of those is signed by a key
//! it does not have or opaque to it. An address goes once its device has been
//! silent for `PRESENCE_KEPT_FOR`; this process erases those as it starts and
//! once a day (`itsanas_coord::server::EXPIRE_EVERY`).
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
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::atomic::{AtomicBool, Ordering},
};

use clap::Parser;
use itsanas_coord::{
    AccountInventory, Admission, DEFAULT_COORD_PORT, Directory, server::CoordServer,
};
use itsanas_crypto::{DeviceKeys, SecretBytes, SymmetricKey};

mod mailjet;

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

    #[command(flatten)]
    inventory: InventoryArgs,

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

    /// Questions about names one address may ask an hour (`Lookup`,
    /// `GetEscrow`, and a refused registration); an IPv6 /48 asks four times
    /// that. Raise it when many members reach this coordinator from one
    /// address (a campus, carrier-grade NAT); a test bench that creates
    /// dozens of accounts from loopback raises it too.
    #[arg(
        long,
        env = "ITSANAS_COORD_NAME_QUESTIONS",
        default_value_t = itsanas_coord::server::NAME_QUESTIONS
    )]
    name_questions: u32,

    /// Forget the account registered under this username, then exit.
    ///
    /// Frees the name for anyone to register again -- a test account would
    /// otherwise hold it for ever. Its devices' live claims and addresses go
    /// with it; its withdrawals stay, so a withdrawn machine stays withdrawn.
    /// Stop the running coordinator first: the directory is opened
    /// exclusively, and this refuses while another process holds it.
    #[arg(long, value_name = "USERNAME")]
    forget_account: Option<String>,

    /// Suspend the account registered under this username, then exit.
    ///
    /// For an abuse report or a legal request. Hosts stop storing anything
    /// new for its machines, it cannot enrol or announce, and nobody is handed
    /// its address. What hosts already keep for it stays, unreadable to the
    /// operator as before: lift it with `--unsuspend`. Stop the running
    /// coordinator first, as for `--forget-account`.
    #[arg(long, value_name = "USERNAME", conflicts_with_all = ["forget_account", "unsuspend"])]
    suspend: Option<String>,

    /// Lift a suspension set with `--suspend`, then exit.
    #[arg(long, value_name = "USERNAME", conflicts_with = "forget_account")]
    unsuspend: Option<String>,
}

#[derive(clap::Args)]
struct InventoryArgs {
    /// Print accounts and stored live device claims locally, then exit.
    ///
    /// Read-only: stop the service first and use the same `--state` directory.
    #[arg(long, conflicts_with_all = [
        "identity",
        "invite_only",
        "admit_first",
        "forget_account",
        "suspend",
        "unsuspend"
    ])]
    inventory: bool,
}

/// `--suspend` (`lift` false) or `--unsuspend` (`lift` true) `name`, saying
/// what changed.
fn suspend_or_lift(directory: &Directory, name: &str, lift: bool) -> Result<(), String> {
    let account = directory
        .account(name)
        .map_err(|error| format!("could not read {name:?}: {error}"))?
        .ok_or_else(|| format!("no account is registered as {name:?}"))?;
    let user = account.user.id;
    if lift {
        let was = directory
            .unsuspend(user)
            .map_err(|error| format!("could not unsuspend {name:?}: {error}"))?;
        if was {
            println!("unsuspended {name:?} ({user})");
            println!("  hosts store for its machines again once they ask the coordinator");
        } else {
            println!("{name:?} ({user}) was not suspended; nothing changed");
        }
    } else {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let fresh = directory
            .suspend(user, now)
            .map_err(|error| format!("could not suspend {name:?}: {error}"))?;
        if fresh {
            println!("suspended {name:?} ({user})");
        } else {
            println!("{name:?} ({user}) was already suspended; nothing changed");
        }
        println!("  hosts take no new data for its machines; what they keep stays, unread");
        println!("  it cannot enrol or announce, and nobody is handed its address");
        println!("  lift it with --unsuspend {name}");
    }
    Ok(())
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

/// The server with a Mailjet sender behind it when the environment names
/// one ([`mailjet::Mailjet::from_env`]), and that configuration.
fn with_mail(server: CoordServer) -> Result<(CoordServer, Option<mailjet::Mailjet>), String> {
    let Some(mailjet) = mailjet::Mailjet::from_env() else {
        return Ok((server, None));
    };
    let (outbox, queue) = itsanas_coord::mail::Outbox::new();
    let sender = mailjet.clone();
    std::thread::Builder::new()
        .name("mail".to_owned())
        .spawn(move || sender.run(&queue))
        .map_err(|error| format!("could not start the mail sender: {error}"))?;
    Ok((server.with_outbox(outbox), Some(mailjet)))
}

/// The start-up line saying whether this coordinator mails.
fn print_mail(mailjet: Option<&mailjet::Mailjet>) {
    match mailjet {
        Some(mailjet) => println!("  mail      from {} through Mailjet", mailjet.from()),
        None => println!(
            "  mail      off (MAILJET_API_KEY, MAILJET_SECRET_KEY, ITSANAS_MAIL_FROM): no address can be bound"
        ),
    }
}

/// The server this run listens with, and the address it got.
fn listen(cli: &Cli) -> Result<(CoordServer, std::net::SocketAddr), String> {
    let server = CoordServer::bind(&cli.listen)
        .map_err(|error| format!("could not listen on {}: {error}", cli.listen))?
        .with_name_questions(cli.name_questions);
    let bound = server
        .local_addr()
        .map_err(|error| format!("bound to nothing: {error}"))?;
    Ok((server, bound))
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();

    if cli.inventory.inventory {
        return run_inventory(&cli.state);
    }

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

    if let Some(name) = &cli.forget_account {
        let forgotten = directory
            .forget_account(name)
            .map_err(|error| format!("could not forget {name:?}: {error}"))?
            .ok_or_else(|| format!("no account is registered as {name:?}"))?;
        println!("forgot {name:?} ({})", forgotten.user);
        println!(
            "  {} device(s) removed, {} withdrawal(s) kept",
            forgotten.devices, forgotten.withdrawals_kept
        );
        println!("  the name is free; its holder's data on hosts is not touched");
        return Ok(());
    }

    if let Some((name, lift)) = cli
        .suspend
        .as_ref()
        .map(|name| (name, false))
        .or_else(|| cli.unsuspend.as_ref().map(|name| (name, true)))
    {
        return suspend_or_lift(&directory, name, lift);
    }

    let (server, bound) = listen(&cli)?;
    let (server, mailjet) = with_mail(server)?;

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
    // Two numbers that must be equal: the claims, and the index that makes a
    // per-account lookup cheap. They are printed rather than merely asserted
    // because an index that has fallen behind makes machines invisible without
    // any other symptom -- they announce, and nobody is ever told about them.
    match directory.enrolled_counts() {
        Ok((claims, indexed)) if claims == indexed => {
            println!("  devices   {claims} enrolled, and the index agrees");
        }
        Ok((claims, indexed)) => {
            println!("  devices   {claims} enrolled but {indexed} indexed -- THEY DISAGREE");
            println!("            Machines in the first and not the second announce and are");
            println!("            never handed out. Stop this, take a copy of the state");
            println!("            directory, and report it: the repair on open should have");
            println!("            made these equal.");
        }
        Err(error) => println!("  devices   could not be counted: {error}"),
    }
    print_mail(mailjet.as_ref());
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

fn run_inventory(state: &Path) -> Result<(), String> {
    let path = state.join("directory.redb");
    let inventory = Directory::read_inventory(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    print!("{}", inventory_output(&inventory));
    Ok(())
}

fn inventory_output(accounts: &[AccountInventory]) -> String {
    let mut output = String::from("Coordinator inventory (local, read-only)\n");
    if accounts.is_empty() {
        output.push_str("No accounts registered.\n");
        return output;
    }

    for entry in accounts {
        let status = if entry.suspended { " [SUSPENDED]" } else { "" };
        writeln!(
            output,
            "Account: {} ({}){status}",
            entry.account.username, entry.account.user.id
        )
        .expect("writing to a String");
        if entry.devices.is_empty() {
            output.push_str("  no active device claims\n");
            continue;
        }

        for inventory in &entry.devices {
            let device = inventory.claim.claim.device;
            writeln!(output, "  Device: {device}").expect("writing to a String");
            writeln!(
                output,
                "    address: {}",
                inventory.address.as_deref().unwrap_or("unavailable")
            )
            .expect("writing to a String");
            match inventory.last_seen_unix {
                Some(unix) => {
                    writeln!(output, "    last seen (Unix seconds): {unix}")
                        .expect("writing to a String");
                }
                None => output.push_str("    last seen: unavailable\n"),
            }
        }
    }
    output
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

#[cfg(test)]
mod tests {
    use super::inventory_output;
    use itsanas_coord::{Directory, NodeClaim, Presence, Registration};
    use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};

    const NOW: u64 = 1_800_000_000;

    fn user(seed: u8) -> UserKeys {
        UserKeys::derive(&MasterSecret::from_bytes([seed; 32]))
    }

    fn device(seed: u8) -> DeviceKeys {
        DeviceKeys::from_seed(&SecretBytes::new([seed; 32]))
    }

    fn register(directory: &Directory, username: &str, owner: &UserKeys) {
        let signed = Registration {
            username: username.to_owned(),
            user: owner.public(),
            issued_unix: NOW,
        }
        .sign(owner);
        directory.register(&signed, NOW).expect("register account");
    }

    fn claim(directory: &Directory, owner: &UserKeys, node: &DeviceKeys) {
        let signed = NodeClaim {
            owner: owner.user_id(),
            device: node.device_id(),
            pledged_bytes: 1024,
            issued_unix: NOW,
            revoked: false,
        }
        .sign(owner);
        directory.claim(&signed, NOW).expect("claim device");
    }

    #[test]
    fn inventory_is_stable_read_only_and_includes_suspended_claims() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("directory.redb");
        let directory = Directory::open(&path).expect("open directory");

        let suspended = user(1);
        let no_devices = user(2);
        let active = user(3);
        register(&directory, "zulu", &active);
        register(&directory, "alpha", &no_devices);
        register(&directory, "bravo", &suspended);

        let with_address = device(11);
        let without_address = device(12);
        let suspended_device = device(13);
        claim(&directory, &active, &with_address);
        claim(&directory, &active, &without_address);
        claim(&directory, &suspended, &suspended_device);
        directory
            .announce(
                &Presence {
                    device: with_address.device_id(),
                    address: "192.0.2.15:9797".to_owned(),
                    at_unix: NOW,
                    head: 4,
                }
                .sign(&with_address),
                NOW,
            )
            .expect("announce address");
        directory
            .suspend(suspended.user_id(), NOW)
            .expect("suspend account");

        drop(directory);
        let before = std::fs::read(&path).expect("read database before inventory");
        assert!(
            Directory::read_inventory(dir.path().join("missing.redb")).is_err(),
            "a missing Directory was treated as an empty inventory"
        );
        let first = inventory_output(&Directory::read_inventory(&path).expect("inventory"));
        let second = inventory_output(&Directory::read_inventory(&path).expect("repeat inventory"));
        let after = std::fs::read(&path).expect("read database after inventory");

        assert_eq!(first, second, "account/device ordering changed");
        assert_eq!(before, after, "inventory modified the Directory");
        assert!(first.find("Account: alpha").unwrap() < first.find("Account: bravo").unwrap());
        assert!(first.find("Account: bravo").unwrap() < first.find("Account: zulu").unwrap());
        assert!(first.contains("Account: bravo ("));
        assert!(first.contains("[SUSPENDED]"));
        assert!(first.contains("no active device claims"));
        assert!(first.contains(&with_address.device_id().to_string()));
        assert!(first.contains(&without_address.device_id().to_string()));
        assert!(first.contains(&suspended_device.device_id().to_string()));
        assert!(first.contains("192.0.2.15:9797"));
        assert!(first.contains("address: unavailable"));
        assert!(first.contains("last seen (Unix seconds): 1800000000"));
        assert!(first.contains("last seen: unavailable"));
    }
}
