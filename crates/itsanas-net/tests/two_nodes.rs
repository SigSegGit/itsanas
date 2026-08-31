//! Two nodes, one socket, real bytes.
//!
//! The M4 exit criterion. Everything below the socket is real: real stores,
//! real chunking, real sealing, real signatures, real TCP. The only thing these
//! tests simulate is which machine each node is running on.
//!
//! Each test runs a server in a scoped thread and drives a client from the test
//! thread, so a failure is a normal assertion in the normal place rather than a
//! panic in a detached thread that the harness reports as a hang.

use std::sync::atomic::{AtomicBool, Ordering};

use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};
use itsanas_net::{
    PeerClient, PeerServer, PeerService, Pledge,
    protocol::{Request, Response},
    session,
};
use itsanas_store::{AtRisk, ChunkerConfig, Store, Vault};

/// One machine: its own store, and a vault for other people's data.
struct Node {
    _dir: tempfile::TempDir,
    store: Store,
    vault: Vault,
    device: DeviceKeys,
}

fn node(master: &MasterSecret, device_seed: u8) -> Node {
    let dir = tempfile::tempdir().expect("temp dir");
    let device = DeviceKeys::from_seed(&SecretBytes::new([device_seed; 32]));
    let store = Store::open_for_testing(
        dir.path().join("store"),
        UserKeys::derive(master),
        DeviceKeys::from_seed(&device.seed()),
        ChunkerConfig::default(),
    )
    .expect("store");
    let vault = Vault::open(dir.path().join("vault")).expect("vault");

    Node {
        _dir: dir,
        store,
        vault,
        device,
    }
}

fn alice() -> MasterSecret {
    MasterSecret::from_bytes([0xA1; 32])
}

/// Stops the server even when the body panics.
///
/// Without it a failing assertion leaves the accept loop running,
/// `thread::scope` never returns, and the harness reports a **hang** instead of
/// the assertion.
///
/// That is worse than it sounds here. Every red-team test in this file runs
/// inside `with_server`, so for as long as this was missing, the tests written
/// to catch an attack reported a timeout when they caught one — and a timeout
/// reads like flakiness, which is the thing everybody retries and nobody
/// investigates. Found by sabotaging a verification step on purpose and
/// watching the suite hang instead of fail.
///
/// `itsanas-coord`'s test harness has had exactly this guard, with exactly this
/// rationale written above it, since the day its server was written. It was
/// never applied here.
struct StopOnDrop<'a>(&'a AtomicBool, std::net::SocketAddr);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
        // Unblock the accept loop's sleep by connecting once more.
        let _ = std::net::TcpStream::connect(self.1);
    }
}

/// Run `body` with `server_node` serving on loopback.
///
/// The server stops as soon as `body` returns, whether it returned or panicked.
fn with_server<T>(
    server_node: &Node,
    pledge: Pledge,
    body: impl FnOnce(std::net::SocketAddr) -> T,
) -> T {
    let server = PeerServer::bind("127.0.0.1:0").expect("bind loopback");
    let address = server.local_addr().expect("local address");
    let shutdown = AtomicBool::new(false);

    let service = PeerService::new(&server_node.store, &server_node.vault, pledge);

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&service, &server_node.device, &shutdown);
        });

        let _stop = StopOnDrop(&shutdown, address);
        body(address)
    })
}

#[test]
fn a_failing_assertion_inside_a_server_scope_fails_rather_than_hangs() {
    // This test passing means it *finished*. There is no assertion that can
    // catch the failure it guards against, because the failure is that nothing
    // returns: `thread::scope` joins the accept loop, the accept loop waits for
    // a shutdown flag that the panic skipped past, and the suite sits there
    // until the harness gives up sixty seconds later and calls it a hang.
    //
    // Every red-team test in this file runs inside `with_server`. For as long
    // as this was broken, a test that caught an attack reported a timeout, and
    // a timeout is what everybody retries and nobody reads.
    let node = node(&alice(), 60);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_server(&node, Pledge::gigabytes(1), |_address| {
            panic!("a failing assertion, as an assertion would");
        })
    }));
    assert!(outcome.is_err(), "the panic did not even reach the caller");
}

// ---------------------------------------------------------------------------
// The exit criterion
// ---------------------------------------------------------------------------

#[test]
fn two_nodes_sync_a_file_over_a_real_socket() {
    let laptop = node(&alice(), 1);
    let pi = node(&alice(), 2);

    laptop
        .store
        .write_file("reports/q3.txt", b"written on the laptop")
        .unwrap();
    laptop.store.flush_segment().unwrap();

    with_server(&laptop, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &pi.device, pi.store.owner(), None).expect("connect");

        assert_eq!(
            client.peer_device(),
            laptop.store.device_id(),
            "the peer identified itself as the wrong device"
        );

        let report = session::round(&pi.store, &pi.vault, &mut client).expect("sync round");
        assert!(
            report.pull.adopted > 0,
            "nothing was adopted from the peer: {report:?}"
        );
    });

    assert_eq!(
        pi.store.read_file("reports/q3.txt").unwrap().unwrap(),
        b"written on the laptop",
        "the file did not cross the wire intact"
    );
}

#[test]
fn a_larger_file_survives_the_wire_byte_for_byte() {
    // Several chunks, so the fetch loop and the reassembly both get exercised.
    let laptop = node(&alice(), 3);
    let pi = node(&alice(), 4);

    let payload = itsanas_testkit::filler("over-the-wire", 3 * 1024 * 1024);
    laptop.store.write_file("big.bin", &payload).unwrap();
    laptop.store.flush_segment().unwrap();

    let entry = laptop.store.stat("big.bin").unwrap().unwrap();
    assert!(entry.chunks.len() > 10, "the test payload was too small");

    with_server(&laptop, Pledge::gigabytes(1), |address| {
        let mut client = PeerClient::connect(address, &pi.device, pi.store.owner(), None).unwrap();
        session::round(&pi.store, &pi.vault, &mut client).unwrap();
    });

    assert_eq!(
        pi.store.read_file("big.bin").unwrap().unwrap(),
        payload,
        "a multi-chunk file came back different"
    );
}

#[test]
fn syncing_twice_transfers_nothing_the_second_time() {
    // Without the have/missing exchange this re-uploads everything on every
    // round, which at real sizes saturates the link forever.
    let laptop = node(&alice(), 5);
    let pi = node(&alice(), 6);

    laptop
        .store
        .write_file("stable.txt", b"unchanging content")
        .unwrap();
    laptop.store.flush_segment().unwrap();

    with_server(&laptop, Pledge::gigabytes(1), |address| {
        let mut client = PeerClient::connect(address, &pi.device, pi.store.owner(), None).unwrap();

        let first = session::round(&pi.store, &pi.vault, &mut client).unwrap();
        assert!(first.changed_anything());

        let second = session::round(&pi.store, &pi.vault, &mut client).unwrap();
        assert_eq!(
            second.pull.adopted, 0,
            "the second round re-adopted work it already had"
        );
        assert_eq!(
            second.push.chunks_offered, 0,
            "the second round re-offered chunks the peer already holds"
        );
        assert!(
            !second.changed_anything(),
            "a quiet round reported progress: {second:?}"
        );
    });
}

