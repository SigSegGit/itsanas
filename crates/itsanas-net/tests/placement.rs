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
    let service = PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1));
    push_to(me, host, &service, placement)
}

/// Every account full: whatever the host is offered for `0` is refused with
/// `ACCOUNT_FULL`, and `0` speaks for its own account.
struct AccountFullFor(DeviceId, UserId);

impl itsanas_net::Owners for AccountFullFor {
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

#[test]
fn chunks_spread_over_five_hosts_about_evenly() {
    // Every chunk already has one proven copy with an anchor host, so each
    // needs one more account: the best-ranked of five. Spread is what the
    // rendezvous order gives, and a push to each host in turn sends each
    // chunk once. Sabotage: offer every short chunk to every host.
    let now = now_unix();
    let me = node(0xA1, 1);
    let chunks = write(&me, "photos/a.bin", 1, 1 << 20);
    let n = chunks.len();
    assert!(
        n >= 60,
        "the fixture needs enough chunks to measure a spread: {n}"
    );

    let hosts: Vec<Node> = (0..5).map(|i| node(0xB0 + i, 10 + i)).collect();
    let mut accounts = BTreeMap::new();
    let anchor = fake(0xEE);
    proven(&me.store, &anchor, now);
    accounts.insert(anchor, user(0xEE));
    me.store.record_holders_at(&chunks, &anchor, now).unwrap();
    for host in &hosts {
        proven(&me.store, &host.id(), now);
        accounts.insert(host.id(), host.store.owner());
    }

    let placement = placement(&me, &accounts, now);
    let taken: Vec<usize> = hosts
        .iter()
        .map(|host| push_plain(&me, host, &placement).chunks_accepted)
        .collect();

    assert_eq!(
        taken.iter().sum::<usize>(),
        n,
        "each chunk once, nowhere twice: {taken:?}"
    );
    for share in &taken {
        assert!(
            *share >= n / 10 && *share <= n * 3 / 10,
            "one fifth each, within a bound: {taken:?} of {n}"
        );
    }
    assert_eq!(me.store.short_count().unwrap(), 0, "every chunk satisfied");
}

#[test]
fn a_host_full_under_either_refusal_falls_through_to_the_next() {
    // A full host is ineligible from the next round on, and its share goes
    // to the next host in the order. Both refusals, the host's pledge and the
    // account's share, count. Sabotage: stamp `note_peer_full` only for
    // `PledgeFull` in a placed push.
    for account_full in [false, true] {
        let now = now_unix();
        let me = node(0xA1, 1);
        let chunks = write(&me, "photos/a.bin", 2, 512 << 10);
        let full = node(0xB1, 11);
        let next = node(0xB2, 12);
        let anchor = fake(0xEE);
        let mut accounts = BTreeMap::new();
        proven(&me.store, &anchor, now);
        accounts.insert(anchor, user(0xEE));
        me.store.record_holders_at(&chunks, &anchor, now).unwrap();
        for host in [&full, &next] {
            proven(&me.store, &host.id(), now);
            accounts.insert(host.id(), host.store.owner());
        }

        let gate = AccountFullFor(me.id(), me.store.owner());
        let refusing = if account_full {
            PeerService::new(&full.store, &full.vault, Pledge::gigabytes(1)).with_owners(&gate)
        } else {
            PeerService::new(&full.store, &full.vault, Pledge::bytes(0))
        };
        let expected = if account_full {
            Refusal::AccountFull
        } else {
            Refusal::PledgeFull
        };

        let first = placement(&me, &accounts, now);
        let refused = push_to(&me, &full, &refusing, &first);
        assert_eq!(refused.refusal, Some(expected), "{refused:?}");
        assert_eq!(refused.chunks_accepted, 0);
        let early = push_plain(&me, &next, &first).chunks_accepted;
        assert!(early < chunks.len(), "the full host's share waits a round");

        let second = placement(&me, &accounts, now + 60);
        let late = push_plain(&me, &next, &second).chunks_accepted;
        assert_eq!(
            early + late,
            chunks.len(),
            "account_full={account_full}: the full host's share fell through"
        );
    }
}

#[test]
fn a_chunk_with_no_proven_copy_goes_to_the_first_host_reached() {
    // No delay, no ranking: a new photograph's first copy goes to whoever
    // answers, as before placement. Once a proven host holds it, a stranger
    // reached later is offered nothing. Sabotage: drop the "no proven remote
    // holder" rule.
    let now = now_unix();
    let me = node(0xA1, 1);
    let chunks = write(&me, "photos/new.bin", 3, 256 << 10);
    let stranger = node(0xB1, 11);

    let report = push_plain(&me, &stranger, &placement(&me, &BTreeMap::new(), now));
    assert_eq!(report.chunks_accepted, chunks.len(), "{report:?}");

    // The stranger proves itself; a second stranger now gets nothing.
    proven(&me.store, &stranger.id(), now);
    let mut accounts = BTreeMap::new();
    accounts.insert(stranger.id(), stranger.store.owner());
    let later = node(0xB2, 12);
    let report = push_plain(&me, &later, &placement(&me, &accounts, now));
    assert_eq!(report.chunks_offered, 0, "{report:?}");
}

#[test]
fn red_team_the_safety_net_waits_a_day_and_serves_only_durability() {
    // The host shares the anchor's account, so it is never wanted: the
    // chunk needs another account, and that host is ranked. Only after a day
    // without durability does any eligible host do. Sabotage: drop the
    // SAFETY_GRACE comparison, or the `!durable` condition.
    let now = now_unix();
    let me = node(0xA1, 1);
    let chunks = write(&me, "photos/a.bin", 4, 256 << 10);
    let host = node(0xB1, 11);
    let anchor = fake(0xEE);
    let ranked = fake(0xEF);
    let mut accounts = BTreeMap::new();
    for (device, account) in [
        (anchor, user(0xEE)),
        (ranked, user(0xEF)),
        (host.id(), user(0xEE)),
    ] {
        proven(&me.store, &device, now);
        accounts.insert(device, account);
    }
    me.store.record_holders_at(&chunks, &anchor, now).unwrap();
    let placement = placement(&me, &accounts, now);

    me.store.settle_short(&chunks).unwrap();
    me.store.enqueue_short(&chunks, now - 23 * HOUR).unwrap();
    let report = push_plain(&me, &host, &placement);
    assert_eq!(report.chunks_offered, 0, "under a day: {report:?}");

    me.store.settle_short(&chunks).unwrap();
    me.store.enqueue_short(&chunks, now - 25 * HOUR).unwrap();
    let report = push_plain(&me, &host, &placement);
    assert_eq!(
        report.chunks_accepted,
        chunks.len(),
        "past a day: {report:?}"
    );
}

#[test]
fn red_team_a_capped_chunk_short_only_on_availability_is_not_sent_everywhere() {
    // Two proven accounts that are each up half the time: durable, not
    // available, and twelve remote copies counting the unproven ones. A day
    // later still nobody new is offered it: that is the chunk "send it to
    // whoever answers" would send to every host. The control removes one
    // unproven copy, under the cap, and the host is wanted again.
    // Sabotage: let the safety net take a durable chunk, or drop the cap.
    for unproven in [10u8, 9] {
        let now = now_unix();
        let me = node(0xA1, 1);
        let chunks = write(&me, "photos/a.bin", 5, 128 << 10);
        let host = node(0xB1, 11);
        let mut accounts = BTreeMap::new();
        for byte in [0xE0, 0xE1] {
            history(&me.store, &fake(byte), now, |index| index % 2 == 0);
            me.store.note_audit(&fake(byte), true).unwrap();
            accounts.insert(fake(byte), user(byte));
            me.store
                .record_holders_at(&chunks, &fake(byte), now)
                .unwrap();
        }
        for byte in 0..unproven {
            me.store
                .record_holders_at(&chunks, &fake(byte), now)
                .unwrap();
        }
        proven(&me.store, &host.id(), now);
        accounts.insert(host.id(), host.store.owner());

        me.store.settle_short(&chunks).unwrap();
        me.store.enqueue_short(&chunks, now - 48 * HOUR).unwrap();
        let report = push_plain(&me, &host, &placement(&me, &accounts, now));
        if unproven == 10 {
            assert_eq!(report.chunks_offered, 0, "capped: {report:?}");
        } else {
            assert_eq!(
                report.chunks_accepted,
                chunks.len(),
                "under the cap: {report:?}"
            );
        }
    }
}

#[test]
fn the_due_walk_restamps_on_recorded_only_digests_and_asks_only_recorded_chunks() {
    // The host holds the first file only. Against this device's whole set
    // every bucket would differ; against what is recorded for the host they
    // agree, so the walk asks nothing. Then the host loses one chunk: only
    // that bucket's recorded chunks are asked about, and the lost one is
    // sent back by the short queue (one more question). Sabotage: hash the
    // whole bucket, or ask the whole bucket.
    let wall = now_unix();
    let me = node(0xA1, 1);
    let held = write(&me, "photos/a.bin", 6, 256 << 10);
    let host = node(0xB1, 11);
    serving(
        &host,
        &PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1)),
        |address| {
            let mut client =
                PeerClient::connect(address, &me.device, me.store.owner(), None).expect("dial");
            session::push(&me.store, &mut client).expect("push");
        },
    );
    let extra = write(&me, "photos/b.bin", 7, 256 << 10);
    me.store.settle_short(&live(&me)).unwrap();
    proven(&me.store, &host.id(), wall);
    let mut accounts = BTreeMap::new();
    accounts.insert(host.id(), host.store.owner());

    let now = wall + REFRESH_AFTER + 60;
    let report = push_plain(&me, &host, &placement(&me, &accounts, now));
    assert_eq!(report.chunks_asked_about, 0, "{report:?}");
    assert_eq!(report.holders_restamped, held.len(), "{report:?}");
    assert!(!extra.is_empty());

    let lost = held[0];
    assert!(host.vault.remove_chunk(me.store.owner(), &lost).unwrap());
    let in_bucket = held
        .iter()
        .filter(|chunk| chunk.as_bytes()[0] == lost.as_bytes()[0])
        .count();
    let later = now + REFRESH_AFTER + 60;
    me.store.note_seen_at(&host.id(), later).unwrap();
    let report = push_plain(&me, &host, &placement(&me, &accounts, later));
    assert_eq!(
        report.chunks_asked_about,
        in_bucket + 1,
        "the bucket's recorded chunks, then the lost one offered back: {report:?}"
    );
    assert_eq!(report.holders_restamped, held.len() - in_bucket);
    assert_eq!(report.chunks_accepted, 1, "the lost chunk went back");
}

