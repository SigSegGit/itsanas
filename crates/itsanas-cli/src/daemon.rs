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

use std::fmt::Write as _;
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use itsanas_crypto::DeviceId;
use itsanas_discover::{HouseholdKey, Lan};
use itsanas_folder::{Folder, Watcher, watch};
use itsanas_net::{PeerClient, PeerServer, PeerService, Pledge, session};
use itsanas_node::contact::{Contact, SharedBoard};
use itsanas_policy::{Attention, Conditions, Network, Power, Scope as PolicyScope};

use crate::{
    config::format_size,
    control::{self, Control, Next, Steering},
    coordinator,
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

/// The longest the snapshot goes unwritten between rounds, or a round when
/// that is shorter ([`snapshot_due`]).
///
/// Its stamp is what says the daemon is alive: an icon reads `stale` past two
/// intervals (`brief_status`), the fleet's S row reads its age, and the
/// steering tests wait for a newer one as proof a pass of the loop went by.
/// Half a minute keeps all three; every pass of the loop (two seconds) cost an
/// idle node most of its disk writes (2026-10-27).
const SNAPSHOT_EVERY: Duration = Duration::from_secs(30);

/// When the snapshot was last written, and for which interval: what
/// [`snapshot_due`] decides from, once a pass of the loop.
#[derive(Default)]
struct SnapshotPace(Option<(Instant, Duration)>);

impl SnapshotPace {
    /// Write the snapshot if it is due; `news` when this pass ran a round or
    /// took in a change.
    fn after(&mut self, node: &Node, interval: Duration, bound: std::net::SocketAddr, news: bool) {
        let now = Instant::now();
        if snapshot_due(self.0, now, interval, news) {
            write_snapshot(node, interval, bound);
            self.0 = Some((now, interval));
        }
    }
}

/// Whether this pass of the loop writes the snapshot: the first time, when
/// `news` (a round ran, the folder changed), when the interval is not the one
/// last written (the stamp is read against it), and otherwise once
/// [`SNAPSHOT_EVERY`] -- or a round, if shorter -- has passed since the last.
/// `last` is when the last one was written and for which interval.
fn snapshot_due(
    last: Option<(Instant, Duration)>,
    now: Instant,
    interval: Duration,
    news: bool,
) -> bool {
    let Some((at, written_for)) = last else {
        return true;
    };
    news || written_for != interval
        || now.saturating_duration_since(at) >= SNAPSHOT_EVERY.min(interval)
}

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

/// How long a continuing coordinator outage stays quiet between complaints.
///
/// A coordinator that is down is down for hours, not for one round. Saying so
/// every round produces roughly six hundred identical lines a day, and a
/// journal nobody reads is the same as no journal on the morning something else
/// breaks. Found by running the daemon against a coordinator that was not
/// there, which is exactly the state MVP acceptance test I describes as normal.
const OUTAGE_QUIET: Duration = Duration::from_secs(30 * 60);

/// [`OUTAGE_QUIET`] for tests outside this module, which cannot name a private
/// constant.
#[cfg(test)]
pub(crate) const OUTAGE_QUIET_FOR_TESTS: Duration = OUTAGE_QUIET;

/// What has already been said about the coordinator, so it is not said again.
#[derive(Debug)]
struct Outage {
    /// Rounds that have failed since the last time anything was printed.
    silent_rounds: usize,
    /// When the next complaint is allowed.
    next_complaint: Instant,
    /// Whether the last round reached it.
    was_reachable: bool,
}

impl Outage {
    fn new() -> Self {
        Self {
            silent_rounds: 0,
            next_complaint: Instant::now(),
            was_reachable: true,
        }
    }

    /// Report a failed round, at most once per [`OUTAGE_QUIET`].
    fn failed(&mut self, why: &str) {
        self.silent_rounds += 1;
        if Instant::now() < self.next_complaint {
            return;
        }
        if self.silent_rounds == 1 {
            println!("coordinator: unreachable ({why})");
            println!("  Peers already known keep syncing. New machines cannot be found.");
        } else {
            println!(
                "coordinator: still unreachable after {} rounds ({why})",
                self.silent_rounds
            );
        }
        self.next_complaint = Instant::now() + OUTAGE_QUIET;
        self.was_reachable = false;
    }

    /// Report a round that reached it, but only if the last one did not.
    fn succeeded(&mut self) {
        if !self.was_reachable {
            println!("coordinator: reachable again");
        }
        self.silent_rounds = 0;
        self.next_complaint = Instant::now();
        self.was_reachable = true;
    }
}

/// Raises the shutdown flag however the scope is left, panic included.
///
/// See the comment at the `thread::scope` in [`run`]: without it a panic in the
/// sync loop leaves the listener thread waiting on a flag nobody set, and the
/// process hangs instead of dying.
struct StopEverythingOnDrop<'a>(&'a AtomicBool);

impl Drop for StopEverythingOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Set by the interrupt handler.
///
/// A process-wide `static` rather than something threaded through, because the
/// signal handler is process-wide: it is installed once, outlives any particular
/// call, and cannot borrow a local.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Set with [`SHUTDOWN`] when a run stops because the vault's disk went, not
/// because somebody asked: `itsanas daemon` then waits for the disk and runs
/// again, instead of exiting.
static PAUSED_FOR_VAULT: AtomicBool = AtomicBool::new(false);

/// Whether the run that just returned stopped for the vault's disk; clears
/// the stop so the next run starts. A Ctrl-C during the pause still wins.
pub(crate) fn resume_after_vault_pause() -> bool {
    if PAUSED_FOR_VAULT.swap(false, Ordering::SeqCst) {
        SHUTDOWN.store(false, Ordering::SeqCst);
        return true;
    }
    false
}

/// Set by the daily update look when it installed a new program: the daemon
/// then exits with a failure code so its service starts the new one.
/// Set when this run stopped because `itsanas update` asked for a restart
/// (`lifecycle.rs`): `main` then exits with the code that restarts it.
static RESTART_ASKED: AtomicBool = AtomicBool::new(false);

/// Whether the run that just ended was asked to restart.
pub(crate) fn restart_asked() -> bool {
    RESTART_ASKED.load(Ordering::SeqCst)
}

/// A stop or restart asked through the home (`lifecycle.rs`), acted on
/// between two rounds.
fn lifecycle_request(node: &Node, shutdown: &AtomicBool) {
    match crate::lifecycle::take(&node.home) {
        Some(crate::lifecycle::Request::Stop) => {
            eprintln!("itsanas: asked to stop; stopping");
            shutdown.store(true, Ordering::SeqCst);
        }
        Some(crate::lifecycle::Request::Restart) => {
            eprintln!("itsanas: asked to restart on the program now installed; restarting");
            RESTART_ASKED.store(true, Ordering::SeqCst);
            shutdown.store(true, Ordering::SeqCst);
        }
        None => {}
    }
}

/// What is asked through the home between two rounds: a disk job, then a
/// stop or a restart.
fn between_rounds(node: &Node, shutdown: &AtomicBool) {
    run_disk_job(node, shutdown);
    lifecycle_request(node, shutdown);
}

/// A vault move asked of this run (`jobs.rs`): done by `start_daemon` once
/// the run has ended and the node is closed, then the daemon starts again.
static PENDING_MOVE: std::sync::Mutex<Option<crate::jobs::Job>> = std::sync::Mutex::new(None);

/// The move this run was stopped for, if any.
pub(crate) fn take_pending_move() -> Option<crate::jobs::Job> {
    PENDING_MOVE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

/// Do the disk job waiting in the home, if any (`jobs.rs`). An export or a
/// restore runs here, on the open node; a move stops this run, as a disk
/// that went away does, and `start_daemon` does it with the node closed.
fn run_disk_job(node: &Node, shutdown: &AtomicBool) {
    use crate::jobs::{self, Kind, State};
    let Some(job) = jobs::take(&node.home) else {
        return;
    };
    let job = match job {
        Ok(job) => job,
        Err(why) => {
            eprintln!("itsanas: {why}");
            return;
        }
    };
    let target = job.path.display().to_string();
    let outcome = match job.kind {
        Kind::MoveVault { .. } if node.away => {
            Err("the vault's disk is away; plug it back in before moving the vault".to_owned())
        }
        Kind::MoveVault { .. } => {
            jobs::report(
                &node.home,
                job.id,
                State::Running,
                &format!("moving the vault to {target}"),
            );
            eprintln!("itsanas: moving the vault to {target}; this run stops for the copy");
            *PENDING_MOVE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(job);
            PAUSED_FOR_VAULT.store(true, Ordering::SeqCst);
            shutdown.store(true, Ordering::SeqCst);
            return;
        }
        Kind::Export => {
            jobs::report(
                &node.home,
                job.id,
                State::Running,
                &format!("writing a backup to {target}"),
            );
            crate::backup::export(node, &job.path)
                .map(|moved| {
                    format!(
                        "backed up {} segments and {} chunks ({}) to {target}; sealed: the 24 \
                         words open it, keep them apart",
                        moved.segments,
                        moved.chunks,
                        itsanas_node::config::format_size(moved.bytes)
                    ) + &crate::drives::warning_for(&job.path, "the backup")
                        .map(|warning| format!(". Note: {warning}"))
                        .unwrap_or_default()
                })
                .map_err(|error| error.to_string())
        }
        Kind::Restore => {
            jobs::report(
                &node.home,
                job.id,
                State::Running,
                &format!("reading the backup at {target}"),
            );
            crate::backup::restore(node, &job.path)
                .map(|(moved, report)| {
                    format!(
                        "read {} segments and {} chunks ({}) from {target}; {} file(s) adopted",
                        moved.segments,
                        moved.chunks,
                        itsanas_node::config::format_size(moved.bytes),
                        report.adopted
                    )
                })
                .map_err(|error| error.to_string())
        }
    };
    match outcome {
        Ok(said) => {
            eprintln!("itsanas: {said}");
            jobs::report(&node.home, job.id, State::Done, &said);
        }
        Err(why) => {
            eprintln!("itsanas: the disk job on {target} failed: {why}");
            jobs::report(&node.home, job.id, State::Failed, &why);
        }
    }
}

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

/// The home router's port, opened by `UPnP` when nothing was written: what
/// makes a machine behind a box reachable without anybody touching the box.
/// On its own thread, so a silent router costs the start nothing; the
/// address found is announced from the next round.
/// And, beside it, the park on a relay while no port is open (§8 0v).
fn open_the_router_port<'scope>(
    scope: &'scope std::thread::Scope<'scope, '_>,
    node: &'scope Node,
    port: u16,
    parking: (
        &'scope PeerServer,
        &'scope itsanas_net::PeerService<'scope>,
        &'scope SharedBoard,
    ),
    shutdown: &'scope AtomicBool,
) {
    let (server, service, board) = parking;
    scope.spawn(move || park_while_unreachable(node, server, service, board, shutdown));
    if node.config.announce.is_some()
        || node.config.coordinator.is_none()
        || std::env::var_os("ITSANAS_NO_UPNP").is_some()
    {
        return;
    }
    open_router(scope, port, shutdown);
}

fn open_router<'scope>(
    scope: &'scope std::thread::Scope<'scope, '_>,
    port: u16,
    shutdown: &'scope AtomicBool,
) {
    scope.spawn(move || match crate::upnp::open_port(port) {
        Ok((gateway, address)) => {
            println!("itsanas: the router opened port {port} (UPnP); reachable at {address}");
            coordinator::set_found_address(Some(address));
            crate::upnp::renew(&gateway, port, shutdown);
        }
        // A router that speaks no UPnP may speak NAT-PMP (HANDOVER §8 0u).
        Err(why) => match crate::natpmp::open_port(port) {
            Ok(mapping) => {
                println!(
                    "itsanas: the router opened port {port} (NAT-PMP); reachable at {}",
                    mapping.address
                );
                coordinator::set_found_address(Some(mapping.address.clone()));
                crate::natpmp::renew(mapping, port, shutdown, |address| {
                    println!("itsanas: the router's address changed; now reachable at {address}");
                    coordinator::set_found_address(Some(address));
                });
            }
            Err(also) => {
                println!(
                    "itsanas: no port opened on the router ({why}; {also}); this machine dials out"
                );
            }
        },
    });
}

