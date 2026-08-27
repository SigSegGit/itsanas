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
    Exposure, PeerClient, PeerServer, PeerService, Pledge,
    protocol::{Request, Response},
    session,
};
use itsanas_store::{ChunkerConfig, Store, Vault};

/// One machine: its own store, and a vault for other people's data.
struct Node {
    _dir: tempfile::TempDir,
    store: Store,
    vault: Vault,
}

fn node(master: &MasterSecret, device_seed: u8) -> Node {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Store::open_for_testing(
        dir.path().join("store"),
        UserKeys::derive(master),
        DeviceKeys::from_seed(&SecretBytes::new([device_seed; 32])),
        ChunkerConfig::default(),
    )
    .expect("store");
    let vault = Vault::open(dir.path().join("vault")).expect("vault");

    Node {
        _dir: dir,
        store,
        vault,
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
    let server = PeerServer::bind("127.0.0.1:0", Exposure::LocalOnly).expect("bind loopback");
    let address = server.local_addr().expect("local address");
    let shutdown = AtomicBool::new(false);

    let service = PeerService::new(&server_node.store, &server_node.vault, pledge);

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&service, &shutdown);
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
            PeerClient::connect(address, pi.store.device_id(), pi.store.owner()).expect("connect");

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
        let mut client =
            PeerClient::connect(address, pi.store.device_id(), pi.store.owner()).unwrap();
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
        let mut client =
            PeerClient::connect(address, pi.store.device_id(), pi.store.owner()).unwrap();

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
            alice_device.store.device_id(),
            alice_device.store.owner(),
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
            PeerClient::connect(address, pi.store.device_id(), pi.store.owner()).unwrap();
        session::push(&pi.store, &mut pi_client).unwrap();
        drop(pi_client);

        // The VM arrives later and has never met the Pi.
        let mut vm_client =
            PeerClient::connect(address, vm.store.device_id(), vm.store.owner()).unwrap();
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
            PeerClient::connect(server_address, owner.store.device_id(), owner.store.owner())
                .unwrap();

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
            PeerClient::connect(address, owner.store.device_id(), owner.store.owner()).unwrap();

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
            PeerClient::connect(address, owner.store.device_id(), owner.store.owner()).unwrap();

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
            PeerClient::connect(address, peer.store.device_id(), peer.store.owner()).unwrap();

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
            PeerClient::connect(address, stranger.store.device_id(), stranger.store.owner())
                .unwrap();

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
        let mut client =
            PeerClient::connect(address, pi.store.device_id(), pi.store.owner()).unwrap();
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
        let mut client =
            PeerClient::connect(address, pi.store.device_id(), pi.store.owner()).unwrap();
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
