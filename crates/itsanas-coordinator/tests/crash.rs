//! The coordinator killed outright, then reopened by another process.
//!
//! `announce` commits without waiting for the disk (`Durability::None`): a
//! presence is a heartbeat the next round restores. Everything else --
//! accounts, claims, escrow -- keeps the default and must survive a crash.
//! An in-process reopen cannot tell the two apart (redb flushes when the
//! `Database` is dropped, so even non-durable commits come back), which is
//! why this kills a real process and reads its state from this one.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use itsanas_coord::Directory;
use itsanas_coord::claim::{NodeClaim, Presence};
use itsanas_coord::directory::Registration;
use itsanas_coord::protocol::{Request, Response};
use itsanas_coord::server::CoordClient;
use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};

/// The coordinator on loopback, and the address it printed.
fn start(state: &std::path::Path) -> (Child, std::net::SocketAddr) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_itsanas-coordinator"))
        .arg("--state")
        .arg(state)
        .args(["--listen", "127.0.0.1:0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the coordinator");
    let mut out = BufReader::new(child.stdout.take().expect("stdout")).lines();
    while let Some(line) = out.next() {
        let line = line.expect("read the coordinator's output");
        if let Some(address) = line.trim().strip_prefix("listening ") {
            // Keep reading: with the pipe closed, the coordinator's next
            // `println!` fails and it dies before serving (seen on Windows).
            std::thread::spawn(move || out.for_each(drop));
            return (child, address.parse().expect("a socket address"));
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("the coordinator exited without saying where it listens");
}

#[test]
fn red_team_a_coordinator_killed_mid_round_keeps_every_account_it_confirmed() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_secs();
    let state = tempfile::tempdir().expect("temp dir");
    let (mut coordinator, address) = start(state.path());

    let owner = UserKeys::derive(&MasterSecret::from_bytes([7; 32]));
    let laptop = DeviceKeys::from_seed(&SecretBytes::new([7; 32]));
    let mut client = CoordClient::connect(address, &laptop, None).expect("dial");
    let registration = Registration {
        username: "camille".to_owned(),
        user: owner.public(),
        issued_unix: now,
    }
    .sign(&owner);
    assert!(matches!(
        client
            .ask(&Request::Register(Box::new(registration)))
            .unwrap(),
        Response::Account(_)
    ));
    let claim = NodeClaim {
        owner: owner.user_id(),
        device: laptop.device_id(),
        pledged_bytes: 1 << 30,
        issued_unix: now,
        revoked: false,
    }
    .sign(&owner);
    assert!(matches!(
        client.ask(&Request::Claim(Box::new(claim))).unwrap(),
        Response::Done
    ));
    // The one non-durable write, last: the kill lands right after it.
    let presence = Presence {
        device: laptop.device_id(),
        address: "192.168.1.20:9797".to_owned(),
        at_unix: now,
    }
    .sign(&laptop);
    assert!(matches!(
        client.ask(&Request::Announce(Box::new(presence))).unwrap(),
        Response::Done
    ));

    // TerminateProcess on Windows, SIGKILL elsewhere: no destructor runs.
    coordinator.kill().expect("kill the coordinator");
    coordinator.wait().expect("reap it");

    let directory = Directory::open(state.path().join("directory.redb"))
        .expect("a killed coordinator's state did not reopen");
    assert!(
        directory.account("camille").expect("read").is_some(),
        "an account the coordinator had confirmed was gone after a crash: its \
         owner's name is free for anyone to take"
    );
    assert_eq!(
        directory.enrolled_counts().expect("counts"),
        (1, 1),
        "a machine the coordinator had enrolled was gone after a crash, or its \
         claim and the per-account index disagree"
    );
}
