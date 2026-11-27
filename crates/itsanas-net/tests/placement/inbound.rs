//! Hosts that only dial in: measured from the sessions they open, and offered
//! their share once a round, never a stranger.

use super::*;

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

    let service = PeerService::new(&me.store, &me.vault, Pledge::gigabytes(1))
        .with_owners(&itsanas_net::Unscoped);
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
    let service = PeerService::new(&me.store, &me.vault, Pledge::gigabytes(1))
        .with_owners(&itsanas_net::Unscoped);
    dial_in(&stranger, &me, &service, |client| {
        client.heads(me.store.owner())
    })
    .expect("heads");
    assert_eq!(me.store.uptime(&stranger.id()).unwrap(), None);
    assert_eq!(
        me.store.uptimes().unwrap(),
        [] as [(itsanas_crypto::DeviceId, itsanas_placement::uptime::Uptime); 0]
    );
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
    let service = PeerService::new(&me.store, &me.vault, Pledge::gigabytes(1))
        .with_owners(&itsanas_net::Unscoped)
        .with_placement(&slot);
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

#[test]
fn red_team_want_hosted_is_measured_once_a_round_and_never_for_a_stranger() {
    // Each measured answer evaluates a page of the queue. A host asking on
    // every connection, or a stranger with no share at all, would set this
    // node to that work as often as it liked. Sabotage: drop `take_pull`.
    let now = now_unix();
    let me = node(0xA1, 1);
    write(&me, "photos/new.bin", 13, 48 << 10);
    let host = node(0xB1, 11);
    let stranger = node(0xC1, 21);
    proven(&me.store, &host.id(), now);
    let accounts: BTreeMap<DeviceId, UserId> = [(host.id(), user(0xB1))].into_iter().collect();
    let slot = PlacementSlot::default();
    let placement = std::sync::Arc::new(placement(&me, &accounts, now));
    *slot.write().unwrap() = Some(placement.clone());
    let service = PeerService::new(&me.store, &me.vault, Pledge::gigabytes(1))
        .with_owners(&itsanas_net::Unscoped)
        .with_placement(&slot);
    let ask = |who: &Node| {
        dial_in(who, &me, &service, |client| client.want_hosted(64))
            .expect("want")
            .1
    };

    assert!(ask(&stranger).is_empty(), "a stranger was given a share");
    assert_eq!(placement.evaluations(), 0, "a stranger cost an evaluation");
    assert!(!ask(&host).is_empty(), "the host's share");
    let spent = placement.evaluations();
    assert!(ask(&host).is_empty(), "a second measured answer this round");
    assert_eq!(placement.evaluations(), spent);
}