#[test]
fn a_node_that_only_ever_accepts_connections_still_learns_what_was_pushed_to_it() {
    // A real bug, found by running two daemons where only one had the other
    // configured as a peer. A push lands in the receiving node's *vault*, and
    // only `pull` applies segments to the store — so the node that never dialled
    // anybody held its own data and never looked at it.
    //
    // Not a corner case: a device behind NAT can push and cannot be dialled, so
    // for its peers this is the only way its work ever arrives.
    let listener = node(&alice(), 24);
    let dialler = node(&alice(), 25);

    dialler
        .store
        .write_file("pushed.txt", b"sent by the device that dialled")
        .unwrap();
    dialler.store.flush_segment().unwrap();

    with_server(&listener, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &dialler.device, dialler.store.owner(), None).unwrap();
        session::push(&dialler.store, &mut client).expect("push");
    });

    // The listener never pulled, and never will.
    assert_eq!(
        listener.store.read_file("pushed.txt").unwrap(),
        None,
        "this test is meaningless if a push alone already reached the store"
    );

    let report = session::drain_vault(&listener.store, &listener.vault).unwrap();

    assert!(report.adopted > 0, "nothing was drained: {report:?}");
    assert_eq!(
        listener.store.read_file("pushed.txt").unwrap().unwrap(),
        b"sent by the device that dialled",
        "a node that only accepts connections never learned what it was holding"
    );

    // And draining again is a no-op, or a daemon would churn forever.
    let second = session::drain_vault(&listener.store, &listener.vault).unwrap();
    assert!(
        !second.changed_anything(),
        "draining twice changed state: {second:?}"
    );
}

// ---------------------------------------------------------------------------
// Hosting for someone else
// ---------------------------------------------------------------------------

#[test]
fn a_host_stores_a_strangers_data_and_cannot_read_a_byte_of_it() {
    // The bargain, over a real socket. Bob's laptop hosts Alice's data.
    let alice_keys = itsanas_testkit::alice();
    let alice_device = node(&alice_keys.master, 7);
    let bob_host = node(&MasterSecret::from_bytes([0xB0; 32]), 8);

    for file in &alice_keys.files {
        alice_device
            .store
            .write_file(file.path, &file.content)
            .unwrap();
    }
    alice_device.store.flush_segment().unwrap();

    with_server(&bob_host, Pledge::gigabytes(1), |address| {
        let mut client = PeerClient::connect(
            address,
            &alice_device.device,
            alice_device.store.owner(),
            None,
        )
        .unwrap();

        let report = session::push(&alice_device.store, &mut client).expect("push to host");
        assert!(report.chunks_accepted > 0, "the host took nothing");
        assert!(report.segments_accepted > 0);
    });

    // Bob is now holding Alice's data.
    let stats = bob_host.vault.stats().unwrap();
    assert_eq!(stats.owners, 1);
    assert!(stats.chunks > 0);
    assert!(stats.bytes > 0);

    // And cannot read any of it.
    let mut everything = Vec::new();
    for address in bob_host
        .vault
        .chunks_for(alice_keys.keys.user_id())
        .unwrap()
    {
        let sealed = bob_host
            .vault
            .get_chunk(alice_keys.keys.user_id(), &address)
            .unwrap()
            .unwrap();

        assert!(
            bob_host
                .store
                .read_file("anything")
                .map(|f| f.is_none())
                .unwrap_or(true),
            "the host's own store somehow gained Alice's file"
        );
        everything.extend_from_slice(&sealed);
    }

    let canary = itsanas_testkit::ALICE_CANARY.as_bytes();
    assert!(
        !everything.is_empty(),
        "the host holds nothing, so this proves nothing"
    );
    assert!(
        !everything
            .windows(canary.len())
            .any(|window| window == canary),
        "Alice's plaintext is sitting on Bob's disk"
    );
}

#[test]
fn a_host_relays_one_device_to_another_that_it_never_met() {
    // The scenario the whole architecture exists for, over a socket this time.
    // The Pi pushes to a host and switches off. The VM, which has never spoken
    // to the Pi, pulls the Pi's work from that host.
    let host = node(&MasterSecret::from_bytes([0xB1; 32]), 9);
    let pi = node(&alice(), 10);
    let vm = node(&alice(), 11);

    pi.store
        .write_file("from-the-pi.txt", b"the Pi wrote this at 3am")
        .unwrap();
    pi.store.flush_segment().unwrap();

    with_server(&host, Pledge::gigabytes(1), |address| {
        // The Pi uploads, then conceptually powers off.
        let mut pi_client =
            PeerClient::connect(address, &pi.device, pi.store.owner(), None).unwrap();
        session::push(&pi.store, &mut pi_client).unwrap();
        drop(pi_client);

        // The VM arrives later and has never met the Pi.
        let mut vm_client =
            PeerClient::connect(address, &vm.device, vm.store.owner(), None).unwrap();
        let report = session::round(&vm.store, &vm.vault, &mut vm_client).unwrap();
        assert!(
            report.pull.adopted > 0,
            "the VM learned nothing from the host: {report:?}"
        );
    });

    assert_eq!(
        vm.store.read_file("from-the-pi.txt").unwrap().unwrap(),
        b"the Pi wrote this at 3am",
        "a host failed to relay one device's work to another"
    );
}

#[test]
fn a_disk_that_quietly_lost_a_block_gets_it_back_from_a_host() {
    // The half of repair that pushing cannot do. `push` restores *replication*
    // — it offers a peer what the peer lacks — and it can put nothing back on
    // this disk. A chunk missing here is the one failure the placement ledger
    // was built to survive, and until now surviving it meant a human running
    // `doctor`, reading the output, and knowing what to do next.
    //
    // A dropped block, an inode lost to a power cut, a backup restored
    // partially. The file is unreadable; the bytes are on three other machines;
    // nothing was reaching for them.
    let master = alice();
    let host = node(&MasterSecret::from_bytes([0xC1; 32]), 50);
    let owner = node(&master, 51);

    let chunks = owner
        .store
        .write_file("thesis.bin", &a_file_of_many_chunks(21, 512 << 10))
        .expect("write")
        .chunks;
    owner.store.flush_segment().expect("flush");

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        session::push(&owner.store, &mut client).expect("push");
    });

    // The disk loses a block. Not deleted through the API: the index still
    // wants the chunk, which is exactly what makes this a fault rather than a
    // deletion.
    let lost = chunks[chunks.len() / 2];
    let blob = owner.store.blobs().path_of(&lost);
    std::fs::remove_file(&blob).expect("remove the blob behind the store's back");

    assert!(
        owner.store.read_file("thesis.bin").is_err(),
        "the fixture did not actually break the file"
    );

    // Enough rounds for the bounded scan to reach it, wherever the cursor lands.
    let mut restored = 0;
    for _ in 0..12 {
        with_server(&host, Pledge::gigabytes(1), |address| {
            let mut client = PeerClient::connect(address, &owner.device, owner.store.owner(), None)
                .expect("dial");
            let report = session::repair(&owner.store, &mut client, session::REPAIR_SCAN_PER_ROUND)
                .expect("repair");
            restored += report.restored;
        });
        if restored > 0 {
            break;
        }
    }

    assert_eq!(restored, 1, "the lost chunk was never fetched back");
    assert!(
        owner.store.read_file("thesis.bin").expect("read").is_some(),
        "the chunk came back but the file is still unreadable"
    );
}