/// How long the parking thread waits between looks: for the router's answer
/// at the start, for a relay to be known, after a refused park.
const PARK_PAUSE: Duration = Duration::from_secs(60);

/// Park on a machine of the account that can be reached, for as long as this
/// one cannot (§8 0v): no `announce`, no port the router opened. A caller who
/// cannot dial this machine reaches it through the park; what it says over it
/// is sealed end to end. Off with no coordinator, or `ITSANAS_NO_RELAY`.
fn park_while_unreachable(
    node: &Node,
    server: &PeerServer,
    service: &itsanas_net::PeerService<'_>,
    board: &SharedBoard,
    shutdown: &AtomicBool,
) {
    if node.config.announce.is_some()
        || node.config.coordinator.is_none()
        || std::env::var_os("ITSANAS_NO_RELAY").is_some()
    {
        return;
    }
    let site = Parking {
        node,
        server,
        service,
        board,
        shutdown,
    };
    park_loop(&site, node.store.device_id(), shutdown, || {
        pause(PARK_PAUSE, shutdown)
    });
}

/// What the parking loop needs from the world, so its decisions can be
/// driven by a test without a socket, a coordinator or a minute per look.
trait ParkSite {
    /// Whether nothing can dial this machine directly ([`fn@unreachable`]).
    fn unreachable(&self) -> bool;
    /// The relays the book knows, best first.
    fn relays(&self) -> Vec<(DeviceId, String)>;
    /// Park on `relay` until it hands over a caller (`Ok(true)`), goes quiet
    /// or closes the park (`Ok(false)`), or refuses it (`Err`).
    fn park(&self, relay: DeviceId, address: &str) -> itsanas_net::Result<bool>;
    /// The relay this machine announces itself behind, if any.
    fn announce(&self, relay: Option<(DeviceId, String)>);
}

/// The daemon's [`ParkSite`]: its listener, its book, the coordinator's
/// announcement.
struct Parking<'a> {
    node: &'a Node,
    server: &'a PeerServer,
    service: &'a itsanas_net::PeerService<'a>,
    board: &'a SharedBoard,
    shutdown: &'a AtomicBool,
}

impl ParkSite for Parking<'_> {
    fn unreachable(&self) -> bool {
        unreachable(self.node, self.server)
    }

    fn relays(&self) -> Vec<(DeviceId, String)> {
        self.board.relays()
    }

    fn park(&self, relay: DeviceId, address: &str) -> itsanas_net::Result<bool> {
        self.server.serve_parked(
            itsanas_net::AddressList(address),
            relay,
            self.service,
            &self.node.device,
            self.shutdown,
        )
    }

    fn announce(&self, relay: Option<(DeviceId, String)>) {
        coordinator::set_parked_on(relay);
    }
}

/// The parking loop: one look per `wait()` (which says `false` once
/// stopping), parked for as long as this machine is unreachable.
fn park_loop(
    site: &impl ParkSite,
    me: DeviceId,
    shutdown: &AtomicBool,
    mut wait: impl FnMut() -> bool,
) {
    let mut said: Option<String> = None;
    // Relays that refused or closed the park, skipped until every one has.
    let mut failed: BTreeSet<DeviceId> = BTreeSet::new();
    while wait() {
        if !site.unreachable() {
            continue;
        }
        let choices: Vec<_> = site
            .relays()
            .into_iter()
            .filter(|(d, _)| *d != me)
            .collect();
        if choices.iter().all(|(d, _)| failed.contains(d)) {
            failed.clear();
        }
        let Some((relay, address)) = choices.into_iter().find(|(d, _)| !failed.contains(d)) else {
            continue;
        };
        // Parked again after each caller, for as long as this machine is
        // still unreachable and the relay still the book's.
        while site.unreachable()
            && site
                .relays()
                .iter()
                .any(|(d, a)| *d == relay && *a == address)
        {
            // Kept set across re-parks, so the address announced does not
            // flip at every caller; cleared once the park is given up.
            site.announce(Some((relay, address.clone())));
            match site.park(relay, &address) {
                Ok(true) if !shutdown.load(Ordering::Relaxed) => {}
                Ok(true) => return,
                Ok(false) => {
                    site.announce(None);
                    failed.insert(relay);
                    break;
                }
                Err(error) => {
                    site.announce(None);
                    failed.insert(relay);
                    let line = format!("itsanas: could not park on {address} ({error})");
                    if said.as_deref() != Some(&line) {
                        println!("{line}");
                        said = Some(line);
                    }
                    break;
                }
            }
        }
        site.announce(None);
    }
}

/// Whether nothing can dial this machine directly: no port the router
/// opened, no caller from outside its network ever arrived, and the address
/// it would publish is a private one. A machine with a public address, or a
/// forward somebody made by hand, never parks: a relay is slower than a
/// direct path, and an address replaced by `relay:` is one same-LAN
/// machines would hairpin through.
fn unreachable(node: &Node, server: &PeerServer) -> bool {
    let listen = server
        .local_addr()
        .map(|bound| bound.to_string())
        .unwrap_or_default();
    coordinator::found_address().is_none()
        && server.witness().from_outside() == 0
        && coordinator::direct_address(&node.config, &listen)
            .is_none_or(|address| itsanas_node::coordinator::is_private_address(&address))
}

/// Sleep `length`, a second at a time; `false` once stopping.
fn pause(length: Duration, shutdown: &AtomicBool) -> bool {
    let until = Instant::now() + length;
    while Instant::now() < until {
        if shutdown.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(Duration::from_secs(1).min(until - Instant::now()));
    }
    !shutdown.load(Ordering::Relaxed)
}

/// The LAN discovery socket, if wanted and free.
fn bind_lan(discover: bool) -> Option<Lan> {
    // Discovery is an optimisation, so a failure to bind is a warning and not
    // an exit. The commonest cause is a second node on the same machine, which
    // is a test rig rather than a mistake, and it must not stop the daemon that
    // can still sync with its configured peers.
    if discover {
        // `ITSANAS_DISCOVERY_PORT` moves a test rig off 21037, so two daemons a
        // test starts hear each other and not a real node on the same machine
        // (`tests/route.rs`). Anything that is not a port is the default.
        let port = std::env::var("ITSANAS_DISCOVERY_PORT")
            .ok()
            .and_then(|port| port.parse::<u16>().ok())
            .filter(|port| *port != 0)
            .unwrap_or(itsanas_discover::DEFAULT_PORT);
        match Lan::bind(port) {
            Ok(lan) => Some(lan),
            Err(error) => {
                eprintln!(
                    "itsanas: local discovery is off ({error}). Peers must be added by hand."
                );
                None
            }
        }
    } else {
        None
    }
}

/// A key's enrolment at its first start, when the copy could not enrol it
/// (`copy_to_key::enrol_at_start`); prints what happened.
///
/// Registers without announcing the configured `listen`, then announces
/// `bound`, the address this daemon really listens on (red team,
/// 2026-10-23): run beside the rounds' own publication, announcing the
/// configured port of a daemon started with `--listen` elsewhere sent the
/// account's machines to a port nobody was on until the next publication.
fn enrol_once(node: &Node, bound: std::net::SocketAddr) {
    let enrolled = enrol_at_daemon_start(node, &bound.to_string(), |node, listen| {
        crate::register_only(node)?;
        // Best effort: the rounds publish this address too, and keep at it
        // until the coordinator takes it.
        if let Err(error) = coordinator::announce(node, listen, itsanas_discover::now_unix()) {
            eprintln!("itsanas: enrolled, but could not announce {listen} yet: {error}");
        }
        Ok(())
    });
    if let Some(line) = enrolled {
        println!("{line}");
    }
}

/// [`enrol_once`]'s decision, with the enrolment given: `enrol` is handed the
/// node and `bound`, the address to announce.
///
/// A `registered` marker naming another coordinator than the configured one
/// is stale -- `itsanas coordinator` pointed this node elsewhere since -- and
/// is dropped, so the node enrols with the one it now uses (red team,
/// 2026-10-23); `copy_to_key::enrol_at_start` then does the rest.
fn enrol_at_daemon_start(
    node: &Node,
    bound: &str,
    enrol: impl FnOnce(&Node, &str) -> Result<()>,
) -> Option<String> {
    if let Some(configured) = &node.config.coordinator
        && crate::registered_with(&node.home).is_some_and(|marked| marked != *configured)
    {
        let _ = std::fs::remove_file(node.home.join(crate::REGISTERED));
    }
    crate::copy_to_key::enrol_at_start(node, |node| enrol(node, bound))
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

    // What the listener tells a machine of this account that asks where the
    // others are (`Request::Presences`). The round owns the book; the listener
    // answers from the copy each round leaves here.
    let board = SharedBoard::default();
    let disk_room = disk_room(node.vault_dir());
    let service = peer_service(node, &board, &disk_room);

    let portable = crate::portable::is_portable(&node.home);
    install_signal_handler()?;
    let shutdown = &SHUTDOWN;

    let neighbourhood = Neighbourhood::new();

    let lan = bind_lan(discover);

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

        scope.spawn(|| crate::update::watch(&node.home, shutdown, &UPDATED));

        // A key copied while its coordinator could not be reached, or was
        // full, enrols here, once (`copy_to_key.rs`). Beside the rounds,
        // which need no coordinator to start.
        scope.spawn(move || enrol_once(node, bound));

        open_the_router_port(
            scope,
            node,
            bound.port(),
            (&server, &service, &board),
            shutdown,
        );
        if let Some(lan) = &lan {
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
            interval,
            plan.scope,
            shutdown,
            &neighbourhood,
            bound,
            Some(&witness),
            &board,
        );
    });

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

/// What this daemon knows about being reachable, and what it has already said.
///
/// Three facts, none of which a node can work out alone, and the rule for when
/// it is worth asking again. A machine that cannot be reached is not broken --
/// a laptop never can be -- so none of this is an error; it is the difference
/// between a member who knows their forward is wrong and one who thinks the
/// network is quiet.
struct Reach {
    /// The address published on the last round that reached the coordinator.
    published: Option<String>,
    /// Whether the coordinator has said it could reach us here.
    verdict: Option<bool>,
    /// Connections from outside at the last check, so a first one is news.
    seen_from_outside: u64,
}

impl Reach {
    const fn new() -> Self {
        Self {
            published: None,
            verdict: None,
            seen_from_outside: 0,
        }
    }

    /// Whether the coordinator should be asked to try reaching us.
    ///
    /// Only when the answer can have changed: the address is new, or it is the
    /// first round of this daemon and nobody has ever said. A probe costs the
    /// coordinator an outbound connection, and a fleet that asked every round
    /// would turn a diagnostic into the load it is meant to prevent.
    fn should_ask(&self, published: &str) -> bool {
        self.verdict.is_none() || self.published.as_deref() != Some(published)
    }
}

/// Ask the coordinator to try reaching this machine, and say what it found.
///
/// Said once per answer, not once per round: the line matters the first time
/// and is noise the twentieth.
fn check_reachable(node: &Node, published: &str, reach: &mut Reach) {
    if !reach.should_ask(published) {
        return;
    }
    reach.published = Some(published.to_owned());

    match coordinator::check_me(node) {
        Ok(answer) => {
            let found = coordinator::found_address();
            let heard = hear_reachability(answer, published, found.as_deref(), reach);
            for line in &heard.lines {
                println!("{line}");
            }
            if heard.withdraw_found {
                coordinator::set_found_address(None);
            }
        }
        Err(error) => eprintln!("itsanas: could not ask whether we are reachable: {error}"),
    }
}

