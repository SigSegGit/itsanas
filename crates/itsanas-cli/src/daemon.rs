//! Running as a service: serve peers and sync on a timer, in one process.
//!
//! Two things need to happen continuously and neither can happen alone. A node
//! that only serves is a host nobody syncs with; a node that only syncs is a
//! peer nobody can reach. So the daemon does both, in one process, against one
//! open store.
//!
//! # Why one process matters more than it looks
//!
//! A node's index is held under an exclusive file lock, so `itsanas serve` and
//! `itsanas sync` cannot run at the same time against the same home — the
//! second refuses to start. Two separate cron entries would therefore fight,
//! and which one won would depend on timing. Doing both from one process is not
//! a convenience; it is the only arrangement that works.
//!
//! It also means the passphrase is entered once and the keys are unlocked once,
//! rather than paying a full Argon2id derivation on every scheduled sync.
//!
//! # The loop
//!
//! ```text
//! reconcile   folder → store, store → folder
//! sync        push and pull with each peer, on the interval
//! reconcile   write out whatever the sync just pulled
//! wait        until the folder changes, or the next sync is due
//! ```
//!
//! Waiting on the *watcher* rather than on a plain sleep is what makes a local
//! edit feel instant while a network round still happens on a timer. The
//! watcher is never trusted on its own: every platform's drops events under
//! load and none of them report changes made while the process was stopped, so
//! a full rescan runs on the interval regardless, and a slower **deep** rescan
//! periodically re-hashes everything to catch what size-and-mtime comparison
//! cannot see.
//!
//! # Finding peers
//!
//! Three ways, in order of how much they need. A third thread announces this
//! node on the local network and records what it hears, so machines in one
//! house find each other with nothing configured. Addresses in the
//! configuration are dialled first, because somebody typed them. And if a
//! coordinator is configured, it is where this node publishes its address and
//! learns where the account's other devices are — the only one of the three
//! that reaches a machine on a different network. Not every round: at start,
//! when the address changes, and hourly (`itsanas_node::contact`).
//!
//! Every candidate is dialled with its device id pinned, so an address that
//! answers as somebody else is refused. Discovery of any kind says who *might*
//! be there; the TLS layer decides who is.
//!
//! # What it does not do yet
//!
//! It does not choose *where* to place data: at a household size the policy is
//! "offer it to every peer this node reaches", which push already does, and
//! choosing between candidates needs a census this node cannot build. Recorded
//! in `docs/ROADMAP.md`.
//!
//! What it does do is the other half of repair, the half push cannot reach: it
//! scans a bounded slice of its own live chunks each round and asks a peer for
//! anything missing from **this** disk. A dropped block or a partial restore
//! used to make a file unreadable until a human ran `doctor` and noticed.
//!
//! It does not *detect* whether its connection is metered. Windows and macOS
//! both expose the answer and reading it would mean a platform crate for one
//! flag, so `--metered` is asked for. Guessing it from the interface type is
//! how a sync tool ends up costing somebody fifty euros, and is refused.

mod app_peers;
mod dialling;
mod disk_work;
mod parking;
mod pledge;
mod reachability;
mod reconciling;
mod round;
mod shutdown;
mod snapshot;

pub(crate) use self::disk_work::take_pending_move;
#[cfg(test)]
pub(crate) use self::reachability::taken_port_message;
pub(crate) use self::reconciling::reconcile_once;
pub(crate) use self::round::present_claim;
#[cfg(test)]
pub(crate) use self::round::refusal_due;
pub(crate) use self::shutdown::restart_asked;
pub(crate) use self::shutdown::resume_after_vault_pause;
#[cfg(test)]
pub(crate) use self::snapshot::OUTAGE_QUIET_FOR_TESTS;

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use itsanas_discover::{HouseholdKey, Lan};
use itsanas_net::PeerServer;
use itsanas_node::contact::{Contact, SharedBoard};
use itsanas_policy::{Attention, Conditions, Network, Power, Scope as PolicyScope};

