//! A host that bounds accounts: asked before every store, eviction marks,
//! summaries of marked chunks, and the real overage.

use super::*;

/// Lets one device store, after it presented the claim `b"ok"`.
struct Gate {
    allowed: std::sync::Mutex<Option<DeviceId>>,
}

impl Owners for Gate {
    fn may_read(&self, _: DeviceId, _: UserId) -> bool {
        true
    }

    fn present(
        &self,
        caller: DeviceId,
        claim: &[u8],
        _vault: &Vault,
    ) -> std::result::Result<(), String> {
        if claim != b"ok" {
            return Err("bad claim".to_owned());
        }
        *self.allowed.lock().unwrap() = Some(caller);
        Ok(())
    }
    fn admits(
        &self,
        caller: DeviceId,
        _owner: UserId,
        _incoming: u64,
        _store: &Store,
        _vault: &Vault,
        _host_pledge: u64,
    ) -> std::result::Result<(), String> {
        match *self.allowed.lock().unwrap() {
            Some(allowed) if allowed == caller => Ok(()),
            _ => Err(UPDATE_REQUIRED.to_owned()),
        }
    }
}

#[test]
fn red_team_a_host_that_bounds_accounts_asks_before_every_store() {
    // §8 1c. The bound is worth nothing if one of the two store verbs skips
    // it, or if the claim is credited to a device other than the one TLS
    // proved. Sabotages: skip the gate in `StoreChunk`; pass a fixed device
    // to `present`.
    let node = node(&alice(), 1);
    let gate = Gate {
        allowed: std::sync::Mutex::new(None),
    };
    let service = service(&node).with_owners(&gate);
    let (them, other) = (
        DeviceId::from_bytes([0x31; 32]),
        DeviceId::from_bytes([0x32; 32]),
    );
    let store = Request::StoreChunk {
        owner: node.store.owner(),
        address: itsanas_crypto::ChunkId::from_bytes([9; 32]),
        sealed: vec![1; 10],
    };

    assert_eq!(
        service.handle(&store, them).unwrap(),
        Response::Refused(UPDATE_REQUIRED.to_owned()),
        "a chunk was stored by a device that presented no claim"
    );
    assert_eq!(
        service
            .handle(
                &Request::Claim {
                    claim: b"ok".to_vec()
                },
                them
            )
            .unwrap(),
        Response::Stored { accepted: true }
    );
    assert_eq!(
        service.handle(&store, them).unwrap(),
        Response::Stored { accepted: true }
    );
    assert_eq!(
        service.handle(&store, other).unwrap(),
        Response::Refused(UPDATE_REQUIRED.to_owned()),
        "a claim presented by one device let another store"
    );

    node.store.write_file("a.txt", b"segment").unwrap();
    let envelope = node.store.flush_segment().unwrap().expect("a segment");
    assert_eq!(
        service
            .handle(
                &Request::StoreSegment {
                    envelope: Box::new(envelope),
                },
                other
            )
            .unwrap(),
        Response::Refused(UPDATE_REQUIRED.to_owned()),
        "a segment was stored by a device that presented no claim"
    );

    // Two-way: a host that knows its own claim answers with it, so a
    // dialling machine behind a router can hold this host to the same
    // rule when it pulls this host's chunks.
    let answering = super::PeerService::new(&node.store, &node.vault, Pledge::gigabytes(1))
        .with_owners(&gate)
        .claiming(b"mine".to_vec());
    assert_eq!(
        answering
            .handle(
                &Request::Claim {
                    claim: b"ok".to_vec()
                },
                them
            )
            .unwrap(),
        Response::Claim(b"mine".to_vec()),
        "a host answered a claim without its own, so a machine that is never \
         dialled can never learn whose the hosts it pulls for are"
    );
}

/// Bob's device speaks for Bob, and Bob may hold `room` bytes here.
struct Bound {
    speaks: std::collections::BTreeMap<DeviceId, UserId>,
    room: u64,
}

impl Owners for Bound {
    fn may_read(&self, _: DeviceId, _: UserId) -> bool {
        true
    }

    fn present(&self, _: DeviceId, _: &[u8], _: &Vault) -> std::result::Result<(), String> {
        Ok(())
    }
    fn admits(
        &self,
        caller: DeviceId,
        owner: UserId,
        incoming: u64,
        _store: &Store,
        vault: &Vault,
        _host_pledge: u64,
    ) -> std::result::Result<(), String> {
        if self.speaks.get(&caller) != Some(&owner) {
            return Err(UPDATE_REQUIRED.to_owned());
        }
        let mine = vault.held_bytes_for(owner).map_err(|e| e.to_string())?;
        if mine.saturating_add(incoming) > self.room {
            return Err(format!("{ACCOUNT_FULL}: {mine} of {} bytes", self.room));
        }
        Ok(())
    }
    fn speaks_for(&self, caller: DeviceId, owner: UserId) -> bool {
        self.speaks.get(&caller) == Some(&owner)
    }
}

fn bob() -> UserId {
    UserKeys::derive(&MasterSecret::from_bytes([0xB0; 32])).user_id()
}