/// The §E fixture: a host whose account share is full, chunks waiting for
/// it, and copies on it held with margin by three other proven accounts.
struct Release {
    now: u64,
    me: Node,
    host: Node,
    accounts: BTreeMap<DeviceId, UserId>,
    /// Held by the host and three others.
    spare: Vec<ChunkId>,
    /// Held by the host and one other: no margin.
    thin: Vec<ChunkId>,
    /// Recorded on the host and the others, not held here.
    ghosts: Vec<ChunkId>,
    others: [DeviceId; 3],
}

impl Release {
    fn new() -> Self {
        let now = now_unix();
        let me = node(0xA1, 1);
        let host = node(0xB1, 11);
        let spare = write(&me, "old/spare.bin", 8, 256 << 10);
        let thin = write(&me, "old/thin.bin", 9, 128 << 10);
        // What waits for the host: more than could ever be released.
        write(&me, "new/waiting.bin", 10, 1 << 20);
        let others = [fake(0xE1), fake(0xE2), fake(0xE3)];
        let mut accounts = BTreeMap::new();
        proven(&me.store, &host.id(), now);
        accounts.insert(host.id(), host.store.owner());
        for (index, device) in others.iter().enumerate() {
            proven(&me.store, device, now);
            accounts.insert(*device, user(0xE1 + u8::try_from(index).unwrap()));
            // Ineligible, so the waiting chunks want the host alone.
            me.store.note_peer_full(device, now).unwrap();
        }
        let ghosts: Vec<ChunkId> = (0..16u8).map(|b| ChunkId::from_bytes([b; 32])).collect();
        for chunk in spare.iter().chain(&thin) {
            let sealed = me.store.blobs().get(chunk).unwrap().unwrap();
            host.vault
                .put_chunk(me.store.owner(), chunk, &sealed)
                .unwrap();
        }
        // The host does hold the ghosts (this device let go of its copies),
        // so the released-chunk refresh keeps their records.
        for ghost in &ghosts {
            host.vault.put_chunk(me.store.owner(), ghost, b"x").unwrap();
        }
        for device in std::iter::once(host.id()).chain(others) {
            me.store.record_holders_at(&spare, &device, now).unwrap();
            me.store.record_holders_at(&ghosts, &device, now).unwrap();
        }
        for device in [host.id(), others[0]] {
            me.store.record_holders_at(&thin, &device, now).unwrap();
        }
        me.store.settle_short(&spare).unwrap();
        me.store.settle_short(&thin).unwrap();
        Self {
            now,
            me,
            host,
            accounts,
            spare,
            thin,
            ghosts,
            others,
        }
    }