use self::{
    app_peers::light_round,
    disk_work::between_rounds,
    parking::open_the_router_port,
    pledge::{check_disks, check_standing, disk_room, local, peer_service, probe_vault},
    reachability::{
        Reach, bind_lan, bind_listener, check_reachable, enrol_once, note_inbound, publish,
    },
    reconciling::{open_folder, take_in, wait_for_work},
    round::{decide, one_round, say_if_alone},
    shutdown::{
        SHUTDOWN, StopEverythingOnDrop, install_signal_handler, stop_if_the_vault_came_or_went,
    },
    snapshot::{Outage, Seen, SnapshotPace, last_contact},
};
use crate::{
    config::format_size,
    control::{self, Control, Next, Steering},
    discovery::{self, Neighbourhood},
    error::{CliError, Result},
    node::Node,
};

/// How often to sync when the operator did not say.
///
/// Five minutes is a compromise: short enough that a change made on the laptop
/// is on the Pi before you have walked to it, long enough that three machines
/// polling each other is not a constant background load on a Raspberry Pi.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(300);

/// The interval of a portable copy: a key is plugged in for an hour, not a
/// month, and what is written on it should reach the other machines while it
/// is still there. It hosts nobody, so it answers no audits, and a round of
/// its own is a handful of dials.
pub const PORTABLE_INTERVAL: Duration = Duration::from_secs(60);

/// The interval a daemon runs on: `--interval`, else the policy's, else a
/// portable copy's or the default.
fn base_interval(asked: Option<Duration>, policy: Option<Duration>, portable: bool) -> Duration {
    asked
        .or(policy)
        .unwrap_or(if portable {
            PORTABLE_INTERVAL
        } else {
            DEFAULT_INTERVAL
        })
        .max(Duration::from_secs(1))
}

/// The banner's line for a portable copy. Said whether or not the vault's
/// disk is there: a copy on a key hosts nobody either way. It used to go to
/// stderr before the banner, and only when the vault was present, and a hand
/// test (2026-10-22) never saw it.
fn portable_line(portable: bool) -> Option<&'static str> {
    portable.then_some("  portable  yes: it syncs this account's files and hosts nobody")
}

/// How finely the sleep between rounds is chopped.
///
/// The daemon spends nearly all its life asleep, and a Ctrl-C that took five
/// minutes to be noticed would be indistinguishable from a hang.
const SHUTDOWN_POLL: Duration = Duration::from_millis(200);

/// How often every file is re-hashed rather than trusted by size and mtime.
///
/// The fast path misses a file rewritten within the same second at exactly the
/// same length. Rare, but silent, and the only thing that finds it is reading
/// the bytes. Hourly is cheap enough for a folder of any sane size and short
/// enough that such an edit is not lost for a working day.
const DEEP_SCAN_EVERY: Duration = Duration::from_secs(3600);

/// Longest a continuous stream of file events may postpone a reconcile.
///
/// Copying a large directory in produces events for as long as the copy takes.
/// Waiting for true quiet is correct in principle and indistinguishable from a
/// hang in practice, so the wait is capped and the reconcile runs on what has
/// landed so far. Anything still arriving is caught by the next pass.
const SETTLE_LIMIT: Duration = Duration::from_secs(10);

static UPDATED: AtomicBool = AtomicBool::new(false);

/// One word for what a scope moves, for the banner.
const fn describe(scope: PolicyScope) -> &'static str {
    match scope {
        PolicyScope::Nothing => "nothing",
        PolicyScope::Metadata => "the log only (no file contents)",
        PolicyScope::Everything => "everything",
    }
}

