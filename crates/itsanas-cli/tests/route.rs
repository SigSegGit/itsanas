//! Test C's only route, run for real: another account's node, found by LAN
//! discovery alone, hosts this account's chunks.
//!
//! The coordinator lists only the asker's own account and a relay never
//! introduces a machine, so the one way `test`'s chunks reach `nicolas` on the
//! fleet is a `test` node on the home LAN that hears `nicolas`'s beacon and
//! dials it as a stranger (`dial_order`, at most `NEW_PEERS_PER_ROUND` a
//! round), then pushes (HANDOVER §8 0x). The fleet's C row said SKIP for six
//! nights while that route was broken by a zero pledge and a closed UDP port,
//! and no cargo test noticed: `acceptance-local.sh` proves hosting between
//! accounts, but through `peer add`, and `lan.rs` proves the beacons, but not
//! that a daemon acts on them.
//!
//! Two accounts on one machine, both pledging, discovery on over real sockets
//! (moved off 21037 by `ITSANAS_DISCOVERY_PORT`), no `peer add`: after a few
//! rounds the second account's `vault/owners/<first uid>` holds chunks.
//!
//! # Why this is `#[ignore]`d
//!
//! Two `init`s and two daemons, each deriving the keystore key (Argon2id, slow
//! on purpose). The release job for ignored tests runs it, on Linux.
//!
//! # What it does not cover
//!
//! Two machines: the broadcast stays on this host. That two containers on the
//! VM hear each other is what the fleet's C row shows each night.

