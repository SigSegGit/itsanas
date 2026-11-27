//! A member's ordinary path, end to end, and who may publish or list a
//! device.

use super::*;

#[test]
fn a_member_registers_enrols_a_device_and_is_then_findable_by_name() {
    // The whole point of a coordinator, end to end: after this, somebody who
    // knows only the username can reach the machines.
    with_coordinator(|address, directory| {
        let nicolas = user(1);
        let laptop = device(1);
        let mut client = dial(address, &laptop, &nicolas);

        let registration = Registration {
            username: "nicolas".to_owned(),
            user: nicolas.public(),
            issued_unix: NOW,
        }
        .sign(&nicolas);
        assert!(matches!(
            client
                .ask(&Request::Register(Box::new(registration)))
                .unwrap(),
            Response::Account(_)
        ));

        let claim = NodeClaim {
            owner: nicolas.user_id(),
            device: laptop.device_id(),
            pledged_bytes: 10 << 30,
            issued_unix: NOW,
            revoked: false,
        }
        .sign(&nicolas);
        assert!(matches!(
            client.ask(&Request::Claim(Box::new(claim))).unwrap(),
            Response::Done
        ));

        let presence = Presence {
            device: laptop.device_id(),
            address: "192.168.1.20:9797".to_owned(),
            at_unix: NOW,
            head: 0,
        }
        .sign(&laptop);
        assert!(matches!(
            client.ask(&Request::Announce(Box::new(presence))).unwrap(),
            Response::Done
        ));

        // A machine of another account, knowing only the name.
        let stranger = device(9);
        enrol_in(directory, &user(9), &stranger);
        let mut looking = dial(address, &stranger, &user(9));

        let Response::Account(account) = looking
            .ask(&Request::Lookup {
                username: "nicolas".to_owned(),
            })
            .unwrap()
        else {
            panic!("the account was not found by name");
        };
        assert_eq!(account.user.id, nicolas.user_id());

        let Response::Peers(peers) = looking
            .ask(&Request::Peers {
                user: account.user.id,
            })
            .unwrap()
        else {
            panic!("no peers returned");
        };
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].address, "192.168.1.20:9797");

        // The same answer with the laptop's own signature on it, which is what
        // lets the reader stop taking the coordinator's word for the address.
        let Response::SignedPeers(signed) = looking
            .ask(&Request::SignedPeers {
                user: account.user.id,
            })
            .unwrap()
        else {
            panic!("no signed peers returned");
        };
        assert_eq!(
            signed.iter().map(|s| &s.presence).collect::<Vec<_>>(),
            peers.iter().collect::<Vec<_>>(),
            "the signed list and the bare one disagree"
        );
        assert!(
            signed.iter().all(|s| s.verify_origin().is_ok()),
            "a presence came back with a signature its device did not make"
        );

        // And with the owner's claim on each, which is what says the laptop
        // is this account's and not merely somewhere.
        let Response::ClaimedPeers(claimed) = looking
            .ask(&Request::ClaimedPeers {
                user: account.user.id,
            })
            .unwrap()
        else {
            panic!("no claimed peers returned");
        };
        assert_eq!(
            claimed.iter().map(|c| &c.presence).collect::<Vec<_>>(),
            signed.iter().collect::<Vec<_>>(),
            "the claimed list and the signed one disagree"
        );
        assert!(
            claimed
                .iter()
                .all(|c| c.verify_for(account.user.id).is_ok()),
            "a listed device came back without a claim of this account on it"
        );
    });
}

#[test]
fn red_team_a_device_cannot_publish_an_address_for_a_device_it_does_not_own() {
    // THE ATTACK: announce a presence naming somebody else's device at an
    // address you control. Every peer looking that user up then dials you.
    // TLS pinning refuses the connection, so no data is exposed — but the
    // victim's machines become unreachable through the coordinator, which is a
    // denial of service that costs the attacker one packet per victim.
    //
    // If this test fails, anyone who can reach the coordinator can black-hole
    // any member's devices.
    with_coordinator(|address, _| {
        let victim = device(1);
        let attacker = device(2);
        let mut client = dial(address, &attacker, &user(2));

        // Correctly signed by the victim's key — the attacker is assumed to
        // have obtained a stale, validly signed presence, which is the
        // strongest version of this attack.
        let forged = Presence {
            device: victim.device_id(),
            address: "10.0.0.66:9797".to_owned(),
            at_unix: NOW,
            head: 0,
        }
        .sign(&victim);

        assert!(
            matches!(
                client.ask(&Request::Announce(Box::new(forged))).unwrap(),
                Response::Refused(_)
            ),
            "a device published an address for another device"
        );
    });
}

