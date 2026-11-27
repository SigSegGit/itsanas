//! Peers and hosts: what one lookup costs across a fleet, and the hosts
//! list showing only other accounts' live offering machines.

use super::*;

/// What one address lookup costs when the coordinator holds a real fleet.
///
/// `#[ignore]`d because it builds a directory of three thousand devices and
/// is a measurement rather than an assertion -- run it with
/// `cargo test -p itsanas-coord -- --ignored --nocapture` and read the
/// numbers.
///
/// **Why it exists.** `peers_of` and `devices_of` walk `live_claims()`,
/// which deserialises **every claim in the directory** and then filters by
/// account. The cost of one member asking "where are my machines" is
/// therefore O(devices in the whole network), not O(devices of that
/// account) -- so the work grows with the square of the fleet. At the scale
/// Nicolas asked about, a thousand members and three thousand machines,
/// that is the wall, and halving the number of *connections* per round does
/// not touch it. Found by the Rodin audit of 2026-09-18.
///
/// This test does not assert a time. It prints one, so that the decision to
/// index by account is made against a number instead of an intuition.
#[test]
#[ignore = "a measurement over three thousand devices, not an assertion"]
fn measure_what_one_lookup_costs_across_a_fleet() {
    let directory = directory();
    let mine = user(1);
    let my_laptop = device(1);
    enrol(&directory, &mine, &my_laptop);

    let service = CoordService::new(&directory);
    let mut sizes = Vec::new();

    for fleet in [0usize, 500, 1500, 3000] {
        while sizes.iter().sum::<usize>() < fleet {
            let n = sizes.len();
            // A separate account per device: the shape that matters is the
            // number of claims in the table, whoever owns them.
            let owner = user(u8::try_from(n % 200).unwrap_or(0).wrapping_add(20));
            let machine = itsanas_crypto::DeviceKeys::from_seed(&itsanas_crypto::SecretBytes::new(
                seed_for(n),
            ));
            let _ = directory.register(
                &crate::directory::Registration {
                    username: format!("member{n}"),
                    user: owner.public(),
                    issued_unix: NOW,
                }
                .sign(&owner),
                NOW,
            );
            let _ = directory.claim(
                &NodeClaim {
                    owner: owner.user_id(),
                    device: machine.device_id(),
                    pledged_bytes: 1 << 30,
                    issued_unix: NOW,
                    revoked: false,
                }
                .sign(&owner),
                NOW,
            );
            sizes.push(1);
        }

        let started = Instant::now();
        let rounds = 20;
        for _ in 0..rounds {
            let _ = service
                .peers_of(mine.user_id(), NOW)
                .expect("the lookup answers");
        }
        let each = started.elapsed() / rounds;
        println!(
            "  {:>5} devices in the directory -> {each:?} per lookup, \
             {:.0} lookups/s",
            sizes.iter().sum::<usize>(),
            1.0 / each.as_secs_f64()
        );
    }
    println!("  a fleet of 3000 machines on a 300 s round asks about 10 lookups/s");
}

/// Thirty-two bits of variation in a device seed, which is enough for a
/// measurement and keeps the fixture deterministic.
fn seed_for(n: usize) -> [u8; 32] {
    let mut seed = [0u8; 32];
    seed[..8].copy_from_slice(&(n as u64).to_le_bytes());
    seed[31] = 0xA5;
    seed
}

/// A member is introduced to machines of other accounts that offer space
/// and were heard lately, and to nothing else. The client filters some of
/// this again, so only a test on this side sees the coordinator's own list.
/// Sabotage: drop each condition of `hosts_for`.
#[test]
fn red_team_hosts_lists_only_another_accounts_live_offering_machines() {
    let directory = directory();
    let me = user(30);
    let my_phone = device(30);
    enrol(&directory, &me, &my_phone);
    let my_other = device(31);
    directory
        .claim(
            &NodeClaim {
                owner: me.user_id(),
                device: my_other.device_id(),
                pledged_bytes: 1 << 30,
                issued_unix: NOW,
                revoked: false,
            }
            .sign(&me),
            NOW,
        )
        .expect("claim");

    let host_owner = user(32);
    let host = device(32);
    enrol(&directory, &host_owner, &host);

    let withdraw = |owner: &itsanas_crypto::UserKeys,
                    machine: &itsanas_crypto::DeviceKeys,
                    pledged_bytes,
                    revoked| {
        directory
            .claim(
                &NodeClaim {
                    owner: owner.user_id(),
                    device: machine.device_id(),
                    pledged_bytes,
                    issued_unix: NOW + 1,
                    revoked,
                }
                .sign(owner),
                NOW + 1,
            )
            .expect("claim again");
    };
    let gone_owner = user(33);
    let gone = device(33);
    enrol(&directory, &gone_owner, &gone);
    let idle_owner = user(34);
    let idle = device(34);
    enrol(&directory, &idle_owner, &idle);
    let banned_owner = user(35);
    let banned = device(35);
    enrol(&directory, &banned_owner, &banned);

    for (owner, machine, address) in [
        (&me, &my_phone, "203.0.113.1:9797"),
        (&me, &my_other, "203.0.113.2:9797"),
        (&host_owner, &host, "203.0.113.3:9797"),
        (&gone_owner, &gone, "203.0.113.4:9797"),
        (&idle_owner, &idle, "203.0.113.5:9797"),
        (&banned_owner, &banned, "203.0.113.6:9797"),
    ] {
        announce_address(&directory, owner, machine, address);
    }
    // After announcing: their addresses are still in the table. Withdrawn
    // with its pledge still on the claim, so only `revoked` says so.
    withdraw(&gone_owner, &gone, 1 << 30, true);
    withdraw(&idle_owner, &idle, 0, false);
    directory
        .suspend(banned_owner.user_id(), NOW)
        .expect("suspend");

    let service = CoordService::new(&directory);
    let told = |now| match ask_at(&service, &Request::Hosts, my_phone.device_id(), now) {
        Response::Hosts(list) => list
            .into_iter()
            .map(|row| row.presence.presence.device)
            .collect::<Vec<_>>(),
        other => panic!("not a list of hosts: {other:?}"),
    };
    assert_eq!(
        told(NOW + PRESENCE_TTL),
        vec![host.device_id()],
        "only another account's machine offering space may be introduced"
    );
    assert_eq!(
        told(NOW + PRESENCE_TTL + 1),
        Vec::new(),
        "a machine silent past the presence window was introduced"
    );
}