    fn placement(&self, now: u64) -> Placement {
        placement(&self.me, &self.accounts, now)
    }

    fn push(&self, version: u16, now: u64) -> PushReport {
        let gate = AccountFullFor(self.me.id(), self.me.store.owner());
        let service = PeerService::new(&self.host.store, &self.host.vault, Pledge::gigabytes(1))
            .with_owners(&gate)
            .speaking_at_most(version);
        push_to(&self.me, &self.host, &service, &self.placement(now))
    }

    /// The spare chunks placement would release on this host.
    fn chosen(&self) -> Vec<ChunkId> {
        let placement = self.placement(self.now);
        self.spare
            .iter()
            .copied()
            .filter(|chunk| {
                placement.release_choice(&self.me.store, chunk).unwrap() == Some(self.host.id())
            })
            .collect()
    }

    fn recorded(&self, chunks: &[ChunkId]) -> Vec<ChunkId> {
        self.me.store.with_record(chunks, &self.host.id()).unwrap()
    }
}

#[test]
fn red_team_release_needs_a_local_copy_and_margin_and_happens_once_per_confirmed_window() {
    // Sabotage: drop the `has_chunk` check (a ghost goes), the margin (a thin
    // chunk goes), or the `released_at` check (the second push releases
    // again).
    let fixture = Release::new();
    let chosen = fixture.chosen();
    assert!(
        !chosen.is_empty(),
        "the fixture must have a spare copy to release"
    );
    let placement = fixture.placement(fixture.now);
    assert!(
        fixture.ghosts.iter().any(|ghost| {
            placement.release_choice(&fixture.me.store, ghost).unwrap() == Some(fixture.host.id())
        }),
        "a ghost must qualify on the ledger alone, or the local-copy rule is untested"
    );

    let report = fixture.push(PROTOCOL_WITH_EVICTION, fixture.now);
    assert_eq!(report.refusal, Some(Refusal::AccountFull), "{report:?}");
    assert_eq!(report.released, chosen.len(), "{report:?}");
    let owner = fixture.me.store.owner();
    for chunk in &chosen {
        assert!(fixture.host.vault.is_evictable(owner, chunk).unwrap());
        assert!(
            fixture
                .me
                .store
                .released_at(chunk, &fixture.host.id())
                .unwrap()
                .is_some()
        );
    }
    assert!(
        fixture.recorded(&chosen).is_empty(),
        "the owner stops counting at once"
    );
    assert_eq!(
        fixture.recorded(&fixture.thin).len(),
        fixture.thin.len(),
        "no margin"
    );
    assert_eq!(
        fixture.recorded(&fixture.ghosts).len(),
        fixture.ghosts.len(),
        "released without a copy here"
    );

    // The host confirms the copies again (it had not evicted yet); an hour
    // past the full window the same choice is not released twice.
    let later = fixture.now + itsanas_net::session::FULL_RETRY + 60;
    fixture
        .me
        .store
        .record_holders_at(&chosen, &fixture.host.id(), fixture.now)
        .unwrap();
    for device in &fixture.others {
        fixture.me.store.note_peer_full(device, later).unwrap();
    }
    let again = fixture.push(PROTOCOL_WITH_EVICTION, later);
    assert_eq!(again.refusal, Some(Refusal::AccountFull), "{again:?}");
    assert_eq!(
        again.released, 0,
        "released twice within CONFIRMED_FOR: {again:?}"
    );
    assert!(CONFIRMED_FOR > later - fixture.now);
}