#[test]
fn a_member_s_device_list_includes_a_machine_that_has_gone_quiet() {
    // `device list` was built on `Peers`, which answers "where can I dial" and
    // so leaves out anything that has not announced within the presence
    // window. The machine somebody opens the list to find -- the laptop lost a
    // fortnight ago, still enrolled and still counted -- was the one it could
    // not show. A device enrolled and never announced stands in for it here.
    with_coordinator(|address, _| {
        let nicolas = user(1);
        let laptop = device(1);
        let quiet = device(2);
        let withdrawn = device(3);
        let mut client = enrolled_member(address, "nicolas", &nicolas, &laptop);
        claim_device(&mut client, &nicolas, &quiet, false, NOW);
        claim_device(&mut client, &nicolas, &withdrawn, false, NOW);
        claim_device(&mut client, &nicolas, &withdrawn, true, NOW + 1);

        let presence = Presence {
            device: laptop.device_id(),
            address: "192.168.1.20:9797".to_owned(),
            at_unix: NOW,
            head: 0,
        }
        .sign(&laptop);
        assert!(matches!(
            client.ask(&Request::Announce(Box::new(presence))).unwrap(),
            Response::Done
        ));

        let Response::Peers(reachable) = client
            .ask(&Request::Peers {
                user: nicolas.user_id(),
            })
            .unwrap()
        else {
            panic!("no peer list");
        };
        assert_eq!(reachable.len(), 1, "the quiet machine is not a dial target");

        let Response::Devices(listed) = client
            .ask(&Request::Devices {
                user: nicolas.user_id(),
            })
            .unwrap()
        else {
            panic!("no device list");
        };
        let ids: Vec<_> = listed.iter().map(|entry| entry.device).collect();
        assert_eq!(
            ids,
            vec![laptop.device_id(), quiet.device_id()],
            "the list must hold every live enrolment, heard-from first, and no withdrawn one"
        );
        assert_eq!(listed[0].address.as_deref(), Some("192.168.1.20:9797"));
        assert!(listed[0].silent_for.is_some());
        assert_eq!(
            (listed[1].silent_for, listed[1].address.as_deref()),
            (None, None),
            "a device that never announced must read as never heard from, not as fresh"
        );
        assert_eq!(listed[1].pledged_bytes, 3 << 30);
    });
}

#[test]
fn red_team_a_stranger_cannot_list_another_member_s_devices() {
    // THE ATTACK: a user id is public -- `Lookup` hands it to anybody who knows
    // a name. The device list says how much each machine pledges and how long
    // each has been silent, which tells a burglar of a different kind which
    // household's NAS has been off for a month. `Peers` never had to say that,
    // so the new message must not become a way to ask it of anyone.
    with_coordinator(|address, _| {
        let nicolas = user(1);
        let _victim = enrolled_member(address, "nicolas", &nicolas, &device(1));

        let mallory = user(9);
        let mut member = enrolled_member(address, "mallory", &mallory, &device(9));
        let answer = member
            .ask(&Request::Devices {
                user: nicolas.user_id(),
            })
            .unwrap();
        assert!(
            matches!(answer, Response::Refused(_)),
            "another member listed nicolas's devices: {answer:?}"
        );

        let mut stranger = dial(address, &device(0xEE), &user(0xEE));
        let answer = stranger
            .ask(&Request::Devices {
                user: nicolas.user_id(),
            })
            .unwrap();
        assert!(
            matches!(answer, Response::Refused(_)),
            "an unenrolled keypair listed nicolas's devices: {answer:?}"
        );
    });
}

#[test]
fn the_peer_list_is_bounded_however_many_devices_a_user_enrols() {
    // A member with a thousand devices must not be a way to make the
    // coordinator send a thousand records to anybody who asks. Since
    // 2026-09-30 nobody can enrol more than `MAX_DEVICES_PER_ACCOUNT`, so this
    // plays a coordinator older than that bound: an account enrolled then
    // keeps its forty devices, and the list cap is what still protects askers.
    with_coordinator(|address, directory| {
        directory.bound_devices(false);
        let nicolas = user(1);
        let first = device(1);
        let mut client = dial(address, &first, &nicolas);
        client
            .ask(&Request::Register(Box::new(
                Registration {
                    username: "nicolas".to_owned(),
                    user: nicolas.public(),
                    issued_unix: NOW,
                }
                .sign(&nicolas),
            )))
            .unwrap();

        // One connection per device, carrying both its enrolment and its
        // address: a device may only announce itself, and a connection may only
        // make so many requests, so batching all forty onto one would exceed
        // the per-connection cap — which is the server behaving correctly.
        for seed in 1..=40u8 {
            let extra = device(seed);
            let mut per_device = dial(address, &extra, &nicolas);
            per_device
                .ask(&Request::Claim(Box::new(
                    NodeClaim {
                        owner: nicolas.user_id(),
                        device: extra.device_id(),
                        pledged_bytes: 1 << 30,
                        issued_unix: NOW,
                        revoked: false,
                    }
                    .sign(&nicolas),
                )))
                .unwrap();
            per_device
                .ask(&Request::Announce(Box::new(
                    Presence {
                        device: extra.device_id(),
                        address: format!("10.1.0.{seed}:9797"),
                        at_unix: NOW,
                        head: 0,
                    }
                    .sign(&extra),
                )))
                .unwrap();
        }

        let mut asking = dial(address, &device(0xF1), &user(0xF1));
        let Response::Peers(peers) = asking
            .ask(&Request::Peers {
                user: nicolas.user_id(),
            })
            .unwrap()
        else {
            panic!("no peers");
        };
        assert!(
            peers.len() <= MAX_PEERS_RETURNED,
            "{} peers returned; the cap is {MAX_PEERS_RETURNED}",
            peers.len()
        );
    });
}
