//! Talking to the account's machines: `serve`, `daemon`, `sync`, `peer`, and
//! the machines a command dials.

use std::{io::Write as _, path::Path, sync::atomic::AtomicBool};

use itsanas_crypto::DeviceId;
use itsanas_net::{PeerClient, PeerServer, PeerService, Pledge, session};

use crate::cli::PeerAction;
use crate::doctor::{conversion_due, convert_stores, store_roots};
use crate::drive_commands::move_vault;
use crate::leaving::{departure_refusal, refuse_if_departed};
use crate::opening::{PASSPHRASE_ENV, open, passphrase};
use crate::refusals::{deferred_note, describe_refusal};
use crate::{
    config::format_size,
    error::{CliError, Result},
    node::Node,
};
use crate::{coordinator, daemon, jobs, phone, setup};

pub(crate) fn start_daemon(
    home: &Path,
    listen: Option<&str>,
    interval: Option<u64>,
    metered: bool,
    discovery: bool,
) -> Result<()> {
    // Exit 0, not an error: `Restart=on-failure` and the logon task would
    // otherwise start it again every thirty seconds, for ever.
    if let Some(why) = departure_refusal(home) {
        eprintln!("itsanas: {why}");
        return Ok(());
    }
    let interval = interval.map(|seconds| std::time::Duration::from_secs(seconds.max(1)));
    // Sealed, the system asks now; refused, this stops here and nothing runs on
    // an empty passphrase.
    let passphrase = if std::env::var(PASSPHRASE_ENV).map_or(true, |value| value.trim().is_empty())
        && setup::unlock::is_on(home)
    {
        setup::unlock::opened_at_start(home, &setup::unlock::SignIn, setup::unlock::platform_gate)
            .map_err(|why| CliError::Usage(format!("the passphrase stays sealed: {why}")))?
            .to_string()
    } else {
        passphrase(false)?
    };
    jobs::clear_stale(home);
    // Before the store opens: an old store in one file per chunk goes into
    // packs on its own when its disk has room (ROADMAP M10). A failure leaves
    // it as it was and the daemon runs on it.
    if store_roots(home).is_ok_and(|roots| {
        conversion_due(
            &roots,
            std::env::var(itsanas_store::blob::PACKS_ENV).as_deref() == Ok("0"),
            &|root| itsanas_node::pledges::disk_space(root).free,
        )
    }) {
        match convert_stores(home) {
            Ok(said) => eprintln!("itsanas: {said}"),
            Err(error) => {
                eprintln!("itsanas: pack files not made, the store is as it was: {error}");
            }
        }
    }
    // A vault on a removable disk comes and goes. Unplugged, this machine is
    // simply away -- its hosts' audits read it as unreachable, never as a
    // host that lost their data -- and it comes back by itself when the disk
    // does, with no service manager needed to restart it.
    loop {
        if !Node::exists(home) {
            return Err(CliError::NoNode(home.to_path_buf()));
        }
        let node = match Node::open(home, &passphrase) {
            Err(CliError::VaultAbsent(path)) => {
                eprintln!(
                    "itsanas: the vault's disk at {} is not there; this machine syncs its own \
                     files and hosts nobody until it is plugged back in, then carries on by \
                     itself. Nothing is lost meanwhile.",
                    path.display()
                );
                Node::open_away(home, &passphrase)?
            }
            other => other?,
        };
        daemon::run(&node, listen, interval, metered, discovery)?;
        if !daemon::resume_after_vault_pause() {
            return Ok(());
        }
        if let Some(job) = daemon::take_pending_move() {
            let same_disk = matches!(job.kind, jobs::Kind::MoveVault { same_disk: true });
            let done = if job.kind == jobs::Kind::ConvertPacks {
                drop(node);
                convert_stores(home)
            } else {
                move_vault(node, &job.path, same_disk)
            };
            let (state, said) = match done {
                Ok(said) => (jobs::State::Done, said),
                Err(error) => (jobs::State::Failed, error.to_string()),
            };
            eprintln!("itsanas: {said}");
            jobs::report(home, job.id, state, &said);
        }
    }
}