#[test]
fn red_team_a_stranger_is_not_told_which_chunks_this_node_has_lost() {
    // THE ATTACK, and it is one repair introduced. Asking a peer "do you have
    // chunk X?" tells it this node does not. The ids are blinded so it learns
    // nothing about the content — but it learns which chunks now exist only on
    // hosts, which is exactly the list to delete if you want to destroy
    // somebody's data. A healing mechanism that publishes the map of the
    // wounds.
    //
    // The first version asked every peer it connected to about everything it
    // had lost, strangers the discovery loop had dialled included.
    //
    // If this test fails, anyone who can complete a handshake with this node
    // learns which of its files are one deletion from being gone.
    let master = alice();
    let holder = node(&MasterSecret::from_bytes([0xD1; 32]), 70);
    let stranger = node(&MasterSecret::from_bytes([0xD2; 32]), 71);
    let owner = node(&master, 72);

    let chunks = owner
        .store
        .write_file("private.bin", &a_file_of_many_chunks(31, 256 << 10))
        .expect("write")
        .chunks;
    owner.store.flush_segment().expect("flush");

    // Only the real host is ever given the data, so only it is ever recorded.
    with_server(&holder, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        session::push(&owner.store, &mut client).expect("push");
    });

    // Lose several blocks, so the queue is not empty and the sweep cannot miss.
    for chunk in chunks.iter().take(3) {
        std::fs::remove_file(owner.store.blobs().path_of(chunk)).expect("lose a block");
    }
    let found = owner.store.verify_integrity(false).expect("doctor");
    assert_eq!(
        found.missing_chunks.len(),
        3,
        "the fixture did not break anything"
    );

    // The stranger answers the phone and is asked nothing, however many rounds
    // it hangs around for.
    for _ in 0..8 {
        with_server(&stranger, Pledge::gigabytes(1), |address| {
            let mut client = PeerClient::connect(address, &owner.device, owner.store.owner(), None)
                .expect("dial");
            let report = session::repair(&owner.store, &mut client, session::REPAIR_SCAN_PER_ROUND)
                .expect("repair");
            assert_eq!(
                report.asked, 0,
                concat!(
                    "a peer that has never been given a byte of this node's data ",
                    "was told which chunks it has lost"
                )
            );
            assert!(report.not_asked >= 3, "the losses were not even considered");
        });
    }

    // And the host that does hold them is still asked, or the rule would be
    // privacy bought by breaking the feature.
    let mut restored = 0;
    for _ in 0..4 {
        with_server(&holder, Pledge::gigabytes(1), |address| {
            let mut client = PeerClient::connect(address, &owner.device, owner.store.owner(), None)
                .expect("dial");
            let report = session::repair(&owner.store, &mut client, session::REPAIR_SCAN_PER_ROUND)
                .expect("repair");
            restored += report.restored;
        });
        if restored == 3 {
            break;
        }
    }
    assert_eq!(
        restored, 3,
        "the peer that does hold the data was not asked"
    );
}

#[test]
fn what_doctor_finds_is_what_repair_fixes_first() {
    // Two detectors that ignored each other. `doctor` walks every file and
    // knows the whole answer in one pass; the daemon's sampling scan reaches a
    // given chunk after fifty-five days on a terabyte store. Somebody running
    // `doctor` because a file would not open therefore learned the answer and
    // had no way to act on it, and the fix was the slowest path in the system.
    //
    // The queue is what joins them. This checks that a loss `doctor` found is
    // repaired without waiting for the sampler to stumble across it.
    let master = alice();
    let host = node(&MasterSecret::from_bytes([0xD3; 32]), 73);
    let owner = node(&master, 74);

    let chunks = owner
        .store
        .write_file("ledger.bin", &a_file_of_many_chunks(32, 256 << 10))
        .expect("write")
        .chunks;
    owner.store.flush_segment().expect("flush");

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        session::push(&owner.store, &mut client).expect("push");
    });

    let lost = chunks[chunks.len() - 1];
    std::fs::remove_file(owner.store.blobs().path_of(&lost)).expect("lose a block");
    assert_eq!(
        owner.store.loss_count().expect("count"),
        0,
        "nothing knows yet"
    );

    owner.store.verify_integrity(false).expect("doctor");
    assert_eq!(
        owner.store.loss_count().expect("count"),
        1,
        "doctor found the loss and told nobody"
    );

    // One round. Not a sweep: the queue means the sampler's odds do not matter.
    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        let report = session::repair(&owner.store, &mut client, 0).expect("repair");
        assert_eq!(
            report.restored, 1,
            "a loss doctor had already found was not repaired in the next round"
        );
    });

    assert_eq!(
        owner.store.loss_count().expect("count"),
        0,
        "the chunk came back and the queue still says it is missing"
    );
    assert!(owner.store.read_file("ledger.bin").expect("read").is_some());
}

#[test]
fn red_team_a_host_cannot_answer_a_repair_request_with_rubbish() {
    // THE ATTACK. A host cannot read what it stores, so the one way it could
    // destroy data is to wait until the owner asks for a chunk back and answer
    // with noise.
    //
    // If the bytes were written unverified, `has_chunk` would become true, the
    // repair scan would stop looking, no other peer would ever be asked, and a
    // loss that was **recoverable** would be permanent. That is strictly worse
    // than the host refusing to answer at all, which is the shape of failure
    // this project refuses everywhere else.
    //
    // If this test fails, any host you have ever stored with can destroy any
    // file of yours that your own disk has damaged.
    let master = alice();
    let liar = node(&MasterSecret::from_bytes([0xC2; 32]), 52);
    let owner = node(&master, 53);

    let chunks = owner
        .store
        .write_file("evidence.bin", &a_file_of_many_chunks(22, 256 << 10))
        .expect("write")
        .chunks;
    owner.store.flush_segment().expect("flush");

    with_server(&liar, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        session::push(&owner.store, &mut client).expect("push");
    });

    // Every chunk it holds is replaced with the same length of noise. It still
    // answers every request, promptly, with something.
    for chunk in &chunks {
        let sealed = liar
            .vault
            .get_chunk(owner.store.owner(), chunk)
            .expect("vault")
            .expect("held");
        liar.vault
            .remove_chunk(owner.store.owner(), chunk)
            .expect("remove");
        liar.vault
            .put_chunk(owner.store.owner(), chunk, &vec![0x7Au8; sealed.len()])
            .expect("substitute");
    }

    let lost = chunks[0];
    std::fs::remove_file(owner.store.blobs().path_of(&lost)).expect("lose a block");

    let mut forged = 0;
    for _ in 0..12 {
        with_server(&liar, Pledge::gigabytes(1), |address| {
            let mut client = PeerClient::connect(address, &owner.device, owner.store.owner(), None)
                .expect("dial");
            let report = session::repair(&owner.store, &mut client, session::REPAIR_SCAN_PER_ROUND)
                .expect("repair");
            assert_eq!(
                report.restored, 0,
                "rubbish was accepted as a repaired chunk"
            );
            forged += report.forged;
        });
        if forged > 0 {
            break;
        }
    }

    assert!(forged > 0, "the substitution was never even attempted");
    assert!(
        !owner.store.has_chunk(&lost),
        concat!(
            "the forged bytes were written under the missing chunk's address, ",
            "so the scan will stop looking for it and no other peer will ever ",
            "be asked. A recoverable loss has been made permanent."
        )
    );
    assert!(
        owner
            .store
            .remote_holders(&lost)
            .expect("holders")
            .iter()
            .all(|holder| holder.device != liar.store.device_id()),
        "a host that answered with rubbish is still recorded as holding it"
    );
}

// ---------------------------------------------------------------------------
// Getting everything back
// ---------------------------------------------------------------------------

