//! Availability as hosts are seen: one network counts once, and a census
//! counts the chunks no host can make reachable.

use super::*;

#[test]
fn red_team_two_hosts_seen_from_one_network_lately_count_as_one_for_availability() {
    // §8 7c, through the store and `Placement::build`. Two accounts, each
    // missed at different sessions: independent, a chunk on both is 99 %
    // reachable. Seen lately from one /24 they are one router, and the chunk
    // is short. A network row older than `LIVE_FOR` groups nothing: the
    // address may be somebody else's by now.
    let now = now_unix();
    let me = node(0xA1, 1);
    let chunks = write(&me, "notes/one.bin", 7, 64 << 10);
    let chunk = chunks[0];
    let (a, b) = (fake(0x21), fake(0x22));
    let mut accounts = BTreeMap::new();
    for (device, missed, account) in [(a, 3, 0x31), (b, 11, 0x32)] {
        history(&me.store, &device, now, |index| index % 16 != missed);
        me.store.note_audit(&device, true).expect("audit");
        accounts.insert(device, user(account));
    }
    me.store.record_holders_at(&chunks, &a, now).unwrap();
    me.store.record_holders_at(&chunks, &b, now).unwrap();

    let apart = placement(&me, &accounts, now)
        .evaluate(&me.store, &chunk)
        .unwrap();
    assert!(
        apart.verdict.satisfied(),
        "independent: {:?}",
        apart.verdict
    );

    let one = |last: u8| -> std::net::IpAddr { format!("203.0.113.{last}").parse().unwrap() };
    let network = |ip| itsanas_placement::Network::of(ip).unwrap();
    let stale = now - itsanas_store::holders::LIVE_FOR - 1;
    me.store
        .note_network_at(&a, network(one(7)), stale)
        .unwrap();
    me.store
        .note_network_at(&b, network(one(99)), stale)
        .unwrap();
    let old = placement(&me, &accounts, now)
        .evaluate(&me.store, &chunk)
        .unwrap();
    assert!(
        old.verdict.satisfied(),
        "a network seen before LIVE_FOR grouped two hosts: {:?}",
        old.verdict
    );

    me.store.note_network_at(&a, network(one(7)), now).unwrap();
    me.store.note_network_at(&b, network(one(99)), now).unwrap();
    let together = placement(&me, &accounts, now)
        .evaluate(&me.store, &chunk)
        .unwrap();
    assert!(
        !together.verdict.durable,
        "two accounts behind one router counted as two households"
    );
    assert!(
        !together.verdict.available,
        "two hosts behind one router counted as independent: {:?}",
        together.verdict
    );
}

#[test]
fn red_team_a_census_counts_the_chunks_no_host_can_make_reachable() {
    // §8 7d. Two accounts up about half the time, at the same hours: every
    // host already holds the chunk, so nothing left can lower the miss, and
    // the census must say so instead of calling it merely "short". Adding a
    // host that is always on takes the count back to zero.
    let now = now_unix();
    let me = node(0xA1, 1);
    let chunks = write(&me, "notes/two.bin", 9, 64 << 10);
    let mut accounts = BTreeMap::new();
    for (device, account) in [(fake(0x41), 0x51), (fake(0x42), 0x52)] {
        history(&me.store, &device, now, |index| index % 2 == 0);
        me.store.note_audit(&device, true).expect("audit");
        accounts.insert(device, user(account));
        me.store.record_holders_at(&chunks, &device, now).unwrap();
    }
    let stuck = placement(&me, &accounts, now)
        .census(&me.store, 1000)
        .unwrap();
    assert_eq!(stuck.examined, chunks.len());
    assert_eq!(
        stuck.unreachable,
        chunks.len(),
        "a network that cannot reach 99 % reported nothing: {stuck:?}"
    );

    let anchor = fake(0x43);
    proven(&me.store, &anchor, now);
    accounts.insert(anchor, user(0x53));
    let helped = placement(&me, &accounts, now)
        .census(&me.store, 1000)
        .unwrap();
    assert_eq!(
        helped.unreachable, 0,
        "an always-on host is something to try: {helped:?}"
    );
}