pub(crate) fn serve(home: &Path, listen: Option<&str>) -> Result<()> {
    refuse_if_departed(home)?;
    let node = open(home)?;
    let address = listen.unwrap_or(&node.config.listen);

    let server = PeerServer::bind(address)?;
    let bound = server.local_addr()?;

    // Asks the coordinator about another account's device when it presents
    // (§8 1c (ii)); with none configured, only this account stores here.
    let owners = itsanas_node::owners::ClaimBook::new()
        .asking(node.store.owner(), coordinator::asker(&node));
    let service = PeerService::new(
        &node.store,
        &node.vault,
        Pledge {
            bytes: node.config.pledge_bytes,
        },
    )
    .with_owners(&owners)
    .claiming(node.claim_bytes(itsanas_discover::now_unix()));

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

pub(crate) fn sync(home: &Path, address: Option<&str>, scope: session::Scope) -> Result<()> {
    refuse_if_departed(home)?;
    let node = open(home)?;
    // What the trash holds no longer, by its days or by `trash delete`, leaves
    // every machine with this round, as with the daemon's: before, only the
    // daemon emptied it, so `trash delete` then `sync` sent nothing.
    if let Err(error) = phone::purged_trash(&node) {
        eprintln!(
            "itsanas: the trash could not be emptied: {}",
            error.message()
        );
    }
    // What changed in the folder goes out with this sync, not the next one.
    sync_folder(&node);

    // Configured peers unpinned, as the daemon dials them; the account's other
    // devices from the coordinator pinned, because the coordinator supplies
    // addresses and is not trusted to say who lives at one.
    //
    // Without the second half, a machine just restored with `login --from` had
    // nothing to sync with: `sync` read only `peer add` entries, so the command
    // a person runs after recovering asked them to type an address -- which is
    // what acceptance test A says must never be needed. Only the daemon asked
    // the coordinator.
    let targets = account_targets(&node, address);

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
    let mut refused_by: Vec<String> = Vec::new();

    for (target, pinned) in &targets {
        print!("{target}: ");
        let _ = std::io::stdout().flush();

        let mut client =
            match connect_peer(target.as_str(), &node.device, node.store.owner(), *pinned) {
                Ok(client) => client,
                Err(error) => {
                    // One unreachable peer must not abort the others: the whole
                    // point is that peers come and go.
                    println!("unreachable ({error})");
                    continue;
                }
            };
        daemon::present_claim(&node, target, &mut client);

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
                    deferred_note(&report.pull)
                );
                if let Some(refused) = describe_refusal(&report.push) {
                    println!("  {refused}");
                    refused_by.push(target.clone());
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
    finish_sync(&node, scope, &refused_by);
    Ok(())
}

/// What `sync` says and does once every peer has had its round.
pub(crate) fn finish_sync(node: &Node, scope: session::Scope, refused_by: &[String]) {
    // Said again last, where a person reads: the refusal line sits in the
    // middle of the round's output, and "sync" exiting 0 read as "my files
    // are there" to a persona whose laptop had pledged nothing (§8 q).
    if !refused_by.is_empty() {
        eprintln!(
            "itsanas: not everything was stored: {} refused it (see above); \
             nothing refused is held there until that machine pledges room",
            refused_by.join(", ")
        );
    }
    // And what came in lands in the folder. Without this a person who ran
    // `sync` by hand saw "received 201 files" and an empty folder until an
    // `itsanas scan` no guide names -- found by a persona run of
    // FIRST-STEPS.md (HANDOVER §8 q). Metadata-only fetched no content.
    if matches!(scope, session::Scope::Everything) {
        sync_folder(node);
    }
}

/// The folder half of a sync: the daemon's own reconcile, so the guards of 0l
/// (a missing marker stops, a mass deletion is held) apply here too.
///
/// Refreshes the write bound either way, so that what the round then pulls is
/// held to the disk ceiling (8.1b): `reconcile_once` does it when there is a
/// folder, and without one the ceiling would be the one `Node::open` sets,
/// which is none.
pub(crate) fn sync_folder(node: &Node) {
    let folder = node.config.folder.as_ref().map(|path| {
        itsanas_folder::Folder::open(path)
            .map_err(|error| eprintln!("itsanas: folder {}: {error}", path.display()))
    });
    match folder {
        Some(Ok(folder)) => daemon::reconcile_once(node, &folder, false),
        None | Some(Err(())) => {
            if let Err(error) = node.bound_writes() {
                eprintln!("itsanas: could not work out this disk's room: {error}");
            }
        }
    }
}

/// Type a machine's address into the list this node syncs with, as `peer add`
/// does; says what was done. Shared with the page's Advanced section.
pub(crate) fn add_peer(node: &mut Node, address: &str) -> Result<String> {
    if address.starts_with(itsanas_node::contact::RELAY_PREFIX) {
        return Err(CliError::Usage(format!(
            "{address}: {UNPINNED_RELAY}, and `peer add` has none to give it; \
             the machine's own `host:port`, or the coordinator, which knows it"
        )));
    }
    if node.config.peers.iter().any(|known| known == address) {
        return Ok(format!("{address} is already configured"));
    }
    node.config.peers.push(address.to_owned());
    node.save_config()?;
    Ok(format!("added {address}"))
}

/// Forget a typed address, as `peer remove` does; says what was done.
pub(crate) fn forget_peer(node: &mut Node, address: &str) -> Result<String> {
    let before = node.config.peers.len();
    node.config.peers.retain(|peer| peer != address);
    if node.config.peers.len() == before {
        return Ok(format!("{address} was not configured"));
    }
    node.save_config()?;
    Ok(format!("removed {address}"))
}

pub(crate) fn peer(home: &Path, action: PeerAction) -> Result<()> {
    let mut node = open(home)?;

    match action {
        PeerAction::Add { address } => println!("{}", add_peer(&mut node, &address)?),
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
        PeerAction::Remove { address } => println!("{}", forget_peer(&mut node, &address)?),
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

/// Where a one-shot command (`sync`, `get`) dials when not given an address:
/// `peer add` entries unpinned, as the daemon dials them; the account's other
/// devices from the coordinator, pinned, because it supplies addresses and is
/// not trusted to say who lives at one; then any device of the address book
/// the coordinator did not list (it was unreachable, or none is configured),
/// pinned, a parked one through its relay.
///
/// Without the coordinator half a machine just restored with `login --from`
/// had nothing to sync with (acceptance test A); without the book half `get`
/// on a laptop away from home could reach nobody the coordinator did not
/// answer for (§8 0v).
pub(crate) fn account_targets(
    node: &Node,
    address: Option<&str>,
) -> Vec<(String, Option<DeviceId>)> {
    if let Some(address) = address {
        return vec![(address.to_owned(), None)];
    }
    let listed =
        node.config.coordinator.as_ref().map(|_| {
            coordinator::peers(node, node.store.owner()).map_err(|error| error.to_string())
        });
    if let Some(Err(error)) = &listed {
        // Not fatal: configured peers and the book may still answer.
        println!("coordinator: unreachable ({error})");
    }
    let (book, _) = itsanas_node::contact::Contact::load(
        &itsanas_node::contact::Contact::path(&node.home),
        node.store.owner(),
    );
    peer_targets(
        &node.config.peers,
        &listed.and_then(std::result::Result::ok).unwrap_or_default(),
        &book.candidates(),
        |address| book.through(address),
    )
}

/// [`account_targets`]'s decision, given what it read: the configured peers,
/// what the coordinator listed, the address book's candidates in dialling
/// order and how the book resolves a `relay:` address.
///
/// A `relay:` address is only ever yielded pinned and in the form
/// [`connect_peer`] dials (`relay:<relay>@<where>`); one whose relay the book
/// cannot place is dropped, never dialled as a socket.
pub(crate) fn peer_targets(
    configured: &[String],
    listed: &[(DeviceId, String)],
    book: &[(DeviceId, String)],
    through: impl Fn(&str) -> Option<(DeviceId, String)>,
) -> Vec<(String, Option<DeviceId>)> {
    let relay = itsanas_node::contact::RELAY_PREFIX;
    let mut targets: Vec<(String, Option<DeviceId>)> =
        configured.iter().map(|peer| (peer.clone(), None)).collect();
    let add = |device: DeviceId, address: &str, targets: &mut Vec<(String, Option<DeviceId>)>| {
        let address = if address.starts_with(relay) {
            match through(address) {
                Some((relay_device, at)) => format!("{relay}{}@{at}", relay_device.to_hex()),
                None => return,
            }
        } else {
            address.to_owned()
        };
        if !targets.iter().any(|(known, _)| *known == address) {
            targets.push((address, Some(device)));
        }
    };
    for (device, address) in listed {
        add(*device, address, &mut targets);
    }
    for (device, address) in book {
        if !listed.iter().any(|(known, _)| known == device) {
            add(*device, address, &mut targets);
        }
    }
    targets
}

/// Dial `address`, or, for a machine parked on a relay (`relay:<id>@<at>`,
/// §8 0v), dial it through that relay with `expect` pinned: what the
/// one-shot commands share with the daemon's dialler.
pub(crate) fn connect_peer(
    address: &str,
    device: &itsanas_crypto::DeviceKeys,
    owner: itsanas_crypto::UserId,
    expect: Option<DeviceId>,
) -> itsanas_net::Result<PeerClient> {
    let relayed = address
        .strip_prefix(itsanas_node::contact::RELAY_PREFIX)
        .and_then(|rest| rest.split_once('@'))
        .and_then(|(id, at)| Some((id.parse::<DeviceId>().ok()?, at)));
    match (relayed, expect) {
        (Some((relay, at)), Some(target)) => PeerClient::connect_relayed(
            itsanas_net::AddressList(at),
            relay,
            target,
            device,
            owner,
            None,
        ),
        // A relayed address names a relay, not the machine behind it: dialled
        // as a socket it fails as a name lookup (the open point of 0.1.8),
        // and dialled unpinned through the relay it reaches whoever the relay
        // splices to. The daemon refuses it the same way.
        (Some(_), None) => Err(itsanas_net::NetError::Refused(UNPINNED_RELAY.to_owned())),
        _ => PeerClient::connect(itsanas_net::AddressList(address), device, owner, expect),
    }
}

/// Why a `relay:` address is not dialled without the device it reaches.
pub(crate) const UNPINNED_RELAY: &str = "a relayed address needs the device it reaches";

#[cfg(test)]
mod tests {
    use super::sync_folder;
    use crate::test_fixtures::device;

    /// A laptop away from home with a storage budget, no `peer add` entry,
    /// and the coordinator either answering or not: `get` used to read only
    /// `config.peers` and said "there is no peer to fetch it from" while the
    /// account's machines were listed and in the address book (§8 0v).
    /// Sabotage: `peer_targets` returns the configured peers alone -- red on
    /// the first two assertions.
    #[test]
    fn red_team_a_file_this_machine_lacks_is_fetched_from_the_account_with_no_peer_configured() {
        let (pi, vm) = (device(1), device(2));
        let listed = vec![(pi, "203.0.113.10:9797".to_owned())];
        let targets = super::peer_targets(&[], &listed, &[], |_| None);
        assert_eq!(
            targets,
            vec![("203.0.113.10:9797".to_owned(), Some(pi))],
            "a machine the coordinator listed is not dialled, or dialled unpinned: \
             `get` reaches nobody, or whoever lives at that address"
        );

        // The coordinator unreachable: the book's machines, pinned, a machine
        // it also listed (here none) not twice.
        let book = vec![(vm, "198.51.100.7:9797".to_owned())];
        let targets = super::peer_targets(&[], &[], &book, |_| None);
        assert_eq!(
            targets,
            vec![("198.51.100.7:9797".to_owned(), Some(vm))],
            "with the coordinator down, `get` ignores the address book and reaches nobody"
        );

        // `peer add` entries still come first and stay unpinned, as the daemon
        // dials them; a book device the coordinator listed is not added again.
        let listed = vec![(vm, "198.51.100.7:9797".to_owned())];
        let targets = super::peer_targets(&["192.0.2.5:9797".to_owned()], &listed, &book, |_| None);
        assert_eq!(
            targets,
            vec![
                ("192.0.2.5:9797".to_owned(), None),
                ("198.51.100.7:9797".to_owned(), Some(vm)),
            ]
        );
    }

    /// A parked machine's address is `relay:<relay>`: dialled as a socket it
    /// fails, and dialled unpinned it reaches whoever the relay splices to.
    /// It is yielded only pinned, in the form `connect_peer` dials through
    /// the relay, and dropped when the book cannot place the relay.
    /// Sabotage: yield the `relay:` address as it is -- red.
    #[test]
    fn red_team_a_parked_machine_is_dialled_through_its_relay_and_pinned_or_not_at_all() {
        let (laptop, relay) = (device(3), device(4));
        let parked = format!("{}{}", itsanas_node::contact::RELAY_PREFIX, relay.to_hex());
        let book = vec![(laptop, parked.clone())];
        let placed =
            |address: &str| (address == parked).then(|| (relay, "203.0.113.20:9797".to_owned()));
        let targets = super::peer_targets(&[], &[], &book, placed);
        assert_eq!(
            targets,
            vec![(
                format!(
                    "{}{}@203.0.113.20:9797",
                    itsanas_node::contact::RELAY_PREFIX,
                    relay.to_hex()
                ),
                Some(laptop)
            )],
            "a parked machine is not dialled through its relay with its key pinned"
        );
        let targets = super::peer_targets(&[], &[], &book, |_| None);
        assert!(
            targets.is_empty(),
            "a relayed address whose relay nobody can place was yielded: {targets:?}"
        );
    }

    /// The one-shot commands' dialler, given a relayed address and no device
    /// to pin: it dialled `relay:...` as a host name (the open point of
    /// 0.1.8). Now refused before any socket, as the daemon refuses it.
    /// Sabotage: delete the `(Some(_), None)` arm in `connect_peer` -- the
    /// error becomes a name lookup or a refused connection, red.
    #[test]
    fn red_team_an_unpinned_relay_address_is_refused_not_dialled_as_a_socket() {
        let keys = itsanas_crypto::DeviceKeys::generate().expect("keys");
        let owner = itsanas_crypto::UserId::from_bytes([9; 32]);
        let address = format!(
            "{}{}@127.0.0.1:1",
            itsanas_node::contact::RELAY_PREFIX,
            device(5).to_hex()
        );
        match super::connect_peer(&address, &keys, owner, None) {
            Err(itsanas_net::NetError::Refused(why)) => assert_eq!(why, super::UNPINNED_RELAY),
            Err(other) => panic!("a relayed address with nothing to pin was dialled: {other}"),
            Ok(_) => panic!("a relayed address with nothing to pin connected"),
        }
    }

    /// What a sync pulled must reach the synced folder without a `scan`: the
    /// file is put in the store as a pull would, and `sync_folder` -- what
    /// `sync` runs after its rounds -- must write it out.
    #[test]
    fn red_team_what_a_sync_pulled_lands_in_the_folder() {
        let base = tempfile::tempdir().expect("temp dir");
        let folder = base.path().join("Photos");
        std::fs::create_dir(&folder).expect("folder");
        let (mut node, _phrase) =
            itsanas_node::Node::create(&base.path().join(".itsanas"), "a passphrase", "camille")
                .expect("create");
        node.config.folder = Some(folder.clone());

        node.store
            .write_file("from-the-pi.txt", b"pulled")
            .expect("pulled");
        sync_folder(&node);

        assert_eq!(
            std::fs::read(folder.join("from-the-pi.txt"))
                .ok()
                .as_deref(),
            Some(&b"pulled"[..]),
            "sync fetched the file and left the folder without it"
        );
    }
}