#[test]
fn a_replacement_device_pulls_a_whole_corpus_back_from_a_stranger() {
    // MVP acceptance test D, the half that was never checked. Recovery from
    // username and passphrase restores the *account* — the user id, the keys,
    // the ability to speak. It says nothing at all about whether the files come
    // back, and that is the only part the user cares about.
    //
    // So: a machine writes a corpus, edits one file, deletes another, uploads
    // to a host belonging to somebody else entirely, and is then destroyed. A
    // replacement device is built from the same master secret with a **new
    // device id**, has never met the machine that is gone, and pulls.
    //
    // What has to survive the trip: file contents byte for byte, an edit
    // arriving as the edit rather than the original, and a deletion arriving as
    // a deletion rather than as a resurrected file. The last is the one that
    // fails quietly — a restore that brings back everything you ever deleted
    // looks like it worked.
    let master = alice();
    let host = node(&MasterSecret::from_bytes([0xB7; 32]), 40);

    let corpus: Vec<(String, Vec<u8>)> = (0..12)
        .map(|index| {
            (
                format!("documents/report-{index:02}.bin"),
                a_file_of_many_chunks(index, 96 << 10),
            )
        })
        .collect();

    {
        let doomed = node(&master, 41);
        for (path, bytes) in &corpus {
            doomed.store.write_file(path, bytes).expect("write");
        }
        doomed
            .store
            .write_file("documents/report-00.bin", b"second thoughts")
            .expect("edit");
        doomed
            .store
            .remove_file("documents/report-11.bin")
            .expect("delete");
        doomed.store.flush_segment().expect("flush");

        with_server(&host, Pledge::gigabytes(1), |address| {
            let mut client =
                PeerClient::connect(address, &doomed.device, doomed.store.owner(), None)
                    .expect("dial");
            let report = session::push(&doomed.store, &mut client).expect("push");
            assert!(report.chunks_accepted > 0, "the host took nothing");
        });
        // And now the machine is gone: its store, its blobs, its device key.
    }

    // A new machine. Same master secret, because that is what `itsanas login`
    // reconstructs from the passphrase; different device seed, because a
    // recovered install is a *new device* and must not pretend to be the one
    // that died.
    let replacement = node(&master, 42);
    assert!(
        replacement.store.list().expect("list").is_empty(),
        "the replacement was not actually empty"
    );

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client = PeerClient::connect(
            address,
            &replacement.device,
            replacement.store.owner(),
            None,
        )
        .expect("dial");
        let report =
            session::round(&replacement.store, &replacement.vault, &mut client).expect("round");
        assert!(
            report.pull.adopted > 0,
            "the replacement learned nothing from the host: {report:?}"
        );
        assert_eq!(
            report.pull.deferred, 0,
            "content was deferred on an unmetered round, so the restore is \
             incomplete and nothing said so"
        );
    });

    for (path, bytes) in &corpus {
        if path == "documents/report-11.bin" {
            assert_eq!(
                replacement.store.read_file(path).expect("read"),
                None,
                "a deleted file came back from the dead. A restore that \
                 resurrects everything you ever deleted looks exactly like a \
                 restore that worked."
            );
            continue;
        }
        let expected: &[u8] = if path == "documents/report-00.bin" {
            b"second thoughts"
        } else {
            bytes
        };
        assert_eq!(
            replacement.store.read_file(path).expect("read").as_deref(),
            Some(expected),
            "{path} did not come back intact"
        );
    }

    // The replacement now also knows *where* its data lives, without having
    // uploaded a byte. That is what makes the next round cheap instead of a
    // full re-upload of everything it just downloaded.
    assert!(
        replacement.store.stats().expect("stats").holder_records > 0,
        concat!(
            "a restored device does not know that the host holds its data, ",
            "so its next round will re-upload everything it just downloaded ",
            "from it"
        )
    );
}

// ---------------------------------------------------------------------------
// What a peer cannot do
// ---------------------------------------------------------------------------

#[test]
fn a_storage_challenge_works_over_the_wire() {
    let host = node(&MasterSecret::from_bytes([0xB2; 32]), 12);
    let owner = node(&alice(), 13);

    owner
        .store
        .write_file("audited.txt", b"prove you are holding this")
        .unwrap();
    owner.store.flush_segment().unwrap();
    let entry = owner.store.stat("audited.txt").unwrap().unwrap();
    let address = entry.chunks[0];

    // The owner can re-derive the sealed bytes without keeping a copy, because
    // chunk sealing is deterministic. That is what makes remote audit possible.
    let expected = owner.store.blobs().get(&address).unwrap().unwrap();

    with_server(&host, Pledge::gigabytes(1), |server_address| {
        let mut client =
            PeerClient::connect(server_address, &owner.device, owner.store.owner(), None).unwrap();

        // Before storing anything, the host cannot pass.
        assert!(
            !client
                .challenge(owner.store.owner(), address, [1; 32], &expected)
                .unwrap(),
            "a host passed a challenge for a chunk it has never seen"
        );

        session::push(&owner.store, &mut client).unwrap();

        assert!(
            client
                .challenge(owner.store.owner(), address, [1; 32], &expected)
                .unwrap(),
            "a host holding the chunk failed its challenge"
        );
        // A different nonce must also work, and must be a different proof.
        assert!(
            client
                .challenge(owner.store.owner(), address, [2; 32], &expected)
                .unwrap()
        );
    });
}

#[test]
fn a_peer_cannot_push_a_forged_segment_into_a_host() {
    let host = node(&MasterSecret::from_bytes([0xB3; 32]), 14);
    let owner = node(&alice(), 15);

    owner.store.write_file("real.txt", b"genuine").unwrap();
    let mut envelope = owner.store.flush_segment().unwrap().unwrap();
    envelope.sealed_body[0] ^= 0xFF;

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).unwrap();

        assert!(
            !client.store_segment(&envelope).unwrap(),
            "a host accepted a segment whose signature does not verify"
        );
    });

    assert_eq!(host.vault.stats().unwrap().segments, 0);
}

#[test]
fn a_host_that_has_pledged_nothing_refuses_to_store_but_still_answers() {
    let host = node(&MasterSecret::from_bytes([0xB4; 32]), 16);
    let owner = node(&alice(), 17);

    owner.store.write_file("unwanted.txt", b"content").unwrap();
    owner.store.flush_segment().unwrap();

    with_server(&host, Pledge::NONE, |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).unwrap();

        let report = session::push(&owner.store, &mut client).unwrap();
        assert_eq!(
            report.chunks_accepted, 0,
            "a node that pledged nothing accepted data anyway"
        );
        assert_eq!(report.segments_accepted, 0);

        // But it is still a working peer.
        assert!(
            client.heads(owner.store.owner()).is_ok(),
            "refusing to store made the node stop answering entirely"
        );
    });

    assert_eq!(host.vault.stats().unwrap().bytes, 0);
}

#[test]
fn a_malformed_request_gets_a_refusal_rather_than_a_dropped_connection() {
    // A peer must not be able to kill a connection, and thereby a sync round,
    // by sending something silly.
    let host = node(&alice(), 18);
    let peer = node(&alice(), 19);

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &peer.device, peer.store.owner(), None).unwrap();

        let response = client
            .request(&Request::Segments {
                owner: peer.store.owner(),
                device: host.store.device_id(),
                after: None,
                limit: 0,
            })
            .expect("the connection survived a malformed request");
        assert!(matches!(response, Response::Refused(_)));

        // The connection still works afterwards.
        assert!(client.heads(peer.store.owner()).is_ok());
    });
}

#[test]
fn a_peer_asking_about_an_unknown_user_gets_an_empty_answer() {
    let host = node(&alice(), 20);
    let stranger = node(&MasterSecret::from_bytes([0xB5; 32]), 21);

    with_server(&host, Pledge::gigabytes(1), |address| {
        let mut client =
            PeerClient::connect(address, &stranger.device, stranger.store.owner(), None).unwrap();

        assert_eq!(
            client.heads(stranger.store.owner()).unwrap(),
            Vec::new(),
            "a host invented chains for a user it has never heard of"
        );
    });
}

// ---------------------------------------------------------------------------
// Convergence over the wire
// ---------------------------------------------------------------------------