#[test]
fn red_team_release_is_never_sent_to_a_protocol_9_peer() {
    // The peer cannot decode `MayEvict`; nothing is released and every record
    // stands. Sabotage: forget the records before `may_evict` answers.
    let fixture = Release::new();
    let chosen = fixture.chosen();
    assert!(!chosen.is_empty());
    let report = fixture.push(PROTOCOL_WITH_EVICTION - 1, fixture.now);
    assert_eq!(report.refusal, Some(Refusal::AccountFull), "{report:?}");
    assert_eq!(report.released, 0, "{report:?}");
    assert_eq!(fixture.recorded(&chosen).len(), chosen.len());
    for chunk in &chosen {
        assert_eq!(
            fixture
                .me
                .store
                .released_at(chunk, &fixture.host.id())
                .unwrap(),
            None
        );
    }
}

/// Open one session from `caller` to `host`, serving `service`, and run
/// `body` on it.
fn dial_in<T>(
    caller: &Node,
    host: &Node,
    service: &PeerService<'_>,
    body: impl FnOnce(&mut PeerClient) -> T,
) -> T {
    serving(host, service, |address| {
        let mut client =
            PeerClient::connect(address, &caller.device, caller.store.owner(), None).expect("dial");
        body(&mut client)
    })
}

#[test]
fn an_inbound_only_host_is_measured_from_the_sessions_it_opens() {
    // A host behind a router is never reached by a dial; the sessions it
    // opens are the only evidence the owner gets. Sabotage: drop the
    // `note_seen_if_known` call on accept.
    let me = node(0xA1, 1);
    let host = node(0xB1, 11);
    let long_ago = now_unix() - 72 * HOUR;
    me.store.note_seen_at(&host.id(), long_ago).expect("seen");
    let before = me.store.uptime(&host.id()).unwrap().expect("a ring");
    assert_eq!(before.last_seen_hour(), Some(long_ago / HOUR));

    let service = PeerService::new(&me.store, &me.vault, Pledge::gigabytes(1));
    dial_in(&host, &me, &service, |client| {
        client.heads(me.store.owner())
    })
    .expect("heads");

    let after = me.store.uptime(&host.id()).unwrap().expect("a ring");
    assert!(
        after.last_seen_hour() >= Some(now_unix() / HOUR - 1),
        "{after:?}"
    );
}

