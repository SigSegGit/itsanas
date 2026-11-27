//! Placement over real sockets: which host gets which chunk, and when a
//! copy may go.
//!
//! The rules themselves are tested pure in `itsanas-placement`; these check
//! that a push feeds them what the store knows and does what they say. The
//! hosts' history is written into the owner's store at stated times rather
//! than lived through: eight days of hourly sessions in a few milliseconds.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};

use itsanas_crypto::{ChunkId, DeviceId, DeviceKeys, MasterSecret, SecretBytes, UserId, UserKeys};
use itsanas_net::{
    PeerClient, PeerServer, PeerService, Placement, PlacementSlot, Pledge, PushReport, Refusal,
    Scope, protocol::PROTOCOL_WITH_EVICTION, session,
};
use itsanas_store::{
    ChunkerConfig, Store, Vault,
    holders::{CONFIRMED_FOR, REFRESH_AFTER},
    now_unix,
};

const HOUR: u64 = 3600;

struct Node {
    _dir: tempfile::TempDir,
    store: Store,
    vault: Vault,
    device: DeviceKeys,
}

impl Node {
    fn id(&self) -> DeviceId {
        self.device.device_id()
    }
}

fn node(master: u8, device_seed: u8) -> Node {
    let dir = tempfile::tempdir().expect("temp dir");
    let device = DeviceKeys::from_seed(&SecretBytes::new([device_seed; 32]));
    let store = Store::open_for_testing(
        dir.path().join("store"),
        UserKeys::derive(&MasterSecret::from_bytes([master; 32])),
        DeviceKeys::from_seed(&device.seed()),
        // Small chunks: a spread is measured over hundreds of them, and
        // hundreds of default-sized chunks are tens of megabytes to seal.
        ChunkerConfig::new(1024, 4096, 16 * 1024).expect("chunker"),
    )
    .expect("store");
    // Every node here runs `measured`, which the daemon arms per round.
    store.set_placement_measured(true).expect("arm");
    let vault = Vault::open(dir.path().join("vault")).expect("vault");
    Node {
        _dir: dir,
        store,
        vault,
        device,
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

/// Push `me`'s content to `host` under `placement`, the host serving `service`.
fn push_to(me: &Node, host: &Node, service: &PeerService<'_>, placement: &Placement) -> PushReport {
    serving(host, service, |address| {
        let mut client =
            PeerClient::connect(address, &me.device, me.store.owner(), None).expect("dial");
        session::push_placed(&me.store, &mut client, Scope::Everything, Some(placement))
            .expect("push")
    })
}

/// Push with a plain, unbounded host service.
fn push_plain(me: &Node, host: &Node, placement: &Placement) -> PushReport {
    let service = PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1))
        .with_owners(&itsanas_net::Unscoped);
    push_to(me, host, &service, placement)
}

/// Every account full: whatever the host is offered for `0` is refused with
/// `ACCOUNT_FULL`, and `0` speaks for its own account.
struct AccountFullFor(DeviceId, UserId);

impl itsanas_net::Owners for AccountFullFor {
    fn may_read(&self, _: itsanas_crypto::DeviceId, _: itsanas_crypto::UserId) -> bool {
        true
    }

    fn present(&self, _: DeviceId, _: &[u8], _: &Vault) -> Result<(), String> {
        Ok(())
    }
    fn admits(
        &self,
        _: DeviceId,
        _: UserId,
        _: u64,
        _: &Store,
        _: &Vault,
        _: u64,
    ) -> Result<(), String> {
        Err(itsanas_net::ACCOUNT_FULL.to_owned())
    }
    fn speaks_for(&self, caller: DeviceId, owner: UserId) -> bool {
        (caller, owner) == (self.0, self.1)
    }
}

fn noise(seed: u8, bytes: usize) -> Vec<u8> {
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

/// The chunks of one freshly written file.
fn write(me: &Node, path: &str, seed: u8, bytes: usize) -> Vec<ChunkId> {
    let before: BTreeSet<ChunkId> = live(me).into_iter().collect();
    me.store
        .write_file(path, &noise(seed, bytes))
        .expect("write");
    live(me)
        .into_iter()
        .filter(|chunk| !before.contains(chunk))
        .collect()
}

fn live(me: &Node) -> Vec<ChunkId> {
    me.store
        .live_chunks_page(None, 100_000)
        .expect("live chunks")
        .0
}

/// Eight days of sessions with `device`, one every six hours, ending at
/// `now`; `seen(i)` says whether the i-th was answered. The last always is,
/// so the device reads as recently seen.
fn history(store: &Store, device: &DeviceId, now: u64, seen: impl Fn(u64) -> bool) {
    let sessions = 8 * 4;
    for index in 0..=sessions {
        let at = now - (sessions - index) * 6 * HOUR;
        if index == 0 || index == sessions || seen(index) {
            store.note_seen_at(device, at).expect("seen");
        } else {
            store.note_unreached_at(device, at).expect("unreached");
        }
    }
}

/// A host this device has watched for a week, always there, audited once.
fn proven(store: &Store, device: &DeviceId, now: u64) {
    history(store, device, now, |_| true);
    store.note_audit(device, true).expect("audit");
}

fn fake(byte: u8) -> DeviceId {
    DeviceId::from_bytes([byte; 32])
}

fn user(byte: u8) -> UserId {
    UserId::from_bytes([byte; 32])
}

fn placement(me: &Node, accounts: &BTreeMap<DeviceId, UserId>, now: u64) -> Placement {
    Placement::build(&me.store, BTreeSet::new(), accounts, now).expect("placement")
}

mod availability;
mod dependencies;
mod inbound;
mod release;
mod spreading;