#[test]
fn concurrent_edits_on_two_machines_converge_over_a_socket() {
    // The same property the simulation proves, but through the real protocol,
    // so a bug in the transport that loses or reorders work would show up here.
    let laptop = node(&alice(), 22);
    let pi = node(&alice(), 23);

    // A shared starting point.
    laptop.store.write_file("doc.txt", b"base").unwrap();
    laptop.store.flush_segment().unwrap();

    with_server(&laptop, Pledge::gigabytes(1), |address| {
        let mut client = PeerClient::connect(address, &pi.device, pi.store.owner(), None).unwrap();
        session::round(&pi.store, &pi.vault, &mut client).unwrap();
    });
    assert_eq!(pi.store.read_file("doc.txt").unwrap().unwrap(), b"base");

    // Now both edit while apart.
    laptop.store.write_file("doc.txt", b"laptop edit").unwrap();
    laptop.store.flush_segment().unwrap();
    pi.store.write_file("doc.txt", b"pi edit").unwrap();
    pi.store.flush_segment().unwrap();

    // Two rounds: the first carries each side's work across, the second lets
    // the laptop see what the Pi pushed.
    with_server(&laptop, Pledge::gigabytes(1), |address| {
        let mut client = PeerClient::connect(address, &pi.device, pi.store.owner(), None).unwrap();
        session::round(&pi.store, &pi.vault, &mut client).unwrap();
        session::round(&pi.store, &pi.vault, &mut client).unwrap();
    });

    let pi_files = pi.store.list().unwrap();
    assert_eq!(
        pi_files.len(),
        2,
        "the Pi should hold both versions after a concurrent edit, got {pi_files:?}"
    );

    let mut bodies: Vec<Vec<u8>> = pi_files
        .iter()
        .map(|path| pi.store.read_file(path).unwrap().unwrap())
        .collect();
    bodies.sort();
    assert_eq!(
        bodies,
        vec![b"laptop edit".to_vec(), b"pi edit".to_vec()],
        "a concurrent edit lost one side's work across the wire"
    );
}

// ---------------------------------------------------------------------------
// Where the data went
// ---------------------------------------------------------------------------

#[test]
fn a_sync_round_records_which_peer_now_holds_this_nodes_data() {
    // The replacement for a coordinator-published node set. An owner already
    // keeps a log of their own chunks, so they can simply write down where they
    // put them — and then nothing has to agree with anybody about membership.
    //
    // Without this the repair loop has no idea whether a chunk exists anywhere
    // but on this disk, and the honest answer to "is my data safe?" is "no
    // idea".
    let master = alice();
    let laptop = node(&master, 1);
    let pi = node(&master, 2);

    let chunks = laptop
        .store
        .write_file("notes/report.txt", b"a real file with real chunks")
        .expect("write")
        .chunks;
    laptop.store.flush_segment().expect("flush");
    assert!(!chunks.is_empty());

    for chunk in &chunks {
        assert!(
            laptop
                .store
                .remote_holders(chunk)
                .expect("holders")
                .is_empty(),
            "nothing should be recorded before anything has been sent"
        );
    }

    with_server(&pi, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &laptop.device, laptop.store.owner(), None).expect("dial");
        let report = session::push(&laptop.store, &mut client).expect("push");
        assert!(report.chunks_accepted > 0, "the push sent nothing");
        assert_eq!(
            report.holders_recorded,
            laptop.store.blobs().addresses().expect("addresses").len(),
            "every offered chunk should be accounted for"
        );
    });

    for chunk in &chunks {
        let holders = laptop.store.remote_holders(chunk).expect("holders");
        assert_eq!(holders.len(), 1, "chunk {chunk:?} was not recorded");
        assert_eq!(
            holders[0].device,
            pi.store.device_id(),
            "the record names the wrong machine"
        );
    }

    assert!(
        laptop.store.under_replicated(2).expect("risk").is_empty(),
        "one remote holder plus this device meets a target of two"
    );
    assert!(
        !laptop.store.under_replicated(3).expect("risk").is_empty(),
        "a target of three is not met by one remote holder"
    );
}

#[test]
fn a_peer_that_already_had_the_data_is_still_recorded_as_holding_it() {
    // The property that makes the ledger converge rather than only grow. A
    // second round sends nothing, because the peer already has everything — and
    // the ledger must still come out of that round knowing the peer holds it.
    //
    // This is what a device restored from its recovery phrase depends on: it
    // learns where its data lives by asking, instead of re-uploading its entire
    // store to find out. The answer costs nothing extra, since it is the same
    // round trip that decides what to send.
    let master = alice();
    let laptop = node(&master, 1);
    let pi = node(&master, 2);

    let chunks = laptop
        .store
        .write_file("notes/report.txt", b"a real file with real chunks")
        .expect("write")
        .chunks;
    laptop.store.flush_segment().expect("flush");

    with_server(&pi, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &laptop.device, laptop.store.owner(), None).expect("dial");
        session::push(&laptop.store, &mut client).expect("first push");
    });

    // Forget everything, as a freshly restored device would have.
    let dropped = laptop
        .store
        .forget_device(&pi.store.device_id())
        .expect("forget");
    assert!(dropped > 0);

    with_server(&pi, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &laptop.device, laptop.store.owner(), None).expect("dial");
        let report = session::push(&laptop.store, &mut client).expect("second push");

        assert_eq!(
            report.chunks_accepted, 0,
            "the peer already had everything, so nothing should have been sent"
        );
        assert_eq!(
            report.holders_recorded, dropped,
            "the ledger should have been rebuilt from what the peer said it had"
        );
    });

    for chunk in &chunks {
        assert_eq!(
            laptop.store.remote_holders(chunk).expect("holders").len(),
            1,
            "the ledger did not recover without re-uploading"
        );
    }
}

#[test]
fn a_host_that_refuses_to_store_is_not_recorded_as_holding_anything() {
    // A node that has pledged nothing still answers, because refusing to host
    // does not stop it being a peer. Recording it as a holder anyway would let
    // a node believe its data was replicated onto a machine that declined it —
    // the worst possible error, since it is indistinguishable from safety until
    // the day the local disk dies.
    let master = alice();
    let laptop = node(&master, 1);
    let stingy = node(&master, 2);

    let chunks = laptop
        .store
        .write_file("notes/report.txt", b"a real file with real chunks")
        .expect("write")
        .chunks;
    laptop.store.flush_segment().expect("flush");

    with_server(&stingy, Pledge { bytes: 0 }, |address| {
        let mut client =
            PeerClient::connect(address, &laptop.device, laptop.store.owner(), None).expect("dial");
        let report = session::push(&laptop.store, &mut client).expect("push");
        assert_eq!(
            report.chunks_accepted, 0,
            "a node pledging nothing accepted data"
        );
        assert_eq!(report.holders_recorded, 0);
    });

    for chunk in &chunks {
        assert!(
            laptop
                .store
                .remote_holders(chunk)
                .expect("holders")
                .is_empty(),
            "a refusal was recorded as safe storage"
        );
    }
    assert!(
        laptop
            .store
            .under_replicated(2)
            .expect("risk")
            .iter()
            .all(AtRisk::only_copy),
        "every chunk should still be reported as existing only here"
    );
}

// ---------------------------------------------------------------------------
// Metadata-only rounds, for a phone on mobile data
// ---------------------------------------------------------------------------

#[test]
fn a_metadata_round_learns_what_changed_without_downloading_it() {
    // What a phone on mobile data needs: know that work is waiting, keep the
    // verified segments so the next round on wifi resumes instead of starting
    // over, and download nothing.
    //
    // Nothing may be half-written. An operation is either applied with its
    // content or deferred, which is the same guarantee a sleeping peer gets.
    //
    // Note what is *not* asserted: that the file appears in a listing. It does
    // not, because a deferred operation writes no index entry, and pretending
    // otherwise here is how that gap would stay hidden.
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);

    laptop
        .store
        .write_file("notes/report.txt", b"a real file with real chunks in it")
        .expect("write");
    laptop.store.flush_segment().expect("flush");

    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        let report = session::pull_scoped(
            &phone.store,
            &phone.vault,
            &mut client,
            session::Scope::Metadata,
        )
        .expect("metadata pull");

        assert_eq!(
            report.adopted, 0,
            "a metadata round materialised a file it never downloaded"
        );
        assert!(
            report.deferred > 0,
            "the operation was neither applied nor deferred, so it was lost"
        );
    });

    assert!(
        phone
            .store
            .read_file("notes/report.txt")
            .expect("read")
            .is_none(),
        "the file is readable after a round that never fetched its bytes"
    );

    // The segments are kept, so this node can relay them onwards and the next
    // round resumes rather than starting over.
    assert!(
        !phone
            .vault
            .heads_for(phone.store.owner())
            .expect("heads")
            .is_empty(),
        "the log segments were discarded, so the cheap half of the work was wasted"
    );

    // Back on wifi.
    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        session::pull_scoped(
            &phone.store,
            &phone.vault,
            &mut client,
            session::Scope::Everything,
        )
        .expect("full pull");
    });

    assert_eq!(
        phone
            .store
            .read_file("notes/report.txt")
            .expect("read")
            .as_deref(),
        Some(&b"a real file with real chunks in it"[..]),
        "the deferred operation never completed once the content was reachable"
    );
}