/// Decide how this daemon should behave, and say why.
///
/// The schedule is not a constant here any more: `itsanas-policy` owns it, so
/// the phone, the Mac shell and this daemon reach the same answer from the same
/// argued decision table rather than three copies of a number. A daemon is
/// [`Attention::Unattended`] — nobody is watching it and no platform is
/// restricting it, which is a different thing from a backgrounded app and used
/// to be conflated with one.
///
/// `metered` is asked for rather than detected. Windows and macOS both expose
/// it, and reading it here would mean a platform crate for a flag; until
/// somebody actually runs the daemon on a tether, a flag they pass is honest
/// and a guess from the interface type is not.
#[must_use]
pub fn conditions(metered: bool) -> Conditions {
    Conditions {
        // A machine running a daemon is a machine somebody left on. Claiming
        // `Charging` would be a guess; `OnBattery` is the conservative reading
        // and produces the same plan, while `Low` is the one state that would
        // change it and is the one we cannot see.
        power: Power::OnBattery,
        network: if metered {
            Network::Metered
        } else {
            Network::Unmetered
        },
        attention: Attention::Unattended,
    }
}

/// Run until interrupted.
///
/// `interval` overrides the policy's own schedule when the operator passed
/// `--interval`; otherwise `plan.every` decides.
pub fn run(
    node: &Node,
    listen: Option<&str>,
    interval: Option<Duration>,
    metered: bool,
    discover: bool,
) -> Result<()> {
    let address = listen.unwrap_or(&node.config.listen);
    let server = bind_listener(node, address)?;
    let bound = server.local_addr()?;

    let portable = crate::portable::is_portable(&node.home);
    install_signal_handler()?;
    let shutdown = &SHUTDOWN;

    let lan = bind_lan(discover);
    // Switched off, it answers for what it holds and takes in nothing, as a
    // phone's loop does.
    local(node)
        .answers_only
        .store(!node.config.contribute, Ordering::Relaxed);

    println!("itsanas daemon");
    println!("  serving   {bound}");
    println!("  user id   {}", node.store.owner());
    println!("  device    {}", node.store.device_id());
    println!("  pledged   {}", format_size(node.config.pledge_bytes));
    let plan = itsanas_policy::plan(conditions(metered), itsanas_policy::Settings::default());
    let interval = base_interval(interval, plan.every, portable);
    println!("  interval  {}s", interval.as_secs());
    if let Some(line) = portable_line(portable) {
        println!("{line}");
    }
    println!("  syncing   {}", describe(plan.scope));
    println!("  because   {}", plan.because);
    match &node.config.folder {
        Some(folder) => println!("  folder    {}", folder.display()),
        None => println!("  folder    none configured (`itsanas folder <path>`)"),
    }
    if node.config.peers.is_empty() {
        println!("  peers     none configured");
    } else {
        println!("  peers     {}", node.config.peers.join(", "));
    }
    match &node.config.coordinator {
        Some(address) => println!("  coordinator {address}"),
        None => println!("  coordinator none (`itsanas coordinator <host:port>`)"),
    }
    match &lan {
        Some(lan) => println!(
            "  discovery on, udp {} — machines on this network find each other",
            lan.local_addr()
                .map_or(itsanas_discover::DEFAULT_PORT, |address| address.port())
        ),
        None if discover => println!("  discovery unavailable — peers must be added by hand"),
        None => println!("  discovery off (--no-discovery)"),
    }
    println!();
    println!("Ctrl-C to stop.");
    println!();

    serve_and_round(
        node,
        &server,
        lan.as_ref(),
        Beat {
            base: interval,
            scope: plan.scope,
            shutdown,
            updates_itself: true,
            asks_the_router: true,
        },
    );

    if UPDATED.load(Ordering::SeqCst) {
        // A failure code on purpose: the Windows wrapper, systemd's
        // Restart=on-failure and launchd's KeepAlive all start the program
        // again on one, and only on one -- and the program is now the new one.
        return Err(CliError::Usage(
            "updated; exiting so the background service starts the new version".to_owned(),
        ));
    }
    println!("stopped.");
    Ok(())
}

/// How a run of the loop is paced and stopped, whichever shell runs it.
#[derive(Clone, Copy)]
struct Beat<'a> {
    base: Duration,
    scope: PolicyScope,
    shutdown: &'a AtomicBool,
    /// Whether this program replaces itself. A phone's is installed by its
    /// system, after a person says yes.
    updates_itself: bool,
    /// Whether it may ask the router for a port. A test on a developer's
    /// network must not.
    asks_the_router: bool,
}