fn carol() -> UserId {
    UserKeys::derive(&MasterSecret::from_bytes([0xC0; 32])).user_id()
}

const BOBS: DeviceId = DeviceId::from_bytes([0x41; 32]);
const CAROLS: DeviceId = DeviceId::from_bytes([0x42; 32]);

fn bound(room: u64) -> Bound {
    Bound {
        speaks: [(BOBS, bob()), (CAROLS, carol())].into_iter().collect(),
        room,
    }
}

fn store_for(owner: UserId, byte: u8) -> Request {
    Request::StoreChunk {
        owner,
        address: ChunkId::from_bytes([byte; 32]),
        sealed: vec![byte; 10],
    }
}

fn may_evict(owner: UserId, bytes: &[u8]) -> Request {
    Request::MayEvict {
        owner,
        chunks: bytes
            .iter()
            .map(|b| ChunkId::from_bytes([*b; 32]))
            .collect(),
    }
}

#[test]
fn red_team_permission_to_evict_is_refused_from_another_account_a_stranger_and_with_no_bound() {
    // A mark lets the account's next store remove a copy the account
    // still counts. Anyone but a device that speaks for the account --
    // Carol's device naming Bob, a device in no book, any device on a
    // host that bounds nobody, or a device naming this host's own
    // account -- must mark nothing. Sabotages: skip `speaks_for`; treat a
    // host without `Owners` as open; drop the own-account check.
    let node = node(&alice(), 3);
    let gate = bound(1_000);
    let bounded = service(&node).with_owners(&gate);
    let open = service(&node);
    bounded.handle(&store_for(bob(), 1), BOBS).unwrap();
    let refused = Response::Refused(NOT_YOURS_TO_EVICT.to_owned());

    let stranger = DeviceId::from_bytes([0x43; 32]);
    assert_eq!(
        bounded.handle(&may_evict(bob(), &[1]), CAROLS).unwrap(),
        refused
    );
    assert_eq!(
        bounded.handle(&may_evict(bob(), &[1]), stranger).unwrap(),
        refused
    );
    assert_eq!(open.handle(&may_evict(bob(), &[1]), BOBS).unwrap(), refused);
    let mine = Bound {
        speaks: [(BOBS, node.store.owner())].into_iter().collect(),
        room: 1_000,
    };
    assert_eq!(
        service(&node)
            .with_owners(&mine)
            .handle(&may_evict(node.store.owner(), &[1]), BOBS)
            .unwrap(),
        refused,
        "a device marked this host's own account's chunks"
    );
    assert!(
        !node
            .vault
            .is_evictable(bob(), &ChunkId::from_bytes([1; 32]))
            .unwrap()
    );

    assert_eq!(
        bounded.handle(&may_evict(bob(), &[1, 2]), BOBS).unwrap(),
        Response::MayEvict { marked: 1 },
        "the account's own device was not let mark the one chunk held"
    );
}

#[test]
fn red_team_a_marked_chunk_is_answered_missing_and_left_out_of_the_summary() {
    // A host that still says "have" for a chunk it is about to evict has
    // the owner re-stamp the copy it just released, and count it after
    // the eviction. Answered missing, and summarised as absent, the owner
    // re-offers it, and that store unmarks it. Sabotages: answer
    // `HaveChunks` from the blobs alone; summarise every row.
    let node = node(&alice(), 5);
    let gate = bound(1_000);
    let service = service(&node).with_owners(&gate);
    for byte in [1, 2] {
        service.handle(&store_for(bob(), byte), BOBS).unwrap();
    }
    let summary = |service: &super::PeerService<'_>| {
        service
            .handle(&Request::ChunkSummary { owner: bob() }, BOBS)
            .unwrap()
    };
    let before = summary(&service);
    service.handle(&may_evict(bob(), &[1]), BOBS).unwrap();

    let have = Request::HaveChunks {
        owner: bob(),
        addresses: vec![ChunkId::from_bytes([1; 32]), ChunkId::from_bytes([2; 32])],
    };
    assert_eq!(
        service.handle(&have, BOBS).unwrap(),
        Response::Missing(vec![ChunkId::from_bytes([1; 32])])
    );
    assert_ne!(summary(&service), before, "a marked chunk was summarised");

    // The re-offer goes through `put_chunk`, which unmarks.
    service.handle(&store_for(bob(), 1), BOBS).unwrap();
    assert_eq!(
        service.handle(&have, BOBS).unwrap(),
        Response::Missing(Vec::new())
    );
    assert_eq!(summary(&service), before);
}

