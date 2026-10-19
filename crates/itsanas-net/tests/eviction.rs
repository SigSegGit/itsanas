//! `MayEvict` over a real socket: who may say it, and who is never asked.
//!
//! The service's own tests decide the rule from a device handed in; these
//! check that the device is the one TLS proved, and that the client keeps the
//! verb from a peer too old to decode it.

use std::sync::atomic::{AtomicBool, Ordering};

use itsanas_crypto::{ChunkId, DeviceId, DeviceKeys, MasterSecret, SecretBytes, UserId, UserKeys};
use itsanas_net::{
    PeerClient, PeerServer, PeerService, Pledge,
    protocol::{PROTOCOL_WITH_EVICTION, Request, Response},
};
use itsanas_store::{ChunkerConfig, Store, Vault};

struct Node {
    _dir: tempfile::TempDir,
    store: Store,
    vault: Vault,
    device: DeviceKeys,
}

fn node(master: u8, device_seed: u8) -> Node {
    let dir = tempfile::tempdir().expect("temp dir");
    let device = DeviceKeys::from_seed(&SecretBytes::new([device_seed; 32]));
    let store = Store::open_for_testing(
        dir.path().join("store"),
        UserKeys::derive(&MasterSecret::from_bytes([master; 32])),
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

/// One device speaks for one account; anybody else for none.
struct OneSpeaker(DeviceId, UserId);

impl itsanas_net::Owners for OneSpeaker {
    fn present(&self, _: DeviceId, _: &[u8], _: &Vault) -> Result<(), String> {
        Ok(())
    }
    fn admits(
        &self,
        caller: DeviceId,
        owner: UserId,
        _: u64,
        _: &Store,
        _: &Vault,
        _: u64,
    ) -> Result<(), String> {
        if (caller, owner) == (self.0, self.1) {
            Ok(())
        } else {
            Err(itsanas_net::UPDATE_REQUIRED.to_owned())
        }
    }
    fn speaks_for(&self, caller: DeviceId, owner: UserId) -> bool {
        (caller, owner) == (self.0, self.1)
    }
}

struct StopOnDrop<'a>(&'a AtomicBool, std::net::SocketAddr);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.1);
    }
}

fn serving<T>(
    host: &Node,
    service: &PeerService<'_>,
    body: impl FnOnce(std::net::SocketAddr) -> T,
) -> T {
    let server = PeerServer::bind("127.0.0.1:0").expect("bind loopback");
    let address = server.local_addr().expect("local address");
    let shutdown = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(service, &host.device, &shutdown);
        });
        let _stop = StopOnDrop(&shutdown, address);
        body(address)
    })
}

#[test]
fn red_team_a_peer_speaking_protocol_9_is_never_asked_to_evict() {
    // A peer at 9 cannot decode the verb and closes the connection the round
    // still needed. The client must answer `None` without sending, and the
    // owner keeps counting the copy. The control half proves the request
    // does reach a peer at 10. Sabotage: drop the version gate in
    // `PeerClient::may_evict`.
    let host = node(0xA1, 1);
    let bob = node(0xB0, 2);
    let chunk = ChunkId::from_bytes([7; 32]);
    host.vault
        .put_chunk(bob.store.owner(), &chunk, &[7; 10])
        .unwrap();
    let gate = OneSpeaker(bob.device.device_id(), bob.store.owner());

    for (version, expected) in [
        (PROTOCOL_WITH_EVICTION - 1, None),
        (PROTOCOL_WITH_EVICTION, Some(1)),
    ] {
        let service = PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1))
            .with_owners(&gate)
            .speaking_at_most(version);
        serving(&host, &service, |address| {
            let mut client = PeerClient::connect(address, &bob.device, bob.store.owner(), None)
                .expect("connect");
            assert_eq!(
                client
                    .may_evict(bob.store.owner(), vec![chunk])
                    .expect("may evict"),
                expected,
                "a peer speaking protocol {version}"
            );
            // The connection is still good: nothing undecodable was sent.
            client.heads(bob.store.owner()).expect("still connected");
        });
        if expected.is_none() {
            assert!(!host.vault.is_evictable(bob.store.owner(), &chunk).unwrap());
        }
    }
}

#[test]
fn red_team_only_the_device_tls_proved_speaks_for_the_account_on_the_wire() {
    // The account named in the request is a claim; the device is not. A
    // stranger naming Bob is refused however it words the request.
    // Sabotage: credit the request's owner rather than the proved device.
    let host = node(0xA1, 1);
    let bob = node(0xB0, 2);
    let stranger = node(0xC0, 3);
    let chunk = ChunkId::from_bytes([7; 32]);
    host.vault
        .put_chunk(bob.store.owner(), &chunk, &[7; 10])
        .unwrap();
    let gate = OneSpeaker(bob.device.device_id(), bob.store.owner());
    let service =
        PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1)).with_owners(&gate);

    serving(&host, &service, |address| {
        let mut client =
            PeerClient::connect(address, &stranger.device, stranger.store.owner(), None)
                .expect("connect");
        assert!(
            client.may_evict(bob.store.owner(), vec![chunk]).is_err(),
            "a stranger marked Bob's chunk"
        );
    });
    assert!(!host.vault.is_evictable(bob.store.owner(), &chunk).unwrap());

    // Bounded on the wire as on the host: an oversize request is malformed.
    let too_many = Request::MayEvict {
        owner: bob.store.owner(),
        chunks: vec![chunk; itsanas_net::protocol::MAX_HAVE_BATCH + 1],
    };
    assert_eq!(
        service.handle(&too_many, bob.device.device_id()).unwrap(),
        Response::Refused("malformed request".to_owned())
    );
}