use std::{
    net::{SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use itsanas_coord::{Directory, server::CoordServer};
use itsanas_crypto::{DeviceKeys, SecretBytes};

const PASSPHRASE: &str = "route-test-passphrase";

/// Several rounds at the test interval, for beacons to be heard and a
/// stranger to be dialled; a route that does not work fails well inside it.
const PATIENCE: Duration = Duration::from_secs(150);

fn itsanas(home: &Path, port: u16) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_itsanas"));
    command
        .arg("--home")
        .arg(home)
        .env("ITSANAS_PASSPHRASE", PASSPHRASE)
        .env("ITSANAS_DISCOVERY_PORT", port.to_string())
        .env_remove("ITSANAS_INSTANCE")
        .stdin(Stdio::null());
    command
}

fn run(home: &Path, port: u16, args: &[&str]) -> String {
    let output = itsanas(home, port)
        .args(args)
        .output()
        .expect("run itsanas");
    assert!(
        output.status.success(),
        "itsanas {args:?} failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Killed and reaped however the test ends.
struct Daemon {
    child: Child,
    log: PathBuf,
}

impl Daemon {
    fn start(home: &Path, port: u16, log: PathBuf) -> Self {
        let out = std::fs::File::create(&log).expect("daemon log");
        let err = out.try_clone().expect("daemon log");
        let child = itsanas(home, port)
            // Every address, not loopback: a beacon names the LAN address it
            // left from, and that is where the other account dials.
            .args(["daemon", "--listen", "0.0.0.0:0", "--interval", "5"])
            .stdout(out)
            .stderr(err)
            .spawn()
            .expect("start the daemon");
        Self { child, log }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A free UDP port, so neither a real daemon on this machine nor another test
/// shares the beacons.
fn free_port() -> u16 {
    UdpSocket::bind("0.0.0.0:0")
        .expect("a free port")
        .local_addr()
        .expect("its address")
        .port()
}

/// Bytes `home` keeps for owners other than its own account.
fn hosted_for_others(home: &Path) -> u64 {
    fn size(path: &Path) -> u64 {
        match std::fs::read_dir(path) {
            Ok(entries) => entries.flatten().map(|entry| size(&entry.path())).sum(),
            Err(_) => std::fs::metadata(path).map_or(0, |m| m.len()),
        }
    }
    std::fs::read_dir(home.join("vault").join("owners")).map_or(0, |owners| {
        owners.flatten().map(|owner| size(&owner.path())).sum()
    })
}

/// The fleet's coordinator, in this process: it confirms each account's
/// devices (a host stores for another account only once it has, Nicolas,
/// 2026-10-05) and introduces nobody -- discovery still does that.
struct Coordinator {
    address: SocketAddr,
    device: String,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl Coordinator {
    fn start() -> Self {
        let dir = tempfile::tempdir().expect("coordinator dir");
        let directory = Directory::open(dir.path().join("directory.redb")).expect("directory");
        let server = CoordServer::bind("127.0.0.1:0").expect("bind the coordinator");
        let address = server.local_addr().expect("its address");
        let keys = DeviceKeys::from_seed(&SecretBytes::new([0xC0; 32]));
        let device = keys.device_id().to_hex();
        let shutdown = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&shutdown);
        let thread = std::thread::spawn(move || {
            let _ = server.serve_until(&directory, &keys, &stop, |_| {});
        });
        Self {
            address,
            device,
            shutdown,
            thread: Some(thread),
            _dir: dir,
        }
    }
}

impl Drop for Coordinator {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Unix seconds, as the daemon stamps its snapshot.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// The first line of the snapshot `home`'s daemon writes, or "".
fn header(home: &Path) -> String {
    std::fs::read_to_string(home.join("status.snapshot"))
        .unwrap_or_default()
        .lines()
        .next()
        .unwrap_or("")
        .to_owned()
}

/// Whether a header carries a contact time, `reached T` (not `reached never
/// since T`): what `isolated` ages.
fn has_contact(header: &str) -> bool {
    let words: Vec<&str> = header.split_whitespace().collect();
    words
        .iter()
        .position(|word| *word == "reached")
        .and_then(|at| words.get(at + 1))
        .is_some_and(|word| word.parse::<u64>().is_ok())
}

/// Two accounts on one machine, the host pledging `host_pledge`, both pinned
/// to a coordinator when `coordinated`. Waits until `done` holds of (what the
/// host keeps for the other, the owner's log, the owner's snapshot header),
/// or the patience runs out; returns them and the host's log.
fn route(
    host_pledge: &str,
    coordinated: bool,
    done: impl Fn(u64, &str, &str) -> bool,
) -> (u64, String, String, String) {
    let base = tempfile::tempdir().expect("scratch");
    let port = free_port();
    let coordinator = coordinated.then(Coordinator::start);
    let (owner, host) = (base.path().join("owner"), base.path().join("host"));
    let folder = base.path().join("owner-folder");
    std::fs::create_dir(&folder).expect("folder");
    run(&owner, port, &["init", "--username", "routeowner"]);
    run(&host, port, &["init", "--username", "routehost"]);
    for home in [&owner, &host] {
        if let Some(coordinator) = &coordinator {
            let address = coordinator.address.to_string();
            run(
                home,
                port,
                &["coordinator", &address, "--device", &coordinator.device],
            );
        }
    }
    run(&owner, port, &["pledge", "1G"]);
    run(&host, port, &["pledge", host_pledge]);
    std::fs::write(folder.join("photo.jpg"), vec![7u8; 300_000]).expect("a file");
    run(
        &owner,
        port,
        &["folder", folder.to_str().expect("utf-8 path"), "--confirm"],
    );

    let owner_daemon = Daemon::start(&owner, port, base.path().join("owner.log"));
    let host_daemon = Daemon::start(&host, port, base.path().join("host.log"));
    let deadline = Instant::now() + PATIENCE;
    let mut held = 0;
    while Instant::now() < deadline {
        held = hosted_for_others(&host);
        if done(held, &owner_daemon.log(), &header(&owner)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    (held, owner_daemon.log(), host_daemon.log(), header(&owner))
}

/// And the host that kept the chunks, a stranger found on the LAN that
/// earned trust, is a contact: the owner's header carries `reached T`, or a
/// laptop whose only other copy is a neighbour's host would read `isolated`
/// while it syncs. Sabotage: the LAN loop of `one_round` counting nobody.
#[test]
#[ignore = "two Argon2id derivations and two running daemons; the release job runs it"]
fn red_team_another_account_found_by_discovery_alone_hosts_the_chunks() {
    let (held, owner_log, host_log, owner_header) = route("1G", true, |held, _, header| {
        held > 0 && has_contact(header)
    });
    assert!(
        held > 0,
        "after {PATIENCE:?} the other account's node, on the same LAN, pledging and \
         confirmed by the coordinator, holds nothing for this one: discovery does not \
         lead to hosting between accounts, and test C has no route on the fleet\n\
         --- owner\n{owner_log}\n--- host\n{host_log}"
    );
    assert!(
        has_contact(&owner_header),
        "a host found on the LAN kept this account's chunks and the owner's header has no \
         contact: it reads `isolated` six hours later while it syncs ({owner_header})\n\
         --- owner\n{owner_log}"
    );
}

#[test]
#[ignore = "two Argon2id derivations and two running daemons; the release job runs it"]
fn red_team_a_host_without_a_coordinator_is_said_to_wait_for_one_not_to_reject_segments() {
    const WHY: &str = "only once its coordinator has confirmed this device";
    let (held, owner_log, _, _) = route("1G", false, |_, log, _| log.contains(WHY));
    assert_eq!(
        held, 0,
        "a host stored for a device no coordinator confirmed"
    );
    assert!(
        owner_log.contains(WHY) && !owner_log.contains("a segment that does not verify"),
        "the owner was not told the host waits for its coordinator: it would go after \
         segments and chains\n--- owner\n{owner_log}"
    );
}

/// And that host, a stranger that answered every round and kept nothing, is
/// no contact: the owner's header still says `reached never since`, or a
/// laptop on a café LAN would read green all week with nothing of its
/// account synced. Waited for until a snapshot stamped after the refusal
/// was first seen, so it is the one written after that round. Sabotage:
/// every machine that answered counts (`counts_as_contact`).
#[test]
#[ignore = "two Argon2id derivations and two running daemons; the release job runs it"]
fn red_team_a_host_that_pledges_nothing_hosts_nothing_and_says_so() {
    const WHY: &str = "pledge is full or zero";
    let said_at = std::cell::Cell::new(None);
    let (held, owner_log, _, owner_header) = route("0", true, |_, log, header| {
        if said_at.get().is_none() && log.contains(WHY) {
            said_at.set(Some(now()));
        }
        let stamp = header
            .strip_prefix("snapshot ")
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|stamp| stamp.parse::<u64>().ok());
        said_at
            .get()
            .zip(stamp)
            .is_some_and(|(said, stamp)| stamp > said)
    });
    assert_eq!(
        held, 0,
        "a host that pledged nothing stored another account's chunks"
    );
    assert!(
        owner_log.contains(WHY),
        "the owner was not told the host pledges nothing\n--- owner\n{owner_log}"
    );
    assert!(
        owner_header.contains(" reached never since "),
        "a stranger that answered and kept nothing counted as a contact: a cut-off laptop \
         next to one reads green ({owner_header})\n--- owner\n{owner_log}"
    );
}