#[test]
fn red_team_a_summary_root_is_the_root_of_the_summary_it_stands_for() {
    // `SummaryRoot` lets an idle round skip the 256 digests (#443's
    // budgets: 13.9 KB a Wi-Fi round, most of it them). An owner whose
    // root matches takes the sets for equal and re-stamps without asking,
    // so a root over another set than the summary's -- the blobs rather
    // than what is held, or a marked chunk counted -- would have it count
    // copies a host no longer has. Sabotage: the root of the store's own
    // summary for every owner; a constant.
    let node = node(&alice(), 6);
    let gate = bound(1_000);
    let service = service(&node).with_owners(&gate);
    for byte in [1, 2, 3] {
        service.handle(&store_for(bob(), byte), BOBS).unwrap();
    }
    service.handle(&may_evict(bob(), &[2]), BOBS).unwrap();
    let root_of = |owner| match service
        .handle(&Request::ChunkSummary { owner }, BOBS)
        .unwrap()
    {
        Response::ChunkSummary(digests) => itsanas_store::summary::root(&digests),
        other => panic!("expected a summary, got {other:?}"),
    };
    for owner in [bob(), node.store.owner()] {
        assert_eq!(
            service
                .handle(&Request::SummaryRoot { owner }, BOBS)
                .unwrap(),
            Response::SummaryRoot(root_of(owner)),
            "the root answered is not the root of the summary: an owner re-stamps copies on a set the host does not hold"
        );
    }
    assert_ne!(
        root_of(bob()),
        root_of(node.store.owner()),
        "the two owners' sets are alike here, so the test proves nothing"
    );
}

#[test]
fn red_team_eviction_is_asked_for_the_real_overage() {
    // An account already past its share (its allowance shrank) needs
    // more freed than the incoming chunk. Asking for the incoming size
    // evicts one chunk, is refused anyway, and the copy went for
    // nothing. Sabotage: ask `evict_marked` for `sealed.len()`.
    assert_eq!(super::overage("account full: 50 of 20 bytes", 10), 40);
    assert_eq!(super::overage("account full: 15 of 20 bytes", 10), 5);
    assert_eq!(super::overage("something else", 10), 10);

    let node = node(&alice(), 6);
    let gate = bound(1_000);
    let roomy = service(&node).with_owners(&gate);
    for byte in 1..=4 {
        roomy.handle(&store_for(bob(), byte), BOBS).unwrap();
    }
    roomy.handle(&may_evict(bob(), &[1, 2, 3]), BOBS).unwrap();
    // Bob's share shrinks to 20 bytes: he holds 40, wants 10 more, and
    // 30 must go. His three marks cover it.
    let tight = bound(20);
    let tight = service(&node).with_owners(&tight);
    assert_eq!(
        tight.handle(&store_for(bob(), 5), BOBS).unwrap(),
        Response::Stored { accepted: true }
    );
    assert_eq!(node.vault.held_bytes_for(bob()).unwrap(), 20);
}

#[test]
fn a_full_account_makes_room_from_its_own_marks_and_never_from_anothers() {
    // Bob is at his share; Carol has marked spares. Bob's store must not
    // evict Carol's, and must evict his own only once his account is the
    // one refused for room. Sabotages: evict for any refusal; evict
    // before asking the bound; evict across owners.
    let node = node(&alice(), 4);
    let gate = bound(20);
    let service = service(&node).with_owners(&gate);
    for byte in [1, 2] {
        assert_eq!(
            service.handle(&store_for(bob(), byte), BOBS).unwrap(),
            Response::Stored { accepted: true }
        );
        service.handle(&store_for(carol(), byte), CAROLS).unwrap();
    }
    service
        .handle(&may_evict(carol(), &[1, 2]), CAROLS)
        .unwrap();

    match service.handle(&store_for(bob(), 3), BOBS).unwrap() {
        Response::Refused(why) => assert!(why.starts_with(ACCOUNT_FULL), "{why}"),
        other => panic!("Bob stored past his share on Carol's marks: {other:?}"),
    }
    assert_eq!(node.vault.held_bytes_for(carol()).unwrap(), 20);

    // A device with no say gets the same refusal and evicts nothing.
    service.handle(&may_evict(bob(), &[1]), BOBS).unwrap();
    let stranger = DeviceId::from_bytes([0x43; 32]);
    assert_eq!(
        service.handle(&store_for(bob(), 3), stranger).unwrap(),
        Response::Refused(UPDATE_REQUIRED.to_owned())
    );
    assert!(
        node.vault
            .has_chunk(bob(), &ChunkId::from_bytes([1; 32]))
            .unwrap()
    );

    // Not full: a store that fits evicts nothing even with marks waiting.
    let roomy = bound(1_000);
    let roomy =
        super::PeerService::new(&node.store, &node.vault, Pledge::gigabytes(1)).with_owners(&roomy);
    roomy.handle(&store_for(bob(), 4), BOBS).unwrap();
    assert!(
        node.vault
            .has_chunk(bob(), &ChunkId::from_bytes([1; 32]))
            .unwrap()
    );
    node.vault
        .remove_chunk(bob(), &ChunkId::from_bytes([4; 32]))
        .unwrap();

    assert_eq!(
        service.handle(&store_for(bob(), 3), BOBS).unwrap(),
        Response::Stored { accepted: true },
        "Bob's own spare did not make room for Bob"
    );
    assert!(
        !node
            .vault
            .has_chunk(bob(), &ChunkId::from_bytes([1; 32]))
            .unwrap()
    );
    assert!(
        node.vault
            .has_chunk(bob(), &ChunkId::from_bytes([2; 32]))
            .unwrap()
    );
    assert_eq!(node.vault.held_bytes_for(bob()).unwrap(), 20);
}
