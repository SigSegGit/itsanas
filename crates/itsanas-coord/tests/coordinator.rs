//! A real coordinator on a real socket.
//!
//! Everything below the address is production: a real directory, real TLS with
//! device authentication, real signatures, real framing. The only thing these
//! tests simulate is which machine each side runs on.

use std::sync::atomic::{AtomicBool, Ordering};

use itsanas_coord::claim::{NodeClaim, Presence};
use itsanas_coord::directory::Registration;
use itsanas_coord::protocol::{MAX_PEERS_RETURNED, Request, Response};
use itsanas_coord::server::{CoordClient, CoordServer};
use itsanas_coord::{COORD_VERSION, Directory};
use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};

const NOW: u64 = 1_700_000_000;

fn user(seed: u8) -> UserKeys {
    UserKeys::derive(&MasterSecret::from_bytes([seed; 32]))
}

fn device(seed: u8) -> DeviceKeys {
    DeviceKeys::from_seed(&SecretBytes::new([seed; 32]))
}

/// Stops the server even when the body panics.
///
/// Without it a failing assertion leaves the accept loop running,
/// `thread::scope` never returns, and the harness reports a hang instead of the
/// assertion — which is how a one-line failure costs ten minutes.
struct StopOnDrop<'a>(&'a AtomicBool, std::net::SocketAddr);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.1);
    }
}

/// Run `body` against a coordinator listening on loopback.
fn with_coordinator<T>(body: impl FnOnce(std::net::SocketAddr, &Directory) -> T) -> T {
    let dir = tempfile::tempdir().expect("temp dir");
    let directory = Directory::open(dir.path().join("directory.redb")).expect("directory");
    let server = CoordServer::bind("127.0.0.1:0").expect("bind");
    let address = server.local_addr().expect("address");
    let shutdown = AtomicBool::new(false);
    let coordinator = device(0xC0);

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.serve_until(&directory, &coordinator, &shutdown, |_| {});
        });

        let _stop = StopOnDrop(&shutdown, address);
        body(address, &directory)
    })
}

fn dial(address: std::net::SocketAddr, keys: &DeviceKeys, owner: &UserKeys) -> CoordClient {
    CoordClient::connect(address, keys, owner.user_id(), None).expect("dial the coordinator")
}

// ---------------------------------------------------------------------------
// The ordinary paths
// ---------------------------------------------------------------------------

#[test]
fn a_member_registers_enrols_a_device_and_is_then_findable_by_name() {
    // The whole point of a coordinator, end to end: after this, somebody who
    // knows only the username can reach the machines.
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
        }
        .sign(&laptop);
        assert!(matches!(
            client.ask(&Request::Announce(Box::new(presence))).unwrap(),
            Response::Done
        ));

        // A different machine, knowing only the name.
        let stranger = device(9);
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
    });
}

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

        assert!(
            matches!(ask(&mut client), Response::Missing),
            "an account had escrow before anybody asked for it"
        );

        client
            .ask(&Request::PutEscrow {
                blob: Some(b"sealed".to_vec()),
            })
            .unwrap();
        assert!(matches!(ask(&mut client), Response::Escrow(_)));

        client.ask(&Request::PutEscrow { blob: None }).unwrap();
        assert!(
            matches!(ask(&mut client), Response::Missing),
            "a withdrawn escrow blob was still served"
        );
    });
}

#[test]
fn a_version_mismatch_is_refused_rather_than_guessed_at() {
    with_coordinator(|address, _| {
        let mut client = dial(address, &device(2), &user(2));
        assert!(matches!(
            client
                .ask(&Request::Hello {
                    version: COORD_VERSION + 1
                })
                .unwrap(),
            Response::Refused(_)
        ));
    });
}

#[test]
fn asking_about_an_unknown_name_says_so_rather_than_inventing_one() {
    with_coordinator(|address, _| {
        let mut client = dial(address, &device(3), &user(3));
        assert!(matches!(
            client
                .ask(&Request::Lookup {
                    username: "nobody".to_owned()
                })
                .unwrap(),
            Response::Missing
        ));
    });
}

// ---------------------------------------------------------------------------
// Red team
// ---------------------------------------------------------------------------

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

#[test]
fn red_team_an_oversized_username_is_refused_before_the_directory_sees_it() {
    // THE ATTACK: hand the directory a megabyte where it expects a name, and
    // see what it does with it. The wire limit exists so nothing downstream has
    // to be robust against a caller-chosen length.
    with_coordinator(|address, _| {
        let mut client = dial(address, &device(4), &user(4));
        let long = "n".repeat(10_000);

        assert!(matches!(
            client
                .ask(&Request::Lookup {
                    username: long.clone()
                })
                .unwrap(),
            Response::Refused(_)
        ));
        assert!(matches!(
            client.ask(&Request::GetEscrow { username: long }).unwrap(),
            Response::Refused(_)
        ));
    });
}

#[test]
fn red_team_a_name_cannot_be_taken_over_by_a_different_key() {
    // THE ATTACK: register somebody else's username to your own key, so that
    // everyone looking them up reaches you instead. The registration is signed
    // by the user's key, so this is the coordinator's job to refuse.
    with_coordinator(|address, _| {
        let nicolas = user(1);
        let mut honest = dial(address, &device(1), &nicolas);
        honest
            .ask(&Request::Register(Box::new(
                Registration {
                    username: "nicolas".to_owned(),
                    user: nicolas.public(),
                    issued_unix: NOW,
                }
                .sign(&nicolas),
            )))
            .unwrap();

        let impostor = user(0xEE);
        let mut hostile = dial(address, &device(0xEE), &impostor);
        assert!(
            matches!(
                hostile
                    .ask(&Request::Register(Box::new(
                        Registration {
                            username: "nicolas".to_owned(),
                            user: impostor.public(),
                            issued_unix: NOW,
                        }
                        .sign(&impostor),
                    )))
                    .unwrap(),
                Response::Refused(_)
            ),
            "a username was taken over by a different key"
        );

        let Response::Account(account) = honest
            .ask(&Request::Lookup {
                username: "nicolas".to_owned(),
            })
            .unwrap()
        else {
            panic!("the account vanished");
        };
        assert_eq!(
            account.user.id,
            nicolas.user_id(),
            "the name now points at the impostor"
        );
    });
}

#[test]
fn a_connection_that_asks_too_much_is_told_why_rather_than_cut_off() {
    // A silent close surfaces as "connection aborted by your host software",
    // which reads like a firewall. Somebody debugging that looks at their
    // network for an hour before looking at the protocol.
    with_coordinator(|address, _| {
        let mut client = dial(address, &device(7), &user(7));
        let mut last = Response::Done;
        for _ in 0..40 {
            match client.ask(&Request::Lookup {
                username: "nobody".to_owned(),
            }) {
                Ok(response) => last = response,
                Err(_) => break,
            }
        }
        assert!(
            matches!(last, Response::Refused(_)),
            "the cap was hit without the caller being told, got {last:?}"
        );
    });
}

#[test]
fn the_peer_list_is_bounded_however_many_devices_a_user_enrols() {
    // A member with a thousand devices must not be a way to make the
    // coordinator send a thousand records to anybody who asks.
    with_coordinator(|address, _| {
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