#[test]
fn a_metadata_round_offers_the_log_but_sends_no_chunks() {
    // The other direction, and the one that costs a phone money: a photo taken
    // on mobile data must not upload itself. The peer still learns that it
    // happened, so nothing is lost and the upload resumes on wifi.
    let master = alice();
    let phone = node(&master, 1);
    let host = node(&master, 2);

    phone
        .store
        .write_file(
            "photos/img.jpg",
            b"pretend this is four megabytes of photograph",
        )
        .expect("write");
    phone.store.flush_segment().expect("flush");

    with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        let report = session::push_scoped(&phone.store, &mut client, session::Scope::Metadata)
            .expect("metadata push");

        assert!(
            report.segments_accepted > 0,
            "the peer was not even told that anything had happened"
        );
        assert_eq!(
            report.chunks_offered, 0,
            "a metadata push offered chunks, which is the upload it exists to avoid"
        );
        assert_eq!(report.chunks_accepted, 0);
        assert_eq!(
            report.holders_recorded, 0,
            "chunks were recorded as stored somewhere they were never sent"
        );
    });
}

#[test]
fn a_metadata_round_makes_the_file_listable_before_it_is_downloaded() {
    // The behaviour everyone expects from a phone client: everything is
    // listed, tapping one downloads it. Before the catalogue existed, a
    // metadata round left the file invisible — deferred means no index entry,
    // and `list` reports the index. A client could show only what it had
    // already downloaded, which on a metered connection is nothing.
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);

    laptop
        .store
        .write_file("photos/holiday.jpg", b"pretend this is a large photograph")
        .expect("write");
    laptop
        .store
        .write_file("notes/todo.txt", b"milk")
        .expect("write");
    laptop.store.flush_segment().expect("flush");

    assert!(
        itsanas_store::catalogue(&phone.store, &phone.vault)
            .expect("catalogue")
            .files
            .is_empty(),
        "a phone that has synced nothing should know of nothing"
    );

    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        session::pull_scoped(
            &phone.store,
            &phone.vault,
            &mut client,
            session::Scope::Metadata,
        )
        .expect("metadata pull");
    });

    // The index still holds nothing — no half-written state.
    assert!(phone.store.list().expect("list").is_empty());

    let known = itsanas_store::catalogue(&phone.store, &phone.vault)
        .expect("catalogue")
        .files;
    let paths: Vec<&str> = known.iter().map(|k| k.path.as_str()).collect();
    assert_eq!(paths, vec!["notes/todo.txt", "photos/holiday.jpg"]);
    assert!(
        known
            .iter()
            .all(|k| k.presence == itsanas_store::Presence::Absent),
        "nothing was downloaded, so nothing should claim to be here"
    );
    assert_eq!(
        known
            .iter()
            .find(|k| k.path == "notes/todo.txt")
            .expect("listed")
            .size,
        4,
        "the size comes from the log, so it is known before the content is"
    );

    // On wifi.
    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        session::pull_scoped(
            &phone.store,
            &phone.vault,
            &mut client,
            session::Scope::Everything,
        )
        .expect("content pull");
    });

    let known = itsanas_store::catalogue(&phone.store, &phone.vault)
        .expect("catalogue")
        .files;
    assert_eq!(known.len(), 2, "the same two files, not four");
    assert!(
        known
            .iter()
            .all(|k| k.presence == itsanas_store::Presence::Local),
        "everything was downloaded, so nothing should still be marked absent"
    );
    assert_eq!(
        itsanas_store::absent_count(&phone.store, &phone.vault).expect("count"),
        0
    );
}

#[test]
fn a_file_deleted_elsewhere_is_never_offered_for_download() {
    // A phone that lists a file deleted a week ago, and downloads it when
    // tapped, has resurrected it. The catalogue reads the log's last word,
    // which is the delete.
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);

    laptop
        .store
        .write_file("gone.txt", b"temporary")
        .expect("write");
    laptop.store.flush_segment().expect("flush");
    laptop.store.remove_file("gone.txt").expect("remove");
    laptop.store.flush_segment().expect("flush");

    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        session::pull_scoped(
            &phone.store,
            &phone.vault,
            &mut client,
            session::Scope::Metadata,
        )
        .expect("metadata pull");
    });

    assert!(
        itsanas_store::catalogue(&phone.store, &phone.vault)
            .expect("catalogue")
            .files
            .is_empty(),
        "a deleted file was offered for download"
    );
}

#[test]
fn a_delete_racing_an_edit_still_leaves_the_file_listed() {
    // The asymmetry the whole merge design rests on: a delete concurrent with
    // an edit loses, because an unexpected file costs a second and a lost edit
    // is unrecoverable. A listing that applied the opposite rule would hide a
    // file the merge engine is about to keep — and the person looking at the
    // phone would conclude their edit was lost.
    let master = alice();
    let laptop = node(&master, 1);
    let pi = node(&master, 2);
    let phone = node(&master, 3);

    laptop.store.write_file("doc.txt", b"base").expect("write");
    laptop.store.flush_segment().expect("flush");

    // The Pi learns about the file, then the two lose sight of each other.
    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &pi.device, pi.store.owner(), None).expect("dial");
        session::round(&pi.store, &pi.vault, &mut client).expect("round");
    });
    assert!(pi.store.read_file("doc.txt").expect("read").is_some());

    // Apart: one edits, the other deletes.
    laptop.store.write_file("doc.txt", b"edited").expect("edit");
    laptop.store.flush_segment().expect("flush");
    pi.store.remove_file("doc.txt").expect("delete");
    pi.store.flush_segment().expect("flush");

    // The phone hears both sides and downloads nothing.
    for host in [&laptop, &pi] {
        with_server(host, Pledge { bytes: 1 << 30 }, |address| {
            let mut client = PeerClient::connect(address, &phone.device, phone.store.owner(), None)
                .expect("dial");
            session::pull_scoped(
                &phone.store,
                &phone.vault,
                &mut client,
                session::Scope::Metadata,
            )
            .expect("metadata pull");
        });
    }

    let known = itsanas_store::catalogue(&phone.store, &phone.vault)
        .expect("catalogue")
        .files;
    assert_eq!(
        known.len(),
        1,
        "the edit lost to a concurrent delete in the listing, got {known:?}"
    );
    assert_eq!(known[0].path, "doc.txt");
    assert_eq!(known[0].presence, itsanas_store::Presence::Absent);
}