#[test]
fn red_team_a_stranger_dialling_in_gets_no_ring() {
    // Anybody can open a session; a ring per caller would be a table any
    // stranger grows with fresh keys. Sabotage: `note_seen_at` in place of
    // `note_seen_if_known_at`.
    let me = node(0xA1, 1);
    let stranger = node(0xB1, 11);
    let service = PeerService::new(&me.store, &me.vault, Pledge::gigabytes(1));
    dial_in(&stranger, &me, &service, |client| {
        client.heads(me.store.owner())
    })
    .expect("heads");
    assert_eq!(me.store.uptime(&stranger.id()).unwrap(), None);
    assert!(me.store.uptimes().unwrap().is_empty());
}

#[test]
fn want_hosted_offers_the_caller_its_share_only_in_measured_mode() {
    // The caller shares the anchor's account, so a chunk the anchor holds
    // wants another account, never the caller; a chunk with no proven copy
    // goes to whoever asks. Without a placement in the slot the answer is
    // today's: everything under-replicated. Sabotage: answer from
    // `under_replicated` whatever the slot holds, or drop the share filter.
    let now = now_unix();
    let me = node(0xA1, 1);
    let anchored = write(&me, "photos/kept.bin", 5, 48 << 10);
    let fresh = write(&me, "photos/new.bin", 6, 48 << 10);
    assert!(anchored.len() > 1 && fresh.len() > 1);
    assert!(anchored.len() + fresh.len() <= 64);
    let host = node(0xB1, 11);
    let anchor = fake(0xEE);
    let ranked = fake(0xEF);
    let mut accounts = BTreeMap::new();
    for (device, account) in [
        (anchor, user(0xEE)),
        (ranked, user(0xEF)),
        (host.id(), user(0xEE)),
    ] {
        proven(&me.store, &device, now);
        accounts.insert(device, account);
    }
    me.store.record_holders_at(&anchored, &anchor, now).unwrap();
    me.store.settle_short(&anchored).unwrap();
    me.store.enqueue_short(&anchored, now - HOUR).unwrap();
    me.store.enqueue_short(&fresh, now - HOUR).unwrap();

    let slot = PlacementSlot::default();
    let service =
        PeerService::new(&me.store, &me.vault, Pledge::gigabytes(1)).with_placement(&slot);
    let ask = || {
        let (owner, mut chunks) =
            dial_in(&host, &me, &service, |client| client.want_hosted(64)).expect("want");
        assert_eq!(owner, me.store.owner());
        chunks.sort();
        chunks
    };

    // Empty slot: today's answer, both files.
    let mut everything: Vec<ChunkId> = anchored.iter().chain(&fresh).copied().collect();
    everything.sort();
    assert_eq!(ask(), everything);

    *slot.write().unwrap() = Some(std::sync::Arc::new(placement(&me, &accounts, now)));
    let mut share = fresh.clone();
    share.sort();
    assert_eq!(
        ask(),
        share,
        "measured: only the chunks with no proven copy"
    );
}
