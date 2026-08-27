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

/// Run `body` with `server_node` serving on loopback.
///
/// The server stops as soon as `body` returns, whether it returned or panicked,
/// so a failing assertion does not leave a thread spinning.
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

        let outcome = body(address);
        shutdown.store(true, Ordering::Relaxed);
        // Unblock the accept loop's sleep by connecting once more.
        let _ = std::net::TcpStream::connect(address);
        outcome
    })
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