/// How a shell that decides when the loop runs wants it run, as
/// `itsanas_policy::hosting` said.
#[derive(Debug)]
pub struct Hosting<'a> {
    pub every: Duration,
    pub scope: PolicyScope,
    /// Answers for what it holds and takes in nothing new.
    pub answers_only: bool,
    /// Beacons and the router's port. Off for a test: on a real network it
    /// would dial the neighbours and open a port on somebody's router.
    pub local_network: bool,
    pub shutdown: &'a AtomicBool,
}

/// The daemon's loop, for a shell with no process of its own to give it: the
/// same listener, park, rounds and audits as `itsanas daemon`. Nothing about a
/// phone reaches the network; only the shell's policy says when this runs.
///
/// # Errors
///
/// If the listener cannot be bound.
pub fn host_until(node: &Node, hosting: &Hosting<'_>) -> Result<()> {
    node.bound_writes()?;
    local(node)
        .answers_only
        .store(hosting.answers_only, Ordering::Relaxed);
    let server = bind_listener(node, &node.config.listen)?;
    let lan = bind_lan(hosting.local_network);
    serve_and_round(
        node,
        &server,
        lan.as_ref(),
        Beat {
            base: hosting.every,
            scope: hosting.scope,
            shutdown: hosting.shutdown,
            updates_itself: false,
            asks_the_router: hosting.local_network,
        },
    );
    Ok(())
}

/// The loop itself: serve, park, publish, round, until told to stop.
fn serve_and_round(node: &Node, server: &PeerServer, lan: Option<&Lan>, beat: Beat<'_>) {
    let Beat {
        base,
        scope: plan_scope,
        shutdown,
        updates_itself,
        asks_the_router,
    } = beat;
    let Ok(bound) = server.local_addr() else {
        eprintln!("itsanas: the listener has no address");
        return;
    };

    // What the listener tells a machine of this account that asks where the
    // others are (`Request::Presences`). The round owns the book; the listener
    // answers from the copy each round leaves here.
    let board = SharedBoard::default();
    let disk_room = disk_room(node);
    let service = peer_service(node, &board, &disk_room);

    let neighbourhood = Neighbourhood::new();

    // Shared with the serving thread, which owns the server for its lifetime.
    let witness = server.witness();

    // Derived from the account's master secret, which only its own machines
    // hold; see `itsanas_discover::beacon`. Never sent.
    let household = HouseholdKey::of(&node.user);

    std::thread::scope(|scope| {
        // If the sync loop panics, the scope joins the listener thread, the
        // listener waits on a shutdown flag the panic skipped, and the process
        // stays alive — serving, never syncing, and looking healthy to
        // systemd. A supervisor that cannot see the difference between a
        // working daemon and a hung one is not supervising anything.
        //
        // Same shape as `StopOnDrop` in the test harnesses. It was written for
        // `itsanas-coord`'s tests, applied nowhere else, and its absence in
        // `itsanas-net` turned every caught attack into a sixty-second timeout.
        let _stop = StopEverythingOnDrop(shutdown);

        scope.spawn(|| {
            if let Err(error) = server.serve_until(&service, &node.device, shutdown) {
                // The sync loop can carry on without the listener; a node that
                // cannot accept connections can still push to its peers. Say so
                // loudly rather than exiting and taking sync down with it.
                eprintln!("itsanas: the listener stopped: {error}");
            }
        });

        if updates_itself {
            scope.spawn(|| crate::update::watch(&node.home, shutdown, &UPDATED));
        }

        // A key copied while its coordinator could not be reached, or was
        // full, enrols here, once (`copy_to_key.rs`). Beside the rounds,
        // which need no coordinator to start.
        scope.spawn(move || enrol_once(node, bound));

        open_the_router_port(
            scope,
            node,
            asks_the_router.then(|| bound.port()),
            (server, &service, &board),
            shutdown,
        );
        if let Some(lan) = lan {
            scope.spawn(|| {
                discovery::run(
                    lan,
                    &node.device,
                    &household,
                    bound.port(),
                    &neighbourhood,
                    shutdown,
                );
            });
        }

        sync_loop(
            node,
            base,
            plan_scope,
            shutdown,
            &neighbourhood,
            bound,
            Some(&witness),
            &board,
        );
    });
}

