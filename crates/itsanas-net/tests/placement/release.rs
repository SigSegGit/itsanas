//! When a copy may go: a local copy, margin and a recent audit, once per
//! confirmation, and never to a peer too old to understand it.

use super::*;

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
    assert_ne!(chosen, [] as [itsanas_crypto::ChunkId; 0]);
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

#[test]
fn red_team_release_needs_an_audit_passed_within_live_for() {
    // `passed > 0` latches for ever after one audit, and a hosted self-claim
    // stamps a record fresh without proving anything. A week and more after
    // the last pass, with every host still seen and every record fresh, no
    // copy may be released. Sabotage: drop `passed_lately` from
    // `release_choice`.
    let fixture = Release::new();
    assert!(!fixture.chosen().is_empty(), "the fixture must release");
    let later = fixture.now + itsanas_store::holders::LIVE_FOR + HOUR;
    let devices: Vec<DeviceId> = std::iter::once(fixture.host.id())
        .chain(fixture.others)
        .collect();
    for device in &devices {
        history(&fixture.me.store, device, later, |_| true);
        fixture
            .me
            .store
            .record_holders_at(&fixture.spare, device, later)
            .unwrap();
    }
    let placement = fixture.placement(later);
    for chunk in &fixture.spare {
        assert_eq!(
            placement.release_choice(&fixture.me.store, chunk).unwrap(),
            None,
            "released on an audit passed more than LIVE_FOR ago"
        );
    }
}
