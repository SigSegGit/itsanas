//! Escrow over the wire: stored by an enrolled device, recovered by name
//! alone, and a budget a reconnection does not reset.

use super::*;

#[test]
fn escrow_is_stored_by_an_enrolled_device_and_recovered_by_name_alone() {
    // MVP acceptance test D, at the protocol layer: a machine with no identity
    // at all fetches the sealed container using only the account name. The
    // passphrase is what protects it, and the coordinator never sees one.
    with_coordinator(|address, _| {
        let nicolas = user(1);
        let laptop = device(1);
        let mut client = dial(address, &laptop, &nicolas);

        let registration = Registration {
            username: "nicolas".to_owned(),
            user: nicolas.public(),
            issued_unix: NOW,
        }
        .sign(&nicolas);
        client
            .ask(&Request::Register(Box::new(registration)))
            .unwrap();
        let claim = NodeClaim {
            owner: nicolas.user_id(),
            device: laptop.device_id(),
            pledged_bytes: 1 << 30,
            issued_unix: NOW,
            revoked: false,
        }
        .sign(&nicolas);
        client.ask(&Request::Claim(Box::new(claim))).unwrap();

        let blob = b"a passphrase-sealed container the coordinator cannot open".to_vec();
        assert!(matches!(
            client
                .ask(&Request::PutEscrow {
                    blob: Some(blob.clone())
                })
                .unwrap(),
            Response::Done
        ));

        // A brand-new machine: fresh device key, no account, no claim.
        let fresh = device(0x5F);
        let mut recovering = dial(address, &fresh, &user(0x5F));
        let Response::Escrow(recovered) = recovering
            .ask(&Request::GetEscrow {
                username: "nicolas".to_owned(),
            })
            .unwrap()
        else {
            panic!("a fresh machine could not fetch the escrow blob");
        };
        assert_eq!(recovered, blob);
    });
}

#[test]
fn escrow_is_off_until_a_blob_is_stored_and_can_be_withdrawn_again() {
    // Passphrase recovery is a trade: it makes the passphrase the weakest link
    // for anyone who steals the coordinator's database. A member must be able
    // to decline it, and to change their mind in both directions — otherwise
    // the only safe choice is never to use it.
    with_coordinator(|address, _| {
        let nicolas = user(1);
        let laptop = device(1);
        let mut client = dial(address, &laptop, &nicolas);
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
        client
            .ask(&Request::Claim(Box::new(
                NodeClaim {
                    owner: nicolas.user_id(),
                    device: laptop.device_id(),
                    pledged_bytes: 1 << 30,
                    issued_unix: NOW,
                    revoked: false,
                }
                .sign(&nicolas),
            )))
            .unwrap();

        let ask = |client: &mut CoordClient| {
            client
                .ask(&Request::GetEscrow {
                    username: "nicolas".to_owned(),
                })
                .unwrap()
        };

        // Before, the answer is a decoy: never `Missing`, which would tell a
        // stranger the account holds nothing worth guessing at.
        let before = ask(&mut client);
        assert!(
            matches!(&before, Response::Escrow(blob) if blob.as_slice() != b"sealed"),
            "an account had escrow before anybody asked for it: {before:?}"
        );

        client
            .ask(&Request::PutEscrow {
                blob: Some(b"sealed".to_vec()),
            })
            .unwrap();
        assert_eq!(ask(&mut client), Response::Escrow(b"sealed".to_vec()));

        client.ask(&Request::PutEscrow { blob: None }).unwrap();
        assert_eq!(
            ask(&mut client),
            before,
            "a withdrawn escrow blob was still served, or the decoy changed"
        );
    });
}

#[test]
fn red_team_an_unenrolled_device_cannot_overwrite_someone_elses_escrow() {
    // THE ATTACK: replace a member's escrow blob with one whose passphrase you
    // chose. The member then "recovers" into an account you control, or —
    // worse and quieter — loses the ability to recover at all.
    with_coordinator(|address, _| {
        let nicolas = user(1);
        let laptop = device(1);
        let mut owner_client = dial(address, &laptop, &nicolas);

        let registration = Registration {
            username: "nicolas".to_owned(),
            user: nicolas.public(),
            issued_unix: NOW,
        }
        .sign(&nicolas);
        owner_client
            .ask(&Request::Register(Box::new(registration)))
            .unwrap();
        let claim = NodeClaim {
            owner: nicolas.user_id(),
            device: laptop.device_id(),
            pledged_bytes: 1 << 30,
            issued_unix: NOW,
            revoked: false,
        }
        .sign(&nicolas);
        owner_client.ask(&Request::Claim(Box::new(claim))).unwrap();
        owner_client
            .ask(&Request::PutEscrow {
                blob: Some(b"the real container".to_vec()),
            })
            .unwrap();

        let attacker = device(0xAA);
        let mut hostile = dial(address, &attacker, &user(0xAA));
        assert!(
            matches!(
                hostile
                    .ask(&Request::PutEscrow {
                        blob: Some(b"a container whose passphrase I chose".to_vec())
                    })
                    .unwrap(),
                Response::Refused(_)
            ),
            "an unenrolled device wrote an escrow blob"
        );

        let mut checking = dial(address, &device(0xBB), &user(0xBB));
        let Response::Escrow(blob) = checking
            .ask(&Request::GetEscrow {
                username: "nicolas".to_owned(),
            })
            .unwrap()
        else {
            panic!("the real blob disappeared");
        };
        assert_eq!(blob, b"the real container");
    });
}

#[test]
fn red_team_reconnecting_does_not_reset_the_escrow_attempt_budget() {
    // THE ATTACK: the escrow blob is the one thing reachable without proving
    // anything, so the rate limit is the only defence. If the counter lived per
    // connection, an attacker would simply reconnect — a handshake per attempt,
    // and the passphrase falls to any word list.
    //
    // If this test fails, recovery-by-passphrase is an account takeover waiting
    // for someone with a list.
    with_coordinator(|address, _| {
        let nicolas = user(1);
        let laptop = device(1);
        let mut client = dial(address, &laptop, &nicolas);
        let registration = Registration {
            username: "nicolas".to_owned(),
            user: nicolas.public(),
            issued_unix: NOW,
        }
        .sign(&nicolas);
        client
            .ask(&Request::Register(Box::new(registration)))
            .unwrap();
        let claim = NodeClaim {
            owner: nicolas.user_id(),
            device: laptop.device_id(),
            pledged_bytes: 1 << 30,
            issued_unix: NOW,
            revoked: false,
        }
        .sign(&nicolas);
        client.ask(&Request::Claim(Box::new(claim))).unwrap();
        client
            .ask(&Request::PutEscrow {
                blob: Some(b"sealed".to_vec()),
            })
            .unwrap();

        // Each attempt on its own fresh connection, with a fresh device key —
        // the cheapest thing an attacker can do.
        let mut refusals = 0;
        for attempt in 0..12u8 {
            let mut grinding = dial(address, &device(0xE0 + attempt), &user(0xE0 + attempt));
            if matches!(
                grinding
                    .ask(&Request::GetEscrow {
                        username: "nicolas".to_owned()
                    })
                    .unwrap(),
                Response::Refused(_)
            ) {
                refusals += 1;
            }
        }

        assert!(
            refusals > 0,
            "twelve fetches from twelve fresh connections were all allowed; \
             the rate limit is per-connection and therefore no limit at all"
        );
    });
}
