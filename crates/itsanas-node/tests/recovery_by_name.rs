//! Recovering an account from its name and passphrase, against a real
//! coordinator, as somebody typing names at it sees it.
//!
//! A recovery by name is the one thing a stranger can ask a coordinator with
//! nothing but a guess, so what it says when it fails must not depend on
//! which part of the guess was wrong.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use itsanas_coord::Directory;
use itsanas_coord::server::CoordServer;
use itsanas_crypto::{DeviceKeys, SecretBytes};
use itsanas_node::{coordinator, node::Node};

const PASSPHRASE: &str = "a passphrase for a test and nowhere else";
const NOW: u64 = 1_700_000_000;

struct StopOnDrop<'a>(&'a AtomicBool, SocketAddr);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.1);
    }
}

fn with_coordinator<T>(body: impl FnOnce(SocketAddr) -> T) -> T {
    let dir = tempfile::tempdir().expect("temp dir");
    let directory = Directory::open(dir.path().join("directory.redb")).expect("directory");
    let server = CoordServer::bind("127.0.0.1:0").expect("bind");
    let address = server.local_addr().expect("address");
    let shutdown = AtomicBool::new(false);
    let device = DeviceKeys::from_seed(&SecretBytes::new([0xC0; 32]));
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&directory, &device, &shutdown, |_| {});
        });
        let _stop = StopOnDrop(&shutdown, address);
        body(address)
    })
}

/// THE ATTACK: type names at a coordinator's recovery with any passphrase,
/// and read what comes back, and how fast. "No recovery container for this
/// name" came at once for a name with nothing; "wrong passphrase" came after
/// an Argon2id run for a name that holds one. Both said which names are worth
/// a guessing attack.
#[test]
fn red_team_a_wrong_name_and_a_wrong_passphrase_read_the_same() {
    with_coordinator(|address| {
        let home = tempfile::tempdir().expect("home");
        let (mut node, _) = Node::create(home.path(), PASSPHRASE, "alice").expect("create");
        node.config.coordinator = Some(address.to_string());
        node.save_config().expect("save config");
        coordinator::register_with(&node, None, NOW).expect("enrol");
        coordinator::set_escrow(&node, Some(PASSPHRASE), &node.secrets).expect("escrow");
        let at = address.to_string();

        let attempt = |name: &str, passphrase: &str| {
            let started = Instant::now();
            let outcome = coordinator::fetch_escrow(&at, None, name, passphrase);
            (
                outcome.map_err(|error| error.to_string()),
                started.elapsed(),
            )
        };
        let (opened, _) = attempt("alice", PASSPHRASE);
        assert!(
            opened.is_ok(),
            "the right name and passphrase did not open: {opened:?}"
        );

        // What one Argon2id run costs on this machine: the difference a client
        // that skipped it for an unknown name would show. The quickest of three
        // of each, so a scheduler's hiccup does not decide.
        let kdf = {
            let sealed = itsanas_crypto::Keystore::lock(
                PASSPHRASE,
                "a label",
                &[0; 64],
                itsanas_crypto::KdfParams::RECOMMENDED,
            )
            .expect("lock");
            let started = Instant::now();
            let _ = sealed.unlock("not the passphrase", "a label");
            started.elapsed()
        };
        // Taken in turns, the quickest of four each (a name's budget is
        // `ESCROW_ATTEMPTS`, five, and one opened above): a slow stretch of a
        // shared runner (a macOS one put 140 ms between two blocks of three
        // taken one after the other) then falls on both alike.
        let (mut wrong_passphrase, mut wrong_name) = (None, None);
        let (mut slow, mut fast) = (std::time::Duration::MAX, std::time::Duration::MAX);
        for _ in 0..4 {
            let (said, took) = attempt("alice", "not the passphrase");
            wrong_passphrase.get_or_insert(said);
            slow = slow.min(took);
            let (said, took) = attempt("personne", PASSPHRASE);
            wrong_name.get_or_insert(said);
            fast = fast.min(took);
        }
        assert_eq!(
            wrong_name, wrong_passphrase,
            "a wrong name and a wrong passphrase read differently: whoever types names learns which exist"
        );
        assert!(
            fast + kdf / 2 >= slow,
            "a wrong name failed in {fast:?} and a wrong passphrase in {slow:?}, an Argon2id run ({kdf:?}) apart: the time says which"
        );
    });
}