/// What the daemon does with the coordinator's answer about `published`.
struct Heard {
    /// What to print, already said once per verdict.
    lines: Vec<String>,
    /// Whether the address the router said it opened stops being published.
    withdraw_found: bool,
}

/// [`check_reachable`]'s decisions, without the coordinator or the statics.
///
/// `found` is the address `UPnP` or NAT-PMP gave (`None` with an `announce`,
/// which is never found). When the coordinator could not reach this device
/// there, the router's yes was not worth what it said: a hand-made forward of
/// the same port to another machine wins over the router's own mapping (hand
/// test 2026-10-22: the home address's port 9799 answered as the VM), or a
/// firewall drops what the router lets in. Kept, that address sends every
/// member to the wrong machine at every round, and it is also what tells
/// [`fn@unreachable`] that this machine needs no relay. Withdrawn, the next
/// publication is the honest local one and the parking thread finds a relay.
/// For the rest of this run: a NAT-PMP renewal that reports a new address
/// publishes it again, and the new address is probed in its turn.
fn hear_reachability(
    answer: Option<coordinator::Reachability>,
    published: &str,
    found: Option<&str>,
    reach: &mut Reach,
) -> Heard {
    let mut heard = Heard {
        lines: Vec::new(),
        withdraw_found: false,
    };
    match answer {
        Some(coordinator::Reachability::Reachable(detail)) => {
            if reach.verdict != Some(true) {
                heard.lines.push(format!("reachable: {detail}"));
            }
            reach.verdict = Some(true);
        }
        Some(coordinator::Reachability::Unreachable(detail)) => {
            if reach.verdict != Some(false) {
                heard
                    .lines
                    .push(format!("NOT reachable from outside: {detail}"));
                heard
                    .lines
                    .push("  Other members cannot dial this machine. It can still take".to_owned());
                heard
                    .lines
                    .push("  part by dialling them, and one reachable side per pair is".to_owned());
                heard.lines.push(
                    "  enough -- but if a forward was meant to work, it does not.".to_owned(),
                );
            }
            reach.verdict = Some(false);
            // The published form is the found address, then perhaps this
            // machine's IPv6 address (`with_global_ipv6`); the probe tried
            // the whole list.
            if let Some(found) = found
                && published.split(',').next() == Some(found)
            {
                heard.lines.push(format!(
                    "  The router said it opened {found}, but that is not this machine: \
                     it is no longer published, and this machine reaches the others \
                     through a relay of the account if one is up."
                ));
                heard.withdraw_found = true;
            }
        }
        // Not an answer, so it does not become one. Leaving the verdict alone
        // means the next round asks again rather than reporting a guess.
        Some(coordinator::Reachability::Unknown(_)) => {}
        // Too old to answer. Not a verdict, and saying nothing is right: the
        // alternative is telling a member their forward is broken when what is
        // out of date is the coordinator.
        None => reach.verdict = None,
    }
    heard
}