/// Reconcile the folder, sync with peers, and wait for whichever comes first.
#[allow(clippy::too_many_arguments)]
fn sync_loop(
    node: &Node,
    base: Duration,
    scope: PolicyScope,
    shutdown: &AtomicBool,
    neighbourhood: &Neighbourhood,
    bound: std::net::SocketAddr,
    witness: Option<&itsanas_net::transport::Witness>,
    board: &SharedBoard,
) {
    let folder = match open_folder(node) {
        Ok(folder) => folder,
        Err(error) => {
            eprintln!("itsanas: {error}");
            eprintln!("itsanas: continuing without a synced folder");
            None
        }
    };

    let mut steering = start_steering(node);
    // No round yet, so the first is due at once rather than a full interval
    // from now. A daemon restarted after a config change should act on it
    // now, and someone watching the first run should see something happen.
    let mut last_round: Option<Instant> = None;
    // The last full round: light ones (application mode) do not put it off.
    let mut last_full: Option<Instant> = None;
    let mut next_deep = Instant::now();
    let mut warned_alone = false;
    let mut outage = Outage::new();
    let mut reach = Reach::new();
    // The address book outlives the process: it remembers which address of
    // each machine worked, and that this coordinator signs its list, so a
    // restart does not re-open the downgrade to an unsigned one.
    let book = Contact::path(&node.home);
    let (mut contact, unreadable) = Contact::load(&book, node.store.owner());
    if let Some(why) = unreadable {
        eprintln!("itsanas: {why}");
    }
    board.replace(contact.board());
    // Where this process's pass over its own chunks stands. In memory: losing
    // it at a restart costs a pass started again, and the vault, whose rows
    // other people's rounds rely on, keeps its cursor on disk.
    let mut own_check: Option<itsanas_crypto::ChunkId> = None;
    let mut snap = SnapshotPace::default();
    let mut seen = Seen::default();

    while !shutdown.load(Ordering::Relaxed) {
        for said in steering.refresh(Control::read(&node.home), itsanas_discover::now_unix()) {
            println!("{said}");
        }
        let interval = steering.interval(base);
        // Paused, nothing touches the folder or the store: edits made
        // meanwhile are found by the first scan after `resume`, and what peers
        // pushed waits in the vault, where it already is safe.
        let paused = steering.paused();
        let deep = !paused && Instant::now() >= next_deep;
        if deep {
            next_deep = Instant::now() + DEEP_SCAN_EVERY;
        }
        let changed_here = !paused && take_in(node, folder.as_ref(), deep);
        if changed_here {
            steering.changed_here(Instant::now());
        }

        let next = decide(node, &mut steering, scope, (last_round, last_full), base);
        if next == Next::Light {
            light_round(node, shutdown, folder.as_ref(), scope);
            last_round = Some(Instant::now());
        }
        if next == Next::Publish {
            paused_round(node, shutdown, &mut contact, bound, &mut outage);
            board.replace(contact.board());
            if let Err(error) = contact.save(&book) {
                eprintln!("itsanas: could not keep the address book: {error}");
            }
            last_round = Some(Instant::now());
        }
        if next == Next::Round {
            let (reached, in_touch, announced) = one_round(
                node,
                shutdown,
                neighbourhood,
                &mut contact,
                bound,
                &mut outage,
                scope,
                folder.as_ref().map(|(folder, _)| folder),
                interval,
            );
            last_round = Some(Instant::now());
            last_full = last_round;
            // What `isolated` ages (`steering.rs`'s
            // red_team_a_round_that_reaches_the_account_stamps_the_contact).
            seen.reached = last_contact(in_touch, seen.reached);
            board.replace(contact.board());
            // A pause asked for during the round: `halted` stopped the dialling,
            // and nothing after it may touch the folder or the store either.
            let paused_since_round = control::paused_on_disk(&node.home);
            if !paused_since_round {
                check_disks(node, interval, &mut own_check);
                probe_vault(node, &mut seen.unwritable);
            }
            if let Err(error) = contact.save(&book) {
                eprintln!("itsanas: could not keep the address book: {error}");
            }

            // Free evidence first, then the paid kind: if something from
            // outside has already reached this machine, the way in works and
            // nothing needs to be asked of anybody.
            if let Some(witness) = witness {
                note_inbound(witness, &mut reach);
            }
            if let Some(published) = &announced {
                check_reachable(node, published, &mut reach);
            }

            say_if_alone(node, neighbourhood, &reached, &mut warned_alone);

            // Write out whatever just arrived, rather than making the user
            // wait for the next loop to see their peer's changes.
            if let Some((folder, _)) = folder.as_ref().filter(|_| !paused_since_round) {
                reconcile_once(node, folder, false);
            }
        }

        snap.after(
            node,
            interval,
            bound,
            changed_here || next != Next::Wait,
            &seen,
        );

        let next_sync = last_round.map_or_else(Instant::now, |last| last + interval);
        wait_for_work(folder.as_ref(), next_sync, shutdown);
        between_rounds(node, shutdown);
        stop_if_the_vault_came_or_went(node, shutdown);
    }
}