#[test]
fn a_round_that_deferred_nothing_does_not_replay_the_chain_next_time() {
    // The replay exists so deferred work is retried. Doing it unconditionally
    // turned the daemon's per-round cost from "the new segments" into "the
    // whole chain, times the number of peers" — a regression introduced with
    // the fix and measured afterwards.
    //
    // The marker is what makes it conditional, and it is only moved by a round
    // that finished everything. If this test fails in the "outstanding"
    // direction the cost regression is back; if it fails in the other, work
    // that could not finish is never looked at again.
    let master = alice();
    let laptop = node(&master, 1);
    let phone = node(&master, 2);

    laptop
        .store
        .write_file("doc.txt", b"content")
        .expect("write");
    laptop.store.flush_segment().expect("flush");

    assert!(
        !phone.store.has_unapplied(&phone.vault).expect("check"),
        "an empty vault cannot have outstanding work"
    );

    // A metadata round takes the segments and finishes nothing.
    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        session::pull_scoped(
            &phone.store,
            &phone.vault,
            &mut client,
            session::Scope::Metadata,
        )
        .expect("metadata pull");
    });

    assert!(
        phone.store.has_unapplied(&phone.vault).expect("check"),
        "segments were taken and never applied, which is the definition of outstanding"
    );

    // A content round finishes it.
    with_server(&laptop, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &phone.device, phone.store.owner(), None).expect("dial");
        let report = session::pull_scoped(
            &phone.store,
            &phone.vault,
            &mut client,
            session::Scope::Everything,
        )
        .expect("content pull");
        assert_eq!(report.deferred, 0);
    });

    assert!(
        !phone.store.has_unapplied(&phone.vault).expect("check"),
        "everything applied, so nothing is outstanding and the next round should not replay"
    );
    assert!(phone.store.read_file("doc.txt").expect("read").is_some());
}

// ---------------------------------------------------------------------------
// Auditing: making a host prove it still has what it said it had
// ---------------------------------------------------------------------------

#[test]
fn an_audit_confirms_a_host_that_is_still_holding_the_data() {
    // The ledger records that a peer *accepted* a chunk. That is evidence, not
    // proof — a host that accepted and then deleted looks identical from here.
    // An audit is what turns one into the other, for the moment it is asked.
    let master = alice();
    let owner = node(&master, 1);
    let host = node(&master, 2);

    owner
        .store
        .write_file("audited.txt", b"prove you are still holding this")
        .expect("write");
    owner.store.flush_segment().expect("flush");

    with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        session::push(&owner.store, &mut client).expect("push");

        let report = session::audit(&owner.store, &mut client, 16).expect("audit");
        assert!(report.asked > 0, "the audit checked nothing at all");
        assert_eq!(report.confirmed, report.asked);
        assert_eq!(report.failed, 0);
        assert!(!report.found_a_liar());
    });
}

#[test]
fn red_team_a_host_that_threw_the_data_away_stops_counting_as_a_holder() {
    // THE ATTACK, and it costs nothing: accept everything offered, delete it
    // immediately, and keep claiming the space. A node that trusted its own
    // ledger would believe its files were on three machines while two of them
    // held nothing, and would find out on the day the third disk died.
    //
    // Passing an audit does not prove a host will still have the bytes
    // tomorrow — that limit is documented — but silently discarding them has to
    // stop being free.
    //
    // If this test fails, the placement ledger is a list of promises with
    // nothing checking any of them.
    let master = alice();
    let owner = node(&master, 1);
    let host = node(&master, 2);

    let chunks = owner
        .store
        .write_file("audited.txt", b"prove you are still holding this")
        .expect("write")
        .chunks;
    owner.store.flush_segment().expect("flush");

    with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        session::push(&owner.store, &mut client).expect("push");
    });

    for chunk in &chunks {
        assert_eq!(
            owner.store.remote_holders(chunk).expect("holders").len(),
            1,
            "the push was not recorded, so this test would prove nothing"
        );
    }

    // The host quietly deletes what it accepted.
    for chunk in &chunks {
        assert!(
            host.vault
                .remove_chunk(owner.store.owner(), chunk)
                .expect("discard"),
            "the host did not actually hold what it was asked to discard"
        );
    }

    with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        let report = session::audit(&owner.store, &mut client, 16).expect("audit");

        assert!(
            report.found_a_liar(),
            "a host that discarded everything passed its audit: {report:?}"
        );
        assert_eq!(report.confirmed, 0);
    });

    for chunk in &chunks {
        assert!(
            owner
                .store
                .remote_holders(chunk)
                .expect("holders")
                .is_empty(),
            "a host that failed its audit is still recorded as a holder"
        );
    }

    // And the consequence the owner actually cares about: the data now shows
    // as existing nowhere else, which is what repair acts on.
    assert!(
        owner
            .store
            .under_replicated(2)
            .expect("risk")
            .iter()
            .all(itsanas_store::AtRisk::only_copy),
        "the withdrawal did not make the chunk show as under-replicated"
    );
}

#[test]
fn an_audit_never_asks_about_a_chunk_it_could_not_check() {
    // Verifying a proof means re-deriving the sealed bytes locally. A chunk
    // this device has collected cannot be re-derived, so challenging on it
    // would fail for a reason that is nothing to do with the peer — and would
    // withdraw a perfectly good record.
    let master = alice();
    let owner = node(&master, 1);
    let host = node(&master, 2);

    let chunks = owner
        .store
        .write_file("audited.txt", b"content that will be collected locally")
        .expect("write")
        .chunks;
    owner.store.flush_segment().expect("flush");

    with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        session::push(&owner.store, &mut client).expect("push");
    });

    // The owner loses its own copy while the host keeps its.
    for chunk in &chunks {
        owner.store.blobs().remove(chunk).expect("drop local copy");
    }

    with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        let report = session::audit(&owner.store, &mut client, 16).expect("audit");

        assert_eq!(
            report.asked, 0,
            "it challenged on something it cannot verify"
        );
        assert!(report.unverifiable > 0, "the skip was not reported");
        assert!(!report.found_a_liar());
    });

    for chunk in &chunks {
        assert_eq!(
            owner.store.remote_holders(chunk).expect("holders").len(),
            1,
            "an honest host lost its record because the owner could not check"
        );
    }
}

#[test]
fn red_team_a_host_that_keeps_discarding_stops_getting_free_uploads() {
    // THE ATTACK, and it is the one auditing alone does not stop. Accept
    // everything, delete it, and wait. The audit catches it every round, the
    // owner re-uploads every round, and the host pays nothing. The more data
    // the owner has, the more it costs them — a free, indefinite drain on their
    // uplink in exchange for agreeing to store data and then not.
    //
    // Detection without memory is not a defence. If this test fails, anyone can
    // exhaust an owner's bandwidth by volunteering to help.
    //
    // What the defence is *not* is a cut-off: one probe chunk still goes each
    // round, because a peer with nothing recorded has nothing to be challenged
    // on and could never earn its way back. So this measures **volume**, which
    // is what the attack costs, rather than whether anything moved at all.
    let master = alice();
    let owner = node(&master, 1);
    let host = node(&master, 2);

    // Large enough that "everything" and "one chunk" are not close.
    let mut payload = vec![0u8; 2_000_000];
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"bandwidth drain corpus");
    hasher.finalize_xof().fill(&mut payload);

    let chunks = owner
        .store
        .write_file("bait.bin", &payload)
        .expect("write")
        .chunks;
    owner.store.flush_segment().expect("flush");
    assert!(
        chunks.len() > 8,
        "the corpus must be many chunks to be meaningful"
    );

    let mut bytes_per_round: Vec<u64> = Vec::new();

    for _ in 0..(itsanas_store::FAILURES_BEFORE_PAUSE + 3) {
        with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
            let mut client = PeerClient::connect(address, &owner.device, owner.store.owner(), None)
                .expect("dial");

            // The audit runs first, exactly as the daemon orders it.
            let _ = session::audit(&owner.store, &mut client, 64).expect("audit");
            let report = session::push(&owner.store, &mut client).expect("push");
            bytes_per_round.push(report.bytes_sent);
        });

        // The host discards whatever it just took.
        for chunk in &chunks {
            let _ = host.vault.remove_chunk(owner.store.owner(), chunk);
        }
    }

    let first = bytes_per_round[0];
    let last = *bytes_per_round.last().expect("rounds");
    assert!(
        first > 1_000_000,
        "the first round did not upload the corpus"
    );
    assert!(
        last * 10 < first,
        "the {}th round still cost {last} bytes against the first round's          {first} — the drain was never cut off",
        bytes_per_round.len()
    );

    let record = owner
        .store
        .reliability(&host.store.device_id())
        .expect("record");
    assert!(!record.worth_sending_to());
    assert!(record.failed > 0);
}