/// Say the first time something from outside this network arrives.
///
/// The free half of the same question. A connection from a public address is
/// proof the forward works, and it costs nothing to notice -- where the probe
/// costs the coordinator a connection. A node with an `announce` set and none
/// of these has something wrong between it and the internet.
fn note_inbound(witness: &itsanas_net::transport::Witness, reach: &mut Reach) {
    let outside = witness.from_outside();
    if outside > reach.seen_from_outside && reach.seen_from_outside == 0 {
        println!("something from outside this network reached us: the way in works");
    }
    reach.seen_from_outside = outside;
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

        let next = if scope.connects() {
            steering.next(Instant::now(), last_round, base)
        } else {
            Next::Wait
        };
        if next == Next::Publish {
            paused_round(node, shutdown, &mut contact, bound, &mut outage);
            board.replace(contact.board());
            if let Err(error) = contact.save(&book) {
                eprintln!("itsanas: could not keep the address book: {error}");
            }
            last_round = Some(Instant::now());
        }
        if next == Next::Round {
            let (reached, announced) = one_round(
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
            board.replace(contact.board());
            // A pause asked for during the round: `halted` stopped the dialling,
            // and nothing after it may touch the folder or the store either.
            let paused_since_round = control::paused_on_disk(&node.home);
            if !paused_since_round {
                check_disks(node, interval, &mut own_check);
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

        snap.after(node, interval, bound, changed_here || next != Next::Wait);

        let next_sync = last_round.map_or_else(Instant::now, |last| last + interval);
        wait_for_work(folder.as_ref(), next_sync, shutdown);
        between_rounds(node, shutdown);
        stop_if_the_vault_came_or_went(node, shutdown);
    }
}

/// Looked at every couple of seconds, not once a round: a disk plugged back
/// in should be hosting again in seconds, not in an hour. And the other way:
/// unplugged, this run stops at once and the next one serves this account
/// alone, rather than an hour of rounds onto the bare mount point.
fn stop_if_the_vault_came_or_went(node: &Node, shutdown: &AtomicBool) {
    let present = vault_present(&node.vault_dir());
    if node.away == present {
        if present {
            eprintln!(
                "itsanas: the vault's disk is back at {}; hosting again",
                node.vault_dir().display()
            );
        } else {
            eprintln!(
                "itsanas: the vault at {} lost its marker (was the disk unplugged?); pausing",
                node.vault_dir().display()
            );
        }
        PAUSED_FOR_VAULT.store(true, Ordering::SeqCst);
        shutdown.store(true, Ordering::SeqCst);
    }
}

/// Take in what changed: the folder (or, with none, the disk's room), then
/// what peers pushed into the vault. Not called at all while paused -- a
/// `match` arm that ignored a result computed anyway was the first version of
/// this guard, and it drained the vault of a paused node (#243's review).
///
/// Whether the folder changed here: files to send out.
fn take_in(node: &Node, folder: Option<&(Folder, Option<Watcher>)>, deep: bool) -> bool {
    let mut changed = false;
    if let Some((folder, _)) = folder {
        changed = reconcile(node, folder, deep);
    } else if let Err(error) = node.bound_writes() {
        // `reconcile_once` refreshes the bound; with no folder nothing
        // did, and the pulls below would meet the ceiling `Node::open`
        // sets, which is none (8.1b).
        eprintln!("itsanas: could not work out this disk's room: {error}");
    }

    // Anything a peer pushed into this node's vault while it was serving.
    // Without this a node that never dials anybody — because it has no
    // peers configured, or because its peers are behind NAT and can only
    // push — would hold its own data and never look at it.
    match session::drain_vault(&node.store, &node.vault) {
        Ok(report) if report.changed_anything() => {
            println!(
                "pushed to us: {} files, {} conflicts",
                report.adopted, report.conflicted
            );
            if let Some((folder, _)) = folder {
                changed |= reconcile(node, folder, false);
            }
        }
        Ok(_) => {}
        Err(error) => eprintln!("itsanas: could not apply pushed data: {error}"),
    }
    changed
}

/// Say it once when a round reached nobody.
fn say_if_alone(
    node: &Node,
    neighbourhood: &Neighbourhood,
    reached: &BTreeSet<DeviceId>,
    warned_alone: &mut bool,
) {
    // Say it once, rather than leaving someone watching a silent
    // terminal wondering whether anything is happening. Silence is the
    // correct output for a working daemon and the worst possible
    // output for one that has nobody to talk to.
    if reached.is_empty() && !*warned_alone {
        *warned_alone = true;
        if node.config.peers.is_empty() && neighbourhood.is_empty() {
            println!("no other machines found yet.");
            println!("  on this network: start the daemon on another machine and it is found");
            println!("  elsewhere:       itsanas peer add <host:port>");
        } else {
            println!(
                "{} machine(s) known, none reachable this round.",
                node.config.peers.len() + neighbourhood.len()
            );
        }
    } else if !reached.is_empty() {
        *warned_alone = false;
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

/// Leave behind what `itsanas status` would have said.
///
/// The store allows one writer and this process is it, so while the daemon runs
/// every command that opens the store refuses -- including the one whose whole
/// job is to say what the node is doing. Somebody who has just dropped a file
/// into their synced folder and types `itsanas status` should not be told to
/// stop their node in order to find out whether it worked.
///
/// Written to a temporary name and renamed, so a reader never sees half of it,
/// and never sees an empty file because this process was interrupted between
/// truncating and writing. That failure is not hypothetical: the Windows
/// provisioning script destroyed a passphrase exactly that way on 2026-09-06.
///
/// Failing to write it is not worth interrupting a sync round over, and not
/// worth a line in the log every five minutes either. `status` says how old the
/// snapshot is, so a stale one announces itself.
/// The snapshot's first line: when, how often, and which program wrote it.
///
/// The version after what every reader parses (`brief_status`, `status`,
/// `snapshot_counts`, the fleet agent): it is the version of the *running
/// daemon*, which is what an `update` that restarted nothing leaves behind --
/// three laptop daemons on 0.1.8 while the program on disk was newer (hand
/// test, 2026-10-22). The fleet run compares it across machines. Then the
/// copies, when anything is stored (`safety.rs`): what the icon colours.
fn snapshot_header(now: u64, every: u64, copies: Option<crate::safety::Copies>) -> String {
    format!(
        "snapshot {now} every {every} version {}{}",
        env!("CARGO_PKG_VERSION"),
        copies.map(|c| c.render()).unwrap_or_default()
    )
}

fn write_snapshot(node: &Node, interval: Duration, bound: std::net::SocketAddr) {
    let Ok((text, counted)) = crate::render_status_and_copies(node, Some(&bound.to_string()))
    else {
        return;
    };
    let now = itsanas_discover::now_unix();
    // When the shortfall began is carried by the file itself, so a restarted
    // daemon neither forgets an old one nor grants a new grace to it.
    let final_path = node.home.join(crate::node::SNAPSHOT);
    let previous = std::fs::read_to_string(&final_path)
        .ok()
        .and_then(|old| crate::safety::Copies::parse(old.lines().next().unwrap_or("")));
    let copies = counted
        .and_then(|c| crate::safety::Copies::this_round(c.have, c.want, true, previous, now));

    // The first line is the time it was taken, in seconds, and the reader turns
    // that into an age. Storing the age itself would be wrong the moment it was
    // written, and formatting a date needs a calendar this project does not
    // carry a dependency for -- while "four minutes ago" is both what somebody
    // wants to know and arithmetic.
    // Then the interval, so a reader can tell a late snapshot from a stale
    // one: "stale" is two intervals without a round, and the interval is this
    // process's to know (`--interval`, the metered plan), nobody else's.
    let stamped = format!(
        "{}\n{text}",
        snapshot_header(now, interval.as_secs(), copies)
    );

    let pending = node.home.join(format!("{}.new", crate::node::SNAPSHOT));
    if std::fs::write(&pending, stamped).is_ok() {
        let _ = std::fs::rename(&pending, &final_path);
    }
}

/// Say what a device short of room did with this round.
///
/// Worth a line because none of it is a failure and all of it reads like one
/// otherwise: files arriving, files being let go of, and the case that has to be
/// said out loud -- content this device would have released and could not,
/// because no live holder is known for it. That last one means the device is
/// over its limit *and* is the only place that data exists, which is the state
/// a person most needs to be told about and the one a silent implementation
/// would hide.
fn report_keeping(peer: &str, keeping: &crate::keeping::KeepingReport) {
    if !keeping.worth_reporting() {
        return;
    }

    let mut line = format!("{peer}: keeping");
    if keeping.fetched > 0 {
        let _ = write!(line, " {} fetched", keeping.fetched);
    }
    if keeping.released > 0 {
        let _ = write!(
            line,
            " {} let go ({} freed)",
            keeping.released,
            format_size(keeping.freed)
        );
    }
    println!("{line}");

    if keeping.not_safe_yet > 0 {
        println!(
            concat!(
                "  {} file(s) could not be let go of: letting go needs {} other ",
                "live machines,"
            ),
            keeping.not_safe_yet,
            itsanas_store::holders::SAFE_TO_RELEASE
        );
        println!("  one of which has confirmed that very chunk recently.");
        println!("  This device stays over its limit until they do.");
    }
}

/// The third half of a round: hold some of what this peer needs held.
///
/// Without it the *dialled* side does all the hosting, because `push` gives
/// this node's work away and `pull` only fetches. That shut out every member
/// behind a router they do not control -- and in a system built on members
/// holding each other's data, being unable to host is being unable to keep your
/// half of the bargain, not a missing convenience.
///
/// Runs after push and pull: a peer is given what it is owed before being asked
/// a favour, and a failure here must not cost this node the sync it came for.
/// Nothing here is fatal, including a peer too old to understand the question,
/// which answers `Refused`.
/// What the listener answers with: this node's store and vault, its pledge,
/// the relay board, and the per-account bound with this node's own claim.
/// Whether this run turns every other account away: its vault's disk is
/// away, or it is a portable copy, which travels and is nobody's host.
fn hosts_nobody(node: &Node) -> bool {
    node.away || crate::portable::is_portable(&node.home)
}

fn peer_service<'a>(
    node: &'a Node,
    board: &'a SharedBoard,
    disk_room: &'a (dyn Fn() -> Option<u64> + Sync),
) -> PeerService<'a> {
    PeerService::new(
        &node.store,
        &node.vault,
        Pledge {
            bytes: node.config.pledge_bytes,
        },
    )
    .with_relay(board)
    .with_disk_room(disk_room)
    // Whose each device is and what it offers, as its claim said (§8 1c).
    .with_owners(owners(node))
    .hosting_away(hosts_nobody(node))
    // Empty in `everyone`, so `WantHosted` answers as it always did; in
    // `measured` each round swaps its placement in (`placing::begin_round`).
    .with_placement(crate::placing::slot())
    .claiming(node.claim_bytes(itsanas_discover::now_unix()))
}

/// The claims this daemon's peers presented (§8 1c), shared by the listener,
/// which takes them, and the hosting pull, which is held to them as offers
/// are. One per process: a node runs one daemon. Asks the coordinator about a
/// device of another account when it presents (§8 1c (ii)).
fn owners(node: &Node) -> &'static itsanas_node::owners::ClaimBook {
    static OWNERS: std::sync::OnceLock<itsanas_node::owners::ClaimBook> =
        std::sync::OnceLock::new();
    OWNERS.get_or_init(|| {
        itsanas_node::owners::ClaimBook::new().asking(node.store.owner(), coordinator::asker(node))
    })
}

/// Ask the coordinator about the devices whose confirmation is ageing or
/// missing (§8 1c (ii)). Said only when it changes something: a withdrawal,
/// or a coordinator that would not answer while devices waited.
fn check_standing(node: &Node) {
    match coordinator::standing(node, owners(node)) {
        Ok(report) => {
            if report.unpinned {
                println!(
                    "coordinator: not pinned, so its word about other accounts' devices is not \
                     taken and they store nothing here; pin it with \
                     `itsanas coordinator <host:port> --device <id>`"
                );
            }
            if report.withdrawn > 0 {
                println!(
                    "coordinator: {} device(s) presenting here were withdrawn by their account; \
                     they store nothing here",
                    report.withdrawn
                );
            }
            if report.unanswered > 0 {
                println!(
                    "coordinator: did not answer for {} device(s) of other accounts; until it \
                     does they store nothing here",
                    report.unanswered
                );
            }
        }
        Err(error) => println!(
            "coordinator: could not confirm the devices of other accounts ({error}); \
             until it can they store nothing here"
        ),
    }
}

/// What the disk can still take beyond the reserve, read per offer.
/// No room at all while the vault's marker is missing: a NAS that dropped
/// while the daemon runs leaves an empty local mount point, and chunks written
/// there land on the system disk and vanish under the mount when it returns
/// (Rodin, 2026-10-13).
fn disk_room(vault: std::path::PathBuf) -> impl Fn() -> Option<u64> + Sync {
    move || {
        if !vault_present(&vault) {
            return Some(0);
        }
        fs4::available_space(&vault)
            .ok()
            .map(|free| free.saturating_sub(itsanas_node::pledges::RESERVE))
    }
}

/// Whether the vault directory still carries its marker.
pub(crate) fn vault_present(vault: &std::path::Path) -> bool {
    vault.join(itsanas_node::node::VAULT_MARKER).is_file()
}

/// The pledge as far as the disk can still honour it (`pledges::keepable`):
/// another program filling the disk must stop the hosting, not a write.
fn keepable_pledge(node: &Node) -> u64 {
    if !vault_present(&node.vault_dir()) {
        return 0;
    }
    itsanas_node::pledges::keepable(
        node.config.pledge_bytes,
        node.vault.held_bytes().unwrap_or(0),
        fs4::available_space(node.vault_dir()).ok(),
    )
}

fn take_on_hosting(node: &Node, peer: &str, client: &mut PeerClient) {
    let pledge = Pledge::bytes(keepable_pledge(node));
    let bound: (&dyn itsanas_net::Owners, &itsanas_store::Store) = (owners(node), &node.store);
    match session::host_for_bounded(&node.vault, client, pledge, Some(bound)) {
        // A peer that wanted nothing has nothing to say, and saying it every
        // five minutes would fill a journal with silence. A peer that wanted
        // something always leaves a line, even when the answer was "already
        // held" -- because that is the case that was silent, and a feature
        // whose success looks exactly like never having run is not verifiable.
        //
        // Found by watching for a line that never came: on this project's own
        // fleet the reciprocal half ran for hours against a peer with three
        // under-replicated chunks and printed nothing at all, because they were
        // chunks this node already had.
        Ok(report) if report.wanted == 0 => {}
        Ok(report) => {
            let mut line = format!("{peer}: asked to hold {} chunks", report.wanted);
            if report.taken > 0 {
                let _ = write!(
                    line,
                    "; took {} ({})",
                    report.taken,
                    format_size(report.bytes_taken)
                );
            }
            if report.already_held > 0 {
                let _ = write!(line, "; {} already held", report.already_held);
            }
            if let Some(why) = &report.bounded {
                let _ = write!(line, "; took no more: {why}");
            }
            if report.pledge_full {
                line.push_str("; this node is full");
            }
            println!("{line}");
        }
        // A machine of another account never lets this one read what it
        // holds for itself (`NOT_YOUR_ACCOUNT`, protocol 11): between accounts
        // the route is the owner's push. Said every round, the expected answer
        // buried the line that mattered (`tests/route.rs`, 2026-10-30).
        Err(itsanas_net::NetError::Refused(why)) if why == itsanas_net::NOT_YOUR_ACCOUNT => {}
        Err(error) => println!("{peer}: could not take on hosting ({error})"),
    }
}

/// One pass over every way this node knows of reaching a peer.
///
/// In order: the coordinator, if this round owes it anything; addresses
/// somebody typed into the configuration; the account's other devices where the
/// coordinator last said they were; and whatever announced itself on the local
/// network. Each is dialled with its device id pinned wherever one is known, so
/// an address that answers as somebody else is refused rather than trusted.
///
/// Returns who was reached and, if this round published, what it published:
/// whether that is worth asking to be probed is the caller's question.
#[allow(clippy::too_many_arguments)]
fn one_round(
    node: &Node,
    shutdown: &AtomicBool,
    neighbourhood: &Neighbourhood,
    contact: &mut Contact,
    bound: std::net::SocketAddr,
    outage: &mut Outage,
    scope: PolicyScope,
    folder: Option<&Folder>,
    interval: Duration,
) -> (BTreeSet<DeviceId>, Option<String>) {
    // The vault's disk dropped under a running daemon: stop this run, rather
    // than run a round that writes hosted data to the local mount point.
    // `itsanas daemon` then waits for the disk and starts again by itself.
    // Away, the disk coming back is the news: this run stops, and `itsanas
    // daemon` opens the node again on its real vault.
    if node.away && vault_present(&node.vault_dir()) {
        eprintln!(
            "itsanas: the vault's disk is back at {}; hosting again",
            node.vault_dir().display()
        );
        PAUSED_FOR_VAULT.store(true, Ordering::SeqCst);
        shutdown.store(true, Ordering::SeqCst);
        return (BTreeSet::new(), None);
    }
    if !node.away && !vault_present(&node.vault_dir()) {
        eprintln!(
            "itsanas: the vault at {} lost its marker (was the disk unplugged?); pausing",
            node.vault_dir().display()
        );
        PAUSED_FOR_VAULT.store(true, Ordering::SeqCst);
        shutdown.store(true, Ordering::SeqCst);
        return (BTreeSet::new(), None);
    }
    // The coordinator first, and only with a reason: this machine has not
    // published since it started, has moved, owes its hourly publication, or
    // last round reached a machine of the account at none of its addresses
    // (`contact.rs`). First because nothing it decides depends on who this
    // round reaches, and a publication that waited for the round would wait
    // behind every sync in it -- hours, on the first sync of a large folder,
    // during which nobody elsewhere can find this machine. It also fills the
    // book before the book is dialled. Failures here are ordinary: it may be
    // down, and a node whose peers are already known keeps working without it.
    let announced = publish(node, shutdown, contact, bound, outage);

    // The devices of other accounts this node hosts for, confirmed with the
    // coordinator before they store (§8 1c (ii)). After the contact above, so
    // a coordinator that just answered is the one asked.
    if !shutdown.load(Ordering::Relaxed) {
        check_standing(node);
    }

    // Placement for the whole round, after the coordinator so its claims are
    // the freshest this round will have, before the first dial so every push
    // of the round ranks with the same one (`placing`).
    let accounts = owners(node).accounts();
    let own = crate::placing::own_devices(
        contact.candidates().into_iter().map(|(device, _)| device),
        &accounts,
        node.store.owner(),
    );
    crate::placing::begin_round(
        node.config.placement,
        &node.store,
        own,
        &accounts,
        itsanas_discover::now_unix(),
        interval.as_secs(),
    );

    // Configured peers next: somebody typed those in, so they are
    // wanted even if they are also on the local network — and being in
    // the configuration is itself the evidence that they are real, so
    // they are confirmed on contact without having to earn it.
    // Hosts of other accounts that proved themselves this round, at the
    // address that reached them: relays of last resort (§8 0v).
    let mut hosts = Vec::new();
    let mut reached = dial_configured(node, shutdown, neighbourhood, scope, &mut hosts);

    // The account's devices at the addresses the coordinator last gave, the
    // one that last worked first. Pinned: the coordinator supplies addresses
    // and is not trusted to say who lives at one.
    dial_listed(
        node,
        shutdown,
        neighbourhood,
        contact,
        &mut reached,
        scope,
        bound,
    );

    // What the account's own machines just sent is written now, not after the
    // other accounts' hosts below, where the slow and older builds are (a
    // laptop's files waited 20 min behind them on 2026-10-06).
    if let Some(folder) = folder.filter(|_| !control::paused_on_disk(&node.home)) {
        reconcile_once(node, folder, false);
    }

    let mut strangers_dialled = 0usize;
    for candidate in neighbourhood.dial_order(&HouseholdKey::of(&node.user)) {
        if halted(node, shutdown) {
            break;
        }
        if reached.contains(&candidate.device) {
            continue;
        }

        // Dialling costs a handshake and a round trip, and minting the
        // identity that provoked it cost an attacker nothing. Confirmed
        // peers are always dialled; unconfirmed ones are rationed, so a
        // flood cannot consume the interval that real syncing needs.
        let known = neighbourhood.is_confirmed(&candidate.device);
        if !known {
            if strangers_dialled >= discovery::NEW_PEERS_PER_ROUND {
                continue;
            }
            strangers_dialled += 1;
        }

        // A stranger that does not answer is not news — this network is
        // built out of machines that are usually off. One of your own
        // machines failing to answer is worth saying.
        let outcome = sync_once(
            node,
            &candidate.address.to_string(),
            None,
            Some(candidate.device),
            candidate.mine,
            false,
            scope,
        );

        if let Some(outcome) = outcome {
            reached.insert(outcome.device);
            // Authenticating proves possession of a keypair that cost
            // nothing to generate. Only doing something a real host
            // does — storing our data, or serving us our own work —
            // earns a place a stranger cannot take.
            if outcome.earned_trust {
                neighbourhood.confirm(outcome.device);
                hosts.push((outcome.device, candidate.address.to_string()));
            }
        }
    }
    contact.hosts_reached(hosts, Instant::now());
    // A machine of the account that answered nowhere -- not at the book's
    // addresses, not on the LAN, not as a configured peer -- may have moved
    // or restarted on another port: the next round reads the coordinator
    // again instead of waiting for the hourly publication (`contact.rs`,
    // `REREAD_AFTER_MISS`). A round cut short dialled nobody it skipped.
    if !halted(node, shutdown) {
        contact.round_reached(&reached, Instant::now());
    }

    (reached, announced)
}

/// Tell the coordinator where this machine is, and learn where the account's
/// When the account check last ran, unix seconds (once every six hours).
static DELETED_CHECKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Ask the coordinator which accounts this host keeps something for were
/// deleted, and free what it keeps for them (HANDOVER §8 0w (8)). Asked at
/// the rounds rather than told once, so a host offline on the day still frees
/// it. If this machine's own account was deleted, it records its departure
/// and stops, as `leave` does: its keys can no longer register anywhere.
fn forget_deleted(node: &Node, shutdown: &AtomicBool) {
    let now = itsanas_discover::now_unix();
    if now.saturating_sub(DELETED_CHECKED.load(Ordering::Relaxed)) < 6 * 3600 {
        return;
    }
    DELETED_CHECKED.store(now, Ordering::Relaxed);
    let mut owners = node.vault.owners().unwrap_or_default();
    let me = node.store.owner();
    if !owners.contains(&me) {
        owners.push(me);
    }
    let Ok(deleted) = coordinator::deleted_among(node, &owners) else {
        return;
    };
    for owner in deleted {
        if owner == me {
            println!(
                "this account was deleted at its coordinator; this machine leaves and stops. \
                 Remove its data with install/clean (--purge-account)."
            );
            if let Err(error) = crate::record_departure(&node.home, now) {
                eprintln!("itsanas: could not record the departure: {error}");
            }
            shutdown.store(true, Ordering::SeqCst);
            continue;
        }
        match node.vault.forget_owner(owner) {
            Ok((chunks, segments)) => println!(
                "an account this machine kept data for was deleted: freed {chunks} chunk(s) \
                 and {segments} log segment(s)"
            ),
            Err(error) => eprintln!("itsanas: could not free a deleted account's data: {error}"),
        }
    }
}

/// others are -- when `contact` says it is due. Returns the address published.
fn publish(
    node: &Node,
    shutdown: &AtomicBool,
    contact: &mut Contact,
    bound: std::net::SocketAddr,
    outage: &mut Outage,
) -> Option<String> {
    let mut announced: Option<String> = None;
    if node.config.coordinator.is_some() && !shutdown.load(Ordering::Relaxed) {
        let listen = bound.to_string();
        let here = coordinator::address_now(&node.config, &listen);
        let due = contact.due(here.as_deref(), Instant::now());
        if due.any() {
            contact.asked(&due, Instant::now());
            match coordinator::contact(node, &listen, itsanas_discover::now_unix(), &due) {
                Ok(contacted) => {
                    outage.succeeded();
                    forget_deleted(node, shutdown);
                    if let Some(published) = contacted.published {
                        contact.published(here.as_deref(), Instant::now());
                        announced = Some(published);
                    }
                    if let Some(found) = contacted.found {
                        contact.read(
                            &found,
                            &contacted.presences,
                            &contacted.claimed,
                            Instant::now(),
                        );
                        if contacted.signed {
                            contact.signed();
                        } else {
                            eprintln!(
                                "itsanas: the coordinator does not sign its list of this \
                                 account's machines (it is older than this node); their \
                                 addresses are taken on its word until it is upgraded"
                            );
                        }
                    }
                    if let Some(error) = contacted.read_failed {
                        eprintln!(
                            "itsanas: published, but could not list the account's machines: {error}"
                        );
                    }
                    if let Some(error) = contacted.pledges_not_kept {
                        eprintln!("itsanas: could not keep the account's pledges: {error}");
                    }
                    if contacted.forged > 0 {
                        eprintln!(
                            "itsanas: the coordinator listed {} address(es) their machine never \
                             signed; ignored. It is lying, or it is not the coordinator you \
                             meant (pin it with its device id).",
                            contacted.forged
                        );
                    }
                }
                Err(error) => outage.failed(&error.to_string()),
            }
        }
    }
    announced
}

/// Stop dialling: the process is ending, or somebody paused syncing during
/// this round. Read before each machine, so a pause gives the bandwidth back
/// once the session under way ends rather than when the whole round does.
fn halted(node: &Node, shutdown: &AtomicBool) -> bool {
    shutdown.load(Ordering::Relaxed) || control::paused_on_disk(&node.home)
}

/// Sync with every peer in the configuration, and say which answered.
///
/// In the same order as everything else a round dials: the ones that can
/// answer from where this machine stands, first. A configured peer is usually
/// a LAN address somebody typed at home, and a round that starts with those
/// spends its first connect timeouts on them from anywhere else.
fn dial_configured(
    node: &Node,
    shutdown: &AtomicBool,
    neighbourhood: &Neighbourhood,
    scope: PolicyScope,
    hosts: &mut Vec<(DeviceId, String)>,
) -> BTreeSet<DeviceId> {
    let mut reached = BTreeSet::new();
    let mut configured = node.config.peers.clone();
    coordinator::reachable_first_addresses(&mut configured);
    for peer in &configured {
        if halted(node, shutdown) {
            break;
        }
        if let Some(outcome) = sync_once(node, peer, None, None, true, false, scope) {
            if outcome.earned_trust {
                hosts.push((outcome.device, peer.clone()));
            }
            neighbourhood.confirm(outcome.device);
            reached.insert(outcome.device);
        }
    }
    reached
}

/// Dial the account's devices at every address the coordinator gave for them,
/// best first, until each answers once.
fn dial_listed(
    node: &Node,
    shutdown: &AtomicBool,
    neighbourhood: &Neighbourhood,
    contact: &mut Contact,
    reached: &mut BTreeSet<DeviceId>,
    scope: PolicyScope,
    bound: std::net::SocketAddr,
) {
    // Never this node itself, whatever device the book names there.
    let is_me = this_listener(
        node.config.announce.clone(),
        coordinator::found_address(),
        bound,
        itsanas_discover::LocalNets::of_this_machine(),
    );
    // A private address only of a machine heard on this LAN, or one that
    // answered, or at a probe round: two houses on 192.168.1.x are not one
    // network, and another VLAN or a VPN may be (`Contact::dialable`). A
    // machine that answered nowhere waits a little longer at each miss, up
    // to half an hour (`QUIET_AFTER_MISS`): most of an account is off.
    let now = Instant::now();
    for (device, address) in contact.dialable(|device| neighbourhood.heard_here(device), is_me, now)
    {
        if halted(node, shutdown) || reached.contains(&device) {
            continue;
        }
        // A machine parked on a relay (§8 0v) is dialled through it, at the
        // relay's own address from this book.
        let through = contact.through(&address);
        if address.starts_with(itsanas_node::contact::RELAY_PREFIX) && through.is_none() {
            continue;
        }
        let Some(outcome) = sync_once(
            node,
            &address,
            through.as_ref(),
            Some(device),
            true,
            true,
            scope,
        ) else {
            contact.failed(device, &address, Instant::now());
            continue;
        };
        reached.insert(outcome.device);
        contact.worked(device, &address, itsanas_discover::now_unix());
        // What this machine of the account knows of the others. Checked
        // row by row (`Contact::relayed`); what it adds is dialled from the
        // next round, behind every address that has worked.
        if let Some(rows) = &outcome.relayed {
            let relayed = contact.relayed(
                rows,
                node.store.owner(),
                node.store.device_id(),
                itsanas_discover::now_unix(),
            );
            // Only what fails its signatures is worth a line: a withdrawn
            // machine or an address older than one heard first-hand is what
            // siblings relay every round (`Relayed`), and printing those
            // drowned the one case that means somebody is lying.
            if relayed.forged > 0 {
                eprintln!(
                    "itsanas: {address} relayed {} address(es) of this account's machines \
                     that fail their signatures (forged, another account's, or dated in \
                     the future); ignored",
                    relayed.forged
                );
            }
        }
        if outcome.earned_trust {
            neighbourhood.confirm(outcome.device);
        }
    }
}

/// Whether an address is this node, for [`dial_listed`]: this machine's
/// addresses at the port the daemon is **bound** to -- not the configured
/// `listen`, which a daemon started with `--listen` does not use (the same
/// mistake the enrolment made, 2026-10-23) -- or what it publishes, written
/// (`announce`) or opened by the router (`found`).
fn this_listener(
    announce: Option<String>,
    found: Option<String>,
    bound: std::net::SocketAddr,
    here: itsanas_discover::LocalNets,
) -> impl Fn(&str) -> bool {
    let published: Vec<String> = announce.into_iter().chain(found).collect();
    move |address: &str| {
        let published: Vec<&str> = published.iter().map(String::as_str).collect();
        itsanas_node::contact::is_this_listener(address, bound.port(), &here, &published)
    }
}

/// Check a slice of the vault's index and of this node's own chunks against
/// the disk, sized so each completes a pass within `REFRESH_AFTER`.
///
/// Since §8 4c a peer whose summary agrees with this node's is re-stamped
/// without being asked, so a summary has to stop counting a blob the disk has
/// lost, or this node keeps counting as a copy it cannot serve. Never fails
/// the round: a check that cannot run is said, and the next round tries again.
fn check_disks(node: &Node, interval: Duration, own_cursor: &mut Option<itsanas_crypto::ChunkId>) {
    let every = interval.as_secs();
    // In batches: one call holds at most `MAX_CHECK_BATCH` keys, however large
    // the slice this round owes.
    let vault_pass = node.vault.chunk_rows().and_then(|rows| {
        let mut owed = itsanas_store::holders::rows_per_round(rows, every);
        let mut total = itsanas_store::DiskCheck::default();
        while owed > 0 {
            let check = node.vault.check_disk(owed)?;
            total.checked += check.checked;
            total.removed += check.removed;
            if check.checked == 0 {
                break;
            }
            owed = owed.saturating_sub(check.checked);
        }
        Ok(total)
    });
    match vault_pass {
        Ok(check) if check.removed > 0 => println!(
            "vault: {} chunk(s) held for others were no longer on this disk; their owners \
             will send them again",
            check.removed
        ),
        Ok(_) => {}
        Err(error) => eprintln!("itsanas: could not check the vault against the disk: {error}"),
    }
    let own_pass = node.store.live_chunk_count().and_then(|rows| {
        let mut owed = itsanas_store::holders::rows_per_round(rows, every);
        let mut missing = 0usize;
        while owed > 0 {
            let batch = owed.min(itsanas_store::MAX_CHECK_BATCH);
            let (lost, next) = node.store.check_disk(own_cursor.as_ref(), batch)?;
            missing += lost;
            *own_cursor = next;
            owed -= batch;
            if own_cursor.is_none() {
                break;
            }
        }
        Ok((missing, ()))
    });
    match own_pass {
        Ok((missing, ())) => {
            if missing > 0 {
                println!(
                    "{missing} chunk(s) of this account are no longer on this disk; \
                     repair will fetch them from their holders"
                );
            }
        }
        Err(error) => {
            eprintln!("itsanas: could not check this node's chunks against the disk: {error}");
        }
    }
}

/// Block until the folder changes, the next sync is due, or shutdown.
fn wait_for_work(
    folder: Option<&(Folder, Option<Watcher>)>,
    next_sync: Instant,
    shutdown: &AtomicBool,
) {
    let budget = next_sync.saturating_duration_since(Instant::now());

    if let Some((_, Some(watcher))) = folder {
        // Chopped into slices so a Ctrl-C during a long quiet period is still
        // noticed promptly.
        let slice = budget.min(Duration::from_secs(2));
        if watcher.wait_for_quiet(slice, watch::DEFAULT_DEBOUNCE, SETTLE_LIMIT) {
            return;
        }
        if shutdown.load(Ordering::Relaxed) {
            return;
        }
        return;
    }

    // No watcher: plain sleep, still chopped for responsiveness.
    let deadline = Instant::now() + budget.min(Duration::from_secs(2));
    while Instant::now() < deadline && !shutdown.load(Ordering::Relaxed) {
        std::thread::sleep(SHUTDOWN_POLL);
    }
}

/// Open the configured folder and start watching it, if one is configured.
fn open_folder(node: &Node) -> Result<Option<(Folder, Option<Watcher>)>> {
    let Some(path) = &node.config.folder else {
        return Ok(None);
    };

    let folder = Folder::open(path)?;

    // A watcher is a latency optimisation. If the platform will not give us
    // one — too many watches, an unusual filesystem — the periodic rescan
    // still does the job, so this is a warning rather than a failure.
    let watcher = match Watcher::new(folder.root()) {
        Ok(watcher) => Some(watcher),
        Err(error) => {
            eprintln!(
                "itsanas: could not watch {}: {error}. Falling back to periodic \
                 scanning only, so local edits will take up to one interval to \
                 be noticed.",
                folder.root().display()
            );
            None
        }
    };

    Ok(Some((folder, watcher)))
}

/// One folder pass. Never propagates an error.
///
/// Two outcomes here are not failures of this daemon and must not read as
/// noise: a storage that is not mounted, and a pass that held deletions. Both
/// mean somebody has to do something, and both are said loudly, because the
/// alternative -- a line in a log nobody reads -- is how a disk stays unmounted
/// for a week.
pub(crate) fn reconcile_once(node: &Node, folder: &Folder, deep: bool) {
    reconcile(node, folder, deep);
}

/// Whether a pass found something changed *here*: a file taken in from the
/// folder or deleted from it. What it wrote out from the store came from
/// elsewhere and has nobody to go to.
fn changed_here(report: &itsanas_folder::ReconcileReport) -> bool {
    !report.imported.is_empty() || !report.removed_from_store.is_empty()
}

/// [`reconcile_once`], saying whether the folder changed here.
fn reconcile(node: &Node, folder: &Folder, deep: bool) -> bool {
    // Counting what other devices wrote since the last pass. On failure the
    // bound from the last pass stays, which is never none: opening the node
    // set one.
    if let Err(error) = node.bound_writes() {
        eprintln!("itsanas: could not count the account's files elsewhere: {error}");
    }
    match folder.reconcile(&node.store, deep) {
        Ok(report) => {
            if report.changed_anything() {
                println!("folder: {}", report.summary());
                // Named, so that a log says which files moved and a rename
                // reads as one, but bounded: a first pass over a full
                // library would otherwise flood it.
                for line in report.lines(Some(itsanas_folder::LINES_IN_A_LOG)) {
                    println!("{line}");
                }
            }
            if report.held_anything() {
                println!(
                    "folder: HELD {} deletion(s): that is most of this folder, and a \
                     folder somebody emptied looks exactly like a disk that is not \
                     mounted",
                    report.held_deletions
                );
                println!("  Nothing was removed from the account, here or anywhere else.");
                println!("  If they really are meant to go: `itsanas folder --confirm`.");
            }
            for (path, why) in &report.failed {
                eprintln!("itsanas: {path}: {why}");
            }
            changed_here(&report)
        }
        Err(error) => {
            eprintln!("itsanas: folder scan failed: {error}");
            false
        }
    }
}

/// What one round against one peer established.
#[derive(Clone, Debug)]
struct Outcome {
    /// Which device actually answered.
    device: DeviceId,
    /// Whether it did something only a real host does.
    ///
    /// Deliberately separate from "it answered". A device key is a free
    /// keypair, so authenticating identifies a peer and vouches for nothing.
    earned_trust: bool,
    /// What the peer said of where the account's machines are
    /// ([`PeerClient::presences`]), unchecked. Asked only of a book device;
    /// `None` if it could not tell.
    relayed: Option<Vec<Vec<u8>>>,
}

/// Dial `peer`, or `expect` through the relay `through` (§8 0v).
fn dial(
    node: &Node,
    peer: &str,
    through: Option<&(DeviceId, String)>,
    expect: Option<DeviceId>,
) -> itsanas_net::Result<PeerClient> {
    let budget = Some(itsanas_net::transport::PEER_SESSION_BUDGET);
    match (through, expect) {
        (Some((relay, address)), Some(target)) => PeerClient::connect_relayed(
            itsanas_net::AddressList(address),
            *relay,
            target,
            &node.device,
            node.store.owner(),
            budget,
        ),
        _ if peer.starts_with(itsanas_node::contact::RELAY_PREFIX) => Err(
            itsanas_net::NetError::Refused("no known relay for this address".to_owned()),
        ),
        _ => PeerClient::connect_within(
            itsanas_net::AddressList(peer),
            &node.device,
            node.store.owner(),
            expect,
            budget,
        ),
    }
}

/// One round against one peer. Never propagates an error.
///
/// A peer being unreachable is the normal state of this network, not a fault:
/// the whole design is built around machines that are usually off. Treating it
/// as an error would mean the daemon exits every time someone shuts a laptop.
///
/// `expect` pins which device must answer. The device is reported even when the
/// round then failed — reaching a machine and falling out with it says nothing
/// about whether the machine exists — but `earned_trust` is only set when the
/// peer stored something of ours or served us our own work.
fn sync_once(
    node: &Node,
    peer: &str,
    through: Option<&(DeviceId, String)>,
    expect: Option<DeviceId>,
    announce_failure: bool,
    ask_presences: bool,
    scope: PolicyScope,
) -> Option<Outcome> {
    // Bounded as a whole: one peer that stalls must not hold the round, and
    // with it the folder pass and every other peer (PEER_SESSION_BUDGET).
    let mut client = match dial(node, peer, through, expect) {
        Ok(client) => client,
        Err(error) => {
            if announce_failure {
                println!("{peer}: unreachable ({error})");
            }
            crate::placing::unreached(&node.store, expect);
            return None;
        }
    };
    let answered = client.peer_device();
    present_claim(node, peer, &mut client);

    // Make the peer prove it still holds what the ledger says it took, a
    // handful of chunks per round. Without this the ledger is a list of
    // promises with nothing checking any of them, and a node believes its data
    // is on three machines while two of them threw it away.
    //
    // A failure is news: it withdraws that record, so the chunk shows as
    // under-replicated, and the operator should know a peer is not holding what
    // it said. Nothing is deleted and nobody is blocked.
    // Fetch back anything this disk has lost. Bounded both ways: a slice of
    // the live chunks examined, a handful of chunks asked for. The peer is
    // doing this for free, and a node that lost a whole disk wants a restore,
    // not a repair.
    match session::repair(&node.store, &mut client, session::REPAIR_SCAN_PER_ROUND) {
        Ok(report) if report.changed_anything() => {
            if report.restored > 0 {
                println!(
                    "{peer}: restored {} chunk(s) this disk had lost",
                    report.restored
                );
            }
            if report.forged > 0 {
                println!(
                    concat!(
                        "{}: answered {} repair request(s) with bytes that ",
                        "are not what was asked for. Those records are withdrawn."
                    ),
                    peer, report.forged
                );
            }
        }
        Ok(_) => {}
        Err(error) => println!("{peer}: could not repair ({error})"),
    }

    match session::audit(&node.store, &mut client, session::CHALLENGES_PER_ROUND) {
        Ok(report) if report.found_a_liar() => {
            println!(
                "{peer}: FAILED {} of {} storage challenges — it is not holding \
                 what it said. Those chunks now count as unreplicated.",
                report.failed, report.asked
            );
            // Say it once, when the sanction actually starts, rather than
            // every round afterwards.
            if let Some(record) = report.record
                && record.consecutive_failures == itsanas_store::FAILURES_BEFORE_PAUSE
            {
                // `concat!` rather than a trailing backslash: fmt eats the
                // backslash and leaves the indentation inside the string, which
                // is how this line spent two commits telling an operator
                // "receives          the log". It also blocks implicit capture,
                // hence the named argument. `scripts/check-messages.py` guards
                // the rest of the codebase against the same thing.
                println!(
                    concat!(
                        "  {peer} is no longer being sent new data. It still ",
                        "receives the log and one chunk a round to answer for. ",
                        "{owed} answered rounds clear this."
                    ),
                    peer = peer,
                    owed = record.rounds_to_reinstatement()
                );
            }
        }
        Ok(report) if report.probing && report.confirmed > 0 => {
            println!("{peer}: answered for the chunk it was given — no longer paused.");
        }
        Ok(_) => {}
        // An audit that could not run is not an accusation. The peer may have
        // hung up, or this device may no longer hold a copy to check against.
        Err(error) => println!("{peer}: could not audit ({error})"),
    }

    // The scope the policy chose. On a metered link this exchanges the log and
    // downloads nothing, which is the difference between a tethered laptop
    // costing kilobytes a day and costing whatever the store happens to weigh.
    // `Nothing` maps to `Metadata`, not to `Everything`. It is unreachable
    // today — `conditions()` never produces `Network::None` or `Power::Low`
    // — and the version of this that said so in a comment mapped it to
    // `Everything`, so the day somebody wires up metered detection the state
    // meaning "do nothing" would have become the state meaning "download the
    // lot". A fallback belongs on the cheap side of the choice it cannot make.
    let wire = match scope {
        PolicyScope::Metadata | PolicyScope::Nothing => session::Scope::Metadata,
        PolicyScope::Everything => session::Scope::Everything,
    };
    // The round's placement, the same `Arc` for every push of the round;
    // `None` in `everyone`, which is the push as it always was.
    let placement = crate::placing::current();
    let earned_trust = match crate::keeping::round_placed(
        &node.store,
        &node.vault,
        &node.config.keeping(),
        &mut client,
        wire,
        placement.as_deref(),
    ) {
        Ok((report, keeping)) => {
            report_round(peer, &report);
            report_keeping(peer, &keeping);
            // A quiet round is the common case. Saying so every five minutes
            // would fill a journal with nothing and train the operator to
            // ignore it.
            report.peer_earned_trust()
        }
        Err(error) => {
            println!("{peer}: failed ({error})");
            false
        }
    };

    take_on_hosting(node, peer, &mut client);

    // One exchange on a connection already open. A failure costs this round
    // the answer and nothing else: the coordinator remains the other source.
    // Only of a machine the book lists: found on the LAN, a stranger is
    // pinned too, and its answer would be read for nothing (Rodin).
    let relayed = if ask_presences {
        client.presences().unwrap_or_else(|error| {
            println!("{peer}: could not ask where the account's machines are ({error})");
            None
        })
    } else {
        None
    };

    Some(Outcome {
        device: answered,
        earned_trust,
        relayed,
    })
}

/// Tell a host whose this device is and what it offers, before offering it
/// anything (§8 1c). A refusal is said once here rather than once per chunk.
///
/// The host answers with its own claim, kept in this daemon's book: a machine
/// behind a router is never dialled, so this is the only way it learns whose
/// the hosts it pulls for are, and what they offer.
pub(crate) fn present_claim(node: &Node, peer: &str, client: &mut PeerClient) {
    let mut siblings = Vec::new();
    match client.present_claim(node.claim_bytes(itsanas_discover::now_unix())) {
        Ok(Some(theirs)) => {
            // The host's own claim: a sibling of this account when it says so,
            // which is the only way a machine behind a router learns one it
            // dials and that never dials it.
            siblings.push(theirs.clone());
            if let Err(why) = owners(node).take(client.peer_device(), &theirs, &node.vault) {
                println!("{peer}: its own claim does not check out ({why}); not hosting for it");
            }
        }
        Ok(None) => {}
        Err(error) => {
            println!("{peer}: refused this device's claim ({error}); it will store nothing here");
        }
    }
    // This account's roster, before any chain is applied this session: the
    // host itself if it is ours, the siblings it knows, and the ones that
    // presented here. Asked after presenting, which is what the host answers
    // on.
    siblings.extend(client.claims(node.store.owner()).unwrap_or_default());
    if let Err(error) = node.admit_roster(&siblings) {
        println!("{peer}: this account's device list could not be updated ({error})");
    }
}

/// Bind the peer listener, and say what to do when the port is taken.
///
/// Nodes created before `init` chose a free port all listen on 9797, so a
/// second account on such a machine fails here -- and under systemd it
/// restarts every thirty seconds with "address in use" in a journal. The error
/// now names a port that is free and not claimed by another node here, and the
/// two commands that move this node to it.
fn bind_listener(node: &Node, address: &str) -> Result<PeerServer> {
    match PeerServer::bind(address) {
        Ok(server) => Ok(server),
        Err(itsanas_net::NetError::Io(error))
            if matches!(
                error.kind(),
                std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            let free =
                crate::first_free_port(&crate::sibling_ports(&node.home), crate::port_is_free);
            Err(CliError::Usage(taken_port_message(address, &error, free)))
        }
        Err(other) => Err(other.into()),
    }
}

/// The sentence for a listen address something else already holds.
pub(crate) fn taken_port_message(
    address: &str,
    error: &dyn std::fmt::Display,
    free: Option<u16>,
) -> String {
    match free {
        Some(port) => format!(
            "cannot serve on {address} ({error}): another node or program has it, or the \
             system reserves it. \
             Port {port} is free here: `itsanas listen 0.0.0.0:{port}`, then \
             `itsanas register` if a coordinator is configured, then start again."
        ),
        None => format!(
            "cannot serve on {address} ({error}), and no port from 9797 to 9896 is free \
             here; choose one with `itsanas listen`."
        ),
    }
}

/// When each peer's refusals were last reported.
///
/// A host with pledge 0 refuses every round, and so does a stranger on the
/// network that pledged nothing: printing it every round is a line every five
/// minutes per peer for ever, which is the journal nobody reads that
/// `OUTAGE_QUIET` exists to prevent for the coordinator.
static REFUSALS_REPORTED: std::sync::Mutex<std::collections::BTreeMap<String, Instant>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Whether a refusal is worth a line, given when the last one was printed.
///
/// The first always is; after that, once per [`OUTAGE_QUIET`].
pub(crate) fn refusal_due(previous: Option<Instant>, now: Instant) -> bool {
    previous.is_none_or(|last| now.duration_since(last) >= OUTAGE_QUIET)
}

/// What a round with one peer moved, and what that peer refused.
///
/// A quiet round prints nothing, but a refusal is printed even when nothing
/// moved: a round where everything was refused is the round that most needs
/// saying, and it used to be silent because it changed nothing. Repeats are
/// held back by [`refusal_due`]; a round without refusals resets it, so a peer
/// that starts refusing again is reported at once.
fn report_round(peer: &str, report: &session::RoundReport) {
    if report.changed_anything() {
        println!(
            "{peer}: sent {} ({} chunks, {} segments), received {} files, {} conflicts{}",
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
    if let Some(no_room) = crate::describe_no_room(&report.pull) {
        println!("{peer}: {no_room}");
    }
    if let Some(refused) = crate::describe_refused_chains(&report.pull) {
        println!("{peer}: {refused}");
    }
    let Ok(mut reported) = REFUSALS_REPORTED.lock() else {
        return;
    };
    match crate::describe_refusal(&report.push) {
        Some(refused) => {
            let now = Instant::now();
            if refusal_due(reported.get(peer).copied(), now) {
                println!("{peer}: {refused}");
                reported.insert(peer.to_owned(), now);
            }
        }
        None => {
            reported.remove(peer);
        }
    }
}

fn install_signal_handler() -> Result<()> {
    // Once per process: a run that resumes after its vault's disk came back
    // installs nothing new, and `ctrlc` refuses a second handler.
    static INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if INSTALLED.get().is_some() {
        return Ok(());
    }
    ctrlc::set_handler(|| {
        PAUSED_FOR_VAULT.store(false, Ordering::SeqCst);
        SHUTDOWN.store(true, Ordering::Relaxed);
        println!();
        println!("stopping — finishing the current round…");
    })
    .map_err(|error| CliError::Usage(format!("could not install a signal handler: {error}")))?;
    let _ = INSTALLED.set(());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snapshot_says_which_version_wrote_it_and_still_reads_as_before() {
        let header = snapshot_header(1_792_000_000, 300, None);
        assert!(
            header.ends_with(&format!(" version {}", env!("CARGO_PKG_VERSION"))),
            "the snapshot does not say which program wrote it: a daemon an update left \
             behind looks like the others ({header})"
        );
        // `brief_status` reads the words in this order; the tray's colour
        // comes from it.
        let words: Vec<&str> = header.split_whitespace().take(4).collect();
        assert_eq!(words, ["snapshot", "1792000000", "every", "300"]);
        let text = format!("{header}\naccount\n  files           3\n");
        assert_eq!(
            crate::setup::verify::snapshot_counts(&text),
            Some((1_792_000_000, 3)),
            "the version broke the reader of the stamp and the file count"
        );
    }

    /// The idle bench of 2026-10-27, read with inotify: `status.snapshot`
    /// was rewritten at every pass of the loop, every two seconds, whatever
    /// the interval -- 150 rewrites a five-minute round, some 4 KB of
    /// journal each, the largest share of an idle node's disk writes (164 KB
    /// a round at a one-minute round, against 60 at ten seconds), and the
    /// whole status, copies included, counted again each time. A day of
    /// two-second passes at the default round: written after every round,
    /// at once when the folder changed or the interval did, otherwise at
    /// most every [`SNAPSHOT_EVERY`] -- and never left longer, so a stale
    /// icon still means a stuck daemon.
    #[test]
    fn red_team_an_idle_daemon_writes_its_snapshot_a_few_times_a_round_not_every_pass() {
        let start = Instant::now();
        let pass = Duration::from_secs(2);
        let interval = Duration::from_secs(300);
        let passes = 24 * 3600 / 2;
        let mut last: Option<(Instant, Duration)> = None;
        let (mut writes, mut longest) = (0u32, Duration::ZERO);
        for i in 0..passes {
            let now = start + pass * i;
            // Off the half-minute grid, so a round's write is not the
            // heartbeat's by chance.
            let round = i % 150 == 7;
            if snapshot_due(last, now, interval, round) {
                if let Some((at, _)) = last {
                    longest = longest.max(now - at);
                }
                writes += 1;
                last = Some((now, interval));
            } else {
                assert!(
                    !round,
                    "pass {i}: a round ran and the snapshot was not written"
                );
            }
        }
        assert!(
            writes <= 3_200,
            "{writes} snapshots in a day of five-minute rounds: written at nearly every \
             two-second pass, 43200 a day"
        );
        assert!(
            longest <= SNAPSHOT_EVERY,
            "the snapshot went {longest:?} unwritten: an icon reads a live daemon as stale"
        );

        let now = start + pass * passes;
        let fresh = Some((now, interval));
        assert!(
            snapshot_due(fresh, now, Duration::from_secs(3600), false),
            "the interval changed and the snapshot kept the old one: a daemon moved to an \
             hourly round reads as stale a minute later"
        );
        let short = Duration::from_secs(10);
        assert!(
            snapshot_due(Some((now, short)), now + short, short, false),
            "at a ten-second round the snapshot waited longer than a round"
        );
        assert!(
            !snapshot_due(fresh, now + pass, interval, false),
            "nothing happened two seconds later and the snapshot was written again"
        );
    }

    /// The wiring of MVP.md test L, through the daemon's own writer: a node
    /// holding a file and no peer writes `copies 0 of 2` with no grace (it
    /// was never safe), the icon reads `exposed`, and the next round keeps
    /// that. The version is still read where the fleet agent reads it.
    /// Sabotage: the header without the copies -- `healthy`, red.
    #[test]
    fn red_team_the_snapshot_a_lone_node_writes_turns_its_icon_from_green() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        let (node, _) =
            itsanas_node::Node::create(&home, "a passphrase", "camille").expect("create");
        node.store
            .write_file("notes.txt", b"only here so far")
            .expect("write");
        let bound: std::net::SocketAddr = "127.0.0.1:9797".parse().unwrap();
        let interval = Duration::from_secs(300);

        write_snapshot(&node, interval, bound);
        let text = std::fs::read_to_string(home.join(crate::node::SNAPSHOT)).expect("snapshot");
        let header = text.lines().next().unwrap_or("").to_owned();
        assert!(
            header.ends_with(&format!(
                " version {} copies 0 of 2",
                env!("CARGO_PKG_VERSION")
            )),
            "the snapshot of a node with a file and no peer does not say it has no copy \
             elsewhere: {header}"
        );
        let now = itsanas_discover::now_unix();
        let brief = crate::brief_status(&home, true, now);
        assert!(
            brief.starts_with("exposed ") && brief.ends_with(" 0 2"),
            "the icon of a node whose only copy is its own disk reads {brief:?}"
        );
        write_snapshot(&node, interval, bound);
        let again = std::fs::read_to_string(home.join(crate::node::SNAPSHOT)).expect("snapshot");
        assert!(
            again
                .lines()
                .next()
                .unwrap_or("")
                .ends_with(" copies 0 of 2"),
            "a second round granted a grace to a node that was never safe: {}",
            again.lines().next().unwrap_or("")
        );
        let version = header
            .split(" version ")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next());
        assert_eq!(version, Some(env!("CARGO_PKG_VERSION")));
    }

    /// The daemon's side of `red_team_an_address_that_is_this_node_is_never_dialled_for_another_device`
    /// (contact.rs): the port that makes an address this node is the one the
    /// daemon is bound to, and both published addresses count. A node
    /// configured for 9797 and started on 9811 is at 9811; 9797 on the same
    /// machine is the next account.
    #[test]
    fn the_daemon_takes_itself_at_its_bound_port_and_its_published_addresses() {
        let ip = |text: &str| text.parse::<std::net::IpAddr>().unwrap();
        let laptop = itsanas_discover::LocalNets::from_interfaces([(
            "Wi-Fi".to_owned(),
            ip("192.168.1.142"),
            ip("255.255.255.0"),
        )]);
        let is_me = this_listener(
            Some("ngas.fr:9811".to_owned()),
            Some("86.238.26.80:9811".to_owned()),
            "0.0.0.0:9811".parse().expect("bound"),
            laptop,
        );
        for itself in ["192.168.1.142:9811", "ngas.fr:9811", "86.238.26.80:9811"] {
            assert!(
                is_me(itself),
                "{itself} is this daemon and would be dialled for another device"
            );
        }
        assert!(
            !is_me("192.168.1.142:9797"),
            "the configured port was taken for the bound one: the next account on this \
             machine would never be dialled"
        );
    }

    /// Red team, hand test 2026-10-22. The laptop's third account listens on
    /// 9799, the router said yes to opening 9799 for it, and a hand-made
    /// forward of 9799 to the VM answered instead: the coordinator's probe
    /// said so, and the daemon went on publishing the address. Every member
    /// dialled the VM there at every round and was refused, and the laptop,
    /// believing itself reachable, never parked on a relay. An address the
    /// router gave and the probe disproves is withdrawn; an `announce`
    /// somebody wrote is theirs to fix, and stays.
    #[test]
    fn red_team_a_router_port_that_reaches_another_machine_is_withdrawn() {
        let other = || {
            Some(coordinator::Reachability::Unreachable(
                "203.0.113.9:9799 answered, but not as this device".to_owned(),
            ))
        };
        let mut reach = Reach::new();
        let heard = hear_reachability(
            other(),
            "203.0.113.9:9799",
            Some("203.0.113.9:9799"),
            &mut reach,
        );
        assert!(
            heard.withdraw_found,
            "a router port the coordinator found another machine behind stayed published: \
             every member keeps dialling the wrong machine, and this one never parks on a relay"
        );
        assert!(
            heard
                .lines
                .iter()
                .any(|line| line.contains("no longer published")),
            "the withdrawal was not said: {:?}",
            heard.lines
        );

        // With this machine's IPv6 address after it, the probe tried both.
        let mut reach = Reach::new();
        let listed = hear_reachability(
            other(),
            "203.0.113.9:9799,[2001:db8::7]:9799",
            Some("203.0.113.9:9799"),
            &mut reach,
        );
        assert!(
            listed.withdraw_found,
            "the list form kept a disproved router port"
        );

        // A written `announce` is never found: told, not withdrawn.
        let mut reach = Reach::new();
        let written = hear_reachability(other(), "ngas.fr:9799", None, &mut reach);
        assert!(!written.withdraw_found);
        assert!(
            !written.lines.is_empty(),
            "an unreachable announce went unsaid"
        );

        // A router port that does reach this machine stays.
        let mut reach = Reach::new();
        let good = hear_reachability(
            Some(coordinator::Reachability::Reachable(
                "it is this device".to_owned(),
            )),
            "203.0.113.9:9799",
            Some("203.0.113.9:9799"),
            &mut reach,
        );
        assert!(!good.withdraw_found, "a working router port was withdrawn");

        // No verdict is no reason either.
        for answer in [
            Some(coordinator::Reachability::Unknown(
                "probed within the hour".to_owned(),
            )),
            None,
        ] {
            let mut reach = Reach::new();
            let unsure = hear_reachability(
                answer,
                "203.0.113.9:9799",
                Some("203.0.113.9:9799"),
                &mut reach,
            );
            assert!(
                !unsure.withdraw_found,
                "a router port was withdrawn on no verdict"
            );
        }
    }

    /// Red team, 2026-10-23. A key that enrols at its first daemon start ran
    /// `register_and_announce`, which announced the configured `listen`
    /// beside the rounds' publication of the bound one: a daemon started on
    /// another port sent the account to the configured port until the next
    /// publication. It announces the bound address. And a `registered` marker
    /// is per coordinator: pointed at another one, the node enrols there at
    /// its next start; pointed at the same, it does not enrol again.
    #[test]
    fn red_team_a_daemon_enrols_at_its_bound_address_with_the_coordinator_it_uses() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (mut node, _) = Node::create(
            &dir.path().join("key"),
            "the passphrase of this key",
            "camille",
        )
        .expect("node");
        node.config.coordinator = Some("198.51.100.7:9797".to_owned());
        node.config.listen = "0.0.0.0:9797".to_owned();

        let mut announced = None;
        let said = enrol_at_daemon_start(&node, "0.0.0.0:9811", |_, listen| {
            announced = Some(listen.to_owned());
            Ok(())
        });
        assert!(said.is_some_and(|line| line.contains("198.51.100.7:9797")));
        assert_eq!(
            announced.as_deref(),
            Some("0.0.0.0:9811"),
            "the enrolment announced another address than the one the daemon is bound to"
        );
        assert_eq!(
            crate::registered_with(&node.home).as_deref(),
            Some("198.51.100.7:9797"),
            "the marker does not name the coordinator enrolled with"
        );
        assert!(
            enrol_at_daemon_start(&node, "0.0.0.0:9811", |_, _| panic!("enrolled twice")).is_none(),
            "a node enrolled with its coordinator enrolled again"
        );

        node.config.coordinator = Some("203.0.113.9:9797".to_owned());
        let mut asked = 0;
        let said = enrol_at_daemon_start(&node, "0.0.0.0:9811", |_, _| {
            asked += 1;
            Ok(())
        });
        assert_eq!(
            asked, 1,
            "pointed at another coordinator, the node did not enrol there"
        );
        assert!(said.is_some_and(|line| line.contains("203.0.113.9:9797")));
        assert_eq!(
            crate::registered_with(&node.home).as_deref(),
            Some("203.0.113.9:9797")
        );
    }

    /// A scripted world for [`park_loop`]: each park pops the next answer
    /// (and may make the machine reachable), every park and announcement is
    /// written down.
    struct FakeSite {
        unreachable: std::cell::Cell<bool>,
        relays: Vec<(DeviceId, String)>,
        answers: std::cell::RefCell<std::collections::VecDeque<(Option<bool>, bool)>>,
        parks: std::cell::RefCell<Vec<DeviceId>>,
        announced: std::cell::RefCell<Vec<Option<DeviceId>>>,
    }

    impl FakeSite {
        /// `answers`: per park, `Some(served)` or `None` for a refusal, and
        /// whether the machine is reachable once it returns.
        fn new(relays: &[DeviceId], answers: &[(Option<bool>, bool)]) -> Self {
            Self {
                unreachable: std::cell::Cell::new(true),
                relays: relays
                    .iter()
                    .map(|d| (*d, format!("203.0.113.{}:9797", d.as_bytes()[0])))
                    .collect(),
                answers: std::cell::RefCell::new(answers.iter().copied().collect()),
                parks: std::cell::RefCell::default(),
                announced: std::cell::RefCell::default(),
            }
        }

        fn parks(&self) -> Vec<DeviceId> {
            self.parks.borrow().clone()
        }
    }

    impl ParkSite for FakeSite {
        fn unreachable(&self) -> bool {
            self.unreachable.get()
        }

        fn relays(&self) -> Vec<(DeviceId, String)> {
            self.relays.clone()
        }

        fn park(&self, relay: DeviceId, _address: &str) -> itsanas_net::Result<bool> {
            self.parks.borrow_mut().push(relay);
            let (served, reachable) = self
                .answers
                .borrow_mut()
                .pop_front()
                .unwrap_or((Some(false), false));
            if reachable {
                self.unreachable.set(false);
            }
            served.ok_or_else(|| {
                itsanas_net::NetError::Io(std::io::Error::other("refused by the relay"))
            })
        }

        fn announce(&self, relay: Option<(DeviceId, String)>) {
            self.announced.borrow_mut().push(relay.map(|(d, _)| d));
        }
    }

    fn id(byte: u8) -> DeviceId {
        DeviceId::from_bytes([byte; 32])
    }

    /// Run `looks` looks of the loop over `site`, as device `me`.
    fn park_looks(site: &FakeSite, me: DeviceId, looks: usize) {
        let stop = AtomicBool::new(false);
        let mut left = looks;
        park_loop(site, me, &stop, || {
            let more = left > 0;
            left = left.saturating_sub(1);
            more
        });
    }

    #[test]
    fn red_team_a_relay_that_closes_the_park_is_passed_over_for_the_next() {
        // A relay that keeps closing parks (full, or going down) would be
        // dialled at every look while a second one sat unused: this machine
        // unreachable for as long as the first stays broken.
        let site = FakeSite::new(&[id(1), id(2)], &[(Some(false), false), (None, false)]);
        park_looks(&site, id(9), 3);
        assert_eq!(
            site.parks()[..2],
            [id(1), id(2)],
            "the relay that closed the park was asked again before the other"
        );
    }

    #[test]
    fn red_team_once_every_relay_has_failed_the_first_is_tried_again() {
        // The skip list is a rotation, not a ban: a relay that was down an
        // hour ago is the one to use when it is the only one.
        let site = FakeSite::new(&[id(1)], &[(None, false), (Some(false), false)]);
        park_looks(&site, id(9), 3);
        assert!(
            site.parks().len() >= 2,
            "a machine with one relay stopped parking for good after one refusal: {:?}",
            site.parks()
        );
    }

    #[test]
    fn red_team_a_machine_never_parks_on_itself() {
        // The book lists the account's machines, this one included; parking
        // on itself is a socket to its own listener and no way in.
        let site = FakeSite::new(&[id(9), id(1)], &[]);
        park_looks(&site, id(9), 2);
        assert!(
            !site.parks().contains(&id(9)),
            "the machine parked on itself: {:?}",
            site.parks()
        );
    }

    #[test]
    fn red_team_a_parked_machine_parks_again_after_each_caller_and_withdraws_when_reached() {
        // One caller per park: without parking again the second caller finds
        // nobody. And once a direct path exists (the router opened a port),
        // the `relay:` address must leave the presence, or callers keep
        // taking the slow road through a third machine.
        let site = FakeSite::new(
            &[id(1)],
            &[(Some(true), false), (Some(true), false), (Some(true), true)],
        );
        park_looks(&site, id(9), 2);
        assert_eq!(
            site.parks(),
            vec![id(1); 3],
            "the machine did not park again after a caller, or kept parking once reachable"
        );
        assert_eq!(
            site.announced.borrow().last().copied(),
            Some(None),
            "a machine reachable again still announces itself behind a relay"
        );
    }

    #[test]
    fn red_team_a_vault_that_drops_while_running_takes_no_chunk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = dir.path().join("nas").join("vault");
        std::fs::create_dir_all(&vault).expect("vault");
        std::fs::write(vault.join(itsanas_node::node::VAULT_MARKER), "device").expect("marker");
        let room = disk_room(vault.clone());
        assert_ne!(room(), Some(0), "a mounted vault with room offered none");
        // The NAS unmounts: an empty local mount point is left.
        std::fs::remove_file(vault.join(itsanas_node::node::VAULT_MARKER)).expect("unmount");
        assert_eq!(
            room(),
            Some(0),
            "chunks were still accepted into a vault whose disk dropped: they would land on \
             the system disk and vanish under the mount when it returns"
        );
    }

    #[test]
    fn a_panic_anywhere_in_the_scope_raises_the_shutdown_flag() {
        // Without this the daemon hangs instead of dying: the scope joins the
        // listener thread, the listener waits on a flag the panic skipped, and
        // the process stays alive, serving, never syncing, and looking healthy
        // to systemd. A supervisor that cannot tell a working daemon from a
        // hung one is not supervising anything.
        let flag = AtomicBool::new(false);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _stop = StopEverythingOnDrop(&flag);
            panic!("the sync loop died");
        }));
        assert!(outcome.is_err(), "the panic did not reach the caller");
        assert!(
            flag.load(Ordering::Relaxed),
            "the daemon would have hung rather than exited"
        );
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

    /// Only what changed here sends a round early: files written out from
    /// the store came from a peer, and two daemons answering each other's
    /// writes with rounds would never be quiet.
    #[test]
    fn only_a_change_made_here_calls_for_an_early_round() {
        let mut report = itsanas_folder::ReconcileReport::default();
        report.exported.push("a".into());
        report.deleted_from_disk.push("b".into());
        assert!(!changed_here(&report), "a peer's file called for a round");
        let mut imported = itsanas_folder::ReconcileReport::default();
        imported.imported.push("c".into());
        assert!(changed_here(&imported));
        let mut deleted = itsanas_folder::ReconcileReport::default();
        deleted.removed_from_store.push("d".into());
        assert!(changed_here(&deleted));
    }

    #[test]
    fn shutdown_is_noticed_quickly_enough_to_feel_immediate() {
        // A Ctrl-C that took a whole interval to be noticed would be
        // indistinguishable from a hang.
        assert!(SHUTDOWN_POLL <= Duration::from_millis(500));
    }
}