/// What the person asked through `itsanas pause`, `resume`, `sync-now` and
/// `interval`, from a terminal or a tray, while this process holds the store
/// (`control.rs`).
fn start_steering(node: &Node) -> Steering {
    let now = itsanas_discover::now_unix();
    let (steering, unreadable) = Steering::start(Control::read(&node.home), now);
    if let Some(why) = unreadable {
        eprintln!("itsanas: {why}");
    }
    if steering.paused() {
        println!(
            "syncing is paused {} (`itsanas resume` to continue); this machine still hosts",
            steering.lasts(now)
        );
    }
    steering
}

/// A round while paused. The others still need to find this machine: it holds
/// their data, and answering their audits is what keeps its standing. So it
/// says where it is, and confirms who it hosts for, and moves no file.
fn paused_round(
    node: &Node,
    shutdown: &AtomicBool,
    contact: &mut Contact,
    bound: std::net::SocketAddr,
    outage: &mut Outage,
) {
    publish(node, shutdown, contact, bound, outage);
    if !shutdown.load(Ordering::Relaxed) {
        check_standing(node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device id made of one repeated byte, for the parking and dialling
    /// tests.
    pub(super) fn id(byte: u8) -> itsanas_crypto::DeviceId {
        itsanas_crypto::DeviceId::from_bytes([byte; 32])
    }

    #[test]
    fn the_default_interval_is_neither_a_busy_loop_nor_an_hour() {
        // Too short and three machines polling each other is a constant load on
        // a Raspberry Pi; too long and the thing feels broken.
        assert!(DEFAULT_INTERVAL >= Duration::from_secs(60));
        assert!(DEFAULT_INTERVAL <= Duration::from_secs(900));
    }

    /// A key is plugged in for an hour: its rounds come every minute, unless
    /// the operator or the policy chose otherwise.
    #[test]
    fn a_portable_copy_syncs_every_minute_unless_told_otherwise() {
        assert_eq!(base_interval(None, None, true), PORTABLE_INTERVAL);
        assert_eq!(PORTABLE_INTERVAL, Duration::from_secs(60));
        assert_eq!(base_interval(None, None, false), DEFAULT_INTERVAL);
        let asked = Duration::from_secs(7);
        assert_eq!(base_interval(Some(asked), None, true), asked);
        let policy = Duration::from_secs(900);
        assert_eq!(base_interval(None, Some(policy), true), policy);
    }

    /// The banner says a portable copy is one, on stdout with the rest.
    #[test]
    fn a_portable_copy_says_so_in_its_banner() {
        assert!(portable_line(true).is_some_and(|line| line.contains("hosts nobody")));
        assert_eq!(portable_line(false), None);
    }
}