/// Questions per audit round in these tests, matching
/// `session::CHALLENGES_PER_ROUND`.
const CHALLENGES: usize = 16;

#[test]
fn red_team_a_host_that_keeps_only_what_it_expects_to_be_asked_is_caught() {
    // THE ATTACK: accept the whole store, work out which chunks the audit will
    // ask about, keep exactly those, delete the rest. If the questions are
    // predictable the host holds a spotless record while storing a rounding
    // error — and for six commits they were not merely predictable, they were
    // a constant.
    //
    // The audit worked through the least recently confirmed records first,
    // which reads like diligence. But a push round re-stamps every record the
    // peer *claims* to hold, a whole batch from one clock reading, so within a
    // batch every timestamp was equal and the sort fell through to its
    // tie-break: the chunk id. The sixteen lowest ids, every round, for ever.
    // At a terabyte that is sixteen chunks out of fourteen million.
    //
    // So this attacker keeps precisely the sixteen lowest ids and nothing else.
    // Under the old rule it survives for ever. If this test fails, a host can
    // pledge a terabyte, store a megabyte, and never be caught by anything in
    // this system.
    let master = alice();
    let owner = node(&master, 1);
    let host = node(&master, 2);

    let chunks = owner
        .store
        .write_file("hostage.bin", &a_file_of_many_chunks(11, 8 << 20))
        .expect("write")
        .chunks;
    owner.store.flush_segment().expect("flush");
    assert!(
        chunks.len() > CHALLENGES * 2,
        "the fixture is {} chunks; with fewer than twice the questions per \
         round, keeping the answers is not an attack, it is just storing the \
         data",
        chunks.len()
    );

    // Exactly what the old selection rule would have asked, every round.
    let mut by_id: Vec<_> = chunks.clone();
    by_id.sort_unstable();
    let kept: std::collections::BTreeSet<_> = by_id.iter().take(CHALLENGES).copied().collect();

    let discard_everything_else = || {
        for chunk in &chunks {
            if !kept.contains(chunk) {
                let _ = host.vault.remove_chunk(owner.store.owner(), chunk);
            }
        }
    };

    with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
        let mut client =
            PeerClient::connect(address, &owner.device, owner.store.owner(), None).expect("dial");
        session::push(&owner.store, &mut client).expect("push");
    });
    discard_everything_else();

    // Six rounds. Re-deleting after each one so the owner's own pushes cannot
    // accidentally repair what the attacker threw away.
    let mut caught_in = None;
    for round in 1..=6u32 {
        with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
            let mut client = PeerClient::connect(address, &owner.device, owner.store.owner(), None)
                .expect("dial");
            let report = session::audit(&owner.store, &mut client, CHALLENGES).expect("audit");
            if report.failed > 0 && caught_in.is_none() {
                caught_in = Some(round);
            }
        });
        discard_everything_else();
    }

    let held = kept.len();
    assert!(
        caught_in.is_some(),
        "a host holding {held} of {} chunks answered six rounds of {CHALLENGES} \
         questions without one failure. The questions are predictable, so the \
         audit proves nothing at all.",
        chunks.len()
    );
}

/// A file large enough to be split into many chunks.
///
/// Single-chunk fixtures make the audit tests pass for the wrong reason: with
/// one record on the ledger, every way of choosing what to challenge picks the
/// same thing, so a selection rule that is broken for a real store looks
/// correct. Twice this happened in this file before anyone noticed.
fn a_file_of_many_chunks(seed: u8, bytes: usize) -> Vec<u8> {
    // Not compressible and not repetitive: a repeating pattern would let the
    // content-defined chunker cut it into a handful of identical pieces, which
    // would collapse back into the single-record case this exists to avoid.
    let mut out = Vec::with_capacity(bytes);
    let mut state = u64::from(seed).wrapping_add(0x9E37_79B9_7F4A_7C15);
    while out.len() < bytes {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.to_le_bytes());
    }
    out
}

#[test]
fn a_paused_host_that_starts_answering_again_is_sent_data_again() {
    // The way back, and it has to actually exist — on a real store, not on a
    // fixture of one chunk.
    //
    // A paused peer is offered one chunk a round so it has something it can
    // prove. The first version left the audit to *find* that chunk in the
    // ledger, where it sat as one fresh record among however many thousand
    // stale ones the peer is paused for. Every question landed on a record the
    // peer had already lost, every round failed, and the sanction never lifted:
    // a ban wearing the words of a suspension. The test that was supposed to
    // catch that used a thirty-seven byte file — one chunk, one record, the
    // one case where finding the probe is guaranteed — and so it passed while
    // the mechanism it named did not work.
    let master = alice();
    let owner = node(&master, 1);
    let host = node(&master, 2);

    let chunks = owner
        .store
        .write_file("bait.bin", &a_file_of_many_chunks(3, 2 << 20))
        .expect("write")
        .chunks;
    assert!(
        chunks.len() > 8,
        "the fixture is {} chunks, which is small enough for the degenerate \
         case to hide a broken selection rule again",
        chunks.len()
    );
    owner.store.flush_segment().expect("flush");

    let round = |discard: bool| {
        with_server(&host, Pledge { bytes: 1 << 30 }, |address| {
            let mut client = PeerClient::connect(address, &owner.device, owner.store.owner(), None)
                .expect("dial");
            let _ = session::audit(&owner.store, &mut client, 16).expect("audit");
            let _ = session::push(&owner.store, &mut client).expect("push");
        });
        if discard {
            for chunk in &chunks {
                let _ = host.vault.remove_chunk(owner.store.owner(), chunk);
            }
        }
    };

    for _ in 0..=itsanas_store::FAILURES_BEFORE_PAUSE {
        round(true);
    }

    assert!(
        !owner
            .store
            .worth_sending_to(&host.store.device_id())
            .expect("record"),
        "the host was never paused, so this test would prove nothing"
    );

    // The host is repaired and keeps what it is given from now on. Each round
    // it is handed one chunk the *owner* picked, and the next round's audit
    // asks about that chunk and nothing else. Each answered round pays off one
    // failure.
    //
    // Both ends of that are the point. Recovery that never arrives was the
    // first version of this mechanism. Recovery in a single round was the
    // second, and it made the sanction free: keep one chunk for one round, get
    // the whole store back, discard it, repeat for ever. So this measures how
    // long it actually takes and asserts it is neither.
    let mut rounds = 0u32;
    while !owner
        .store
        .worth_sending_to(&host.store.device_id())
        .expect("record")
    {
        rounds += 1;
        assert!(
            rounds <= itsanas_store::PROBATION_CEILING + 8,
            concat!(
                "a host answering every question for {} rounds is still paused. ",
                "There is no way back, only a ban with a friendlier message"
            ),
            rounds
        );
        round(false);
    }

    assert!(
        rounds >= itsanas_store::FAILURES_BEFORE_PAUSE,
        concat!(
            "the pause lifted after {} answered rounds, having taken {} failures ",
            "to start. A sanction a host escapes faster than it earned costs it ",
            "one chunk for one round and nothing else"
        ),
        rounds,
        itsanas_store::FAILURES_BEFORE_PAUSE
    );

    // And once cleared, the rest of the data flows again.
    round(false);
    for chunk in &chunks {
        assert!(
            !owner
                .store
                .remote_holders(chunk)
                .expect("holders")
                .is_empty(),
            "the recovered host was never sent the rest of the data"
        );
    }

    // Throughout, the log kept flowing, which is what keeps a paused peer able
    // to relay for devices that have done nothing wrong.
    assert!(
        !host
            .vault
            .heads_for(owner.store.owner())
            .expect("heads")
            .is_empty()
    );
}
