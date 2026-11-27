//! Questions about a name: the per-address budget, the answer floor, and
//! names that cannot be taken over.

use super::*;

#[test]
fn asking_about_an_unknown_name_says_so_rather_than_inventing_one() {
    with_coordinator(|address, directory| {
        enrol_in(directory, &user(3), &device(3));
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

/// One of each request that says something about a name: the questions about
/// a name nobody holds, the registrations of `taken`, held by another key, so
/// that each is refused. What the budget and the floor must both cover.
fn name_questions(taken: &str) -> Vec<(&'static str, Request)> {
    let joiner = user(0x35);
    let registration = || {
        Box::new(
            Registration {
                username: taken.to_owned(),
                user: joiner.public(),
                issued_unix: NOW,
            }
            .sign(&joiner),
        )
    };
    vec![
        (
            "a Lookup",
            Request::Lookup {
                username: "personne".to_owned(),
            },
        ),
        (
            "a GetEscrow",
            Request::GetEscrow {
                username: "personne".to_owned(),
            },
        ),
        ("a Register", Request::Register(registration())),
        (
            "a RegisterInvited",
            Request::RegisterInvited {
                registration: registration(),
                secret: [0x5A; itsanas_coord::SECRET_LEN],
            },
        ),
    ]
}

/// Every machine of a household sits behind one address and registers its
/// own entry again and again. Counted like a question, the twenty machines an
/// account may have ran the address out of its budget in an hour, and setup
/// then failed for each new one.
#[test]
fn red_team_machines_registering_their_own_names_never_run_out() {
    let server = CoordServer::bind("127.0.0.1:0")
        .expect("bind")
        .with_name_questions(2);
    with_server(&server, |address, _| {
        let owner = user(0x37);
        let mut client = dial(address, &device(0x37), &owner);
        for round in 0..6 {
            let answer = client.ask(&Request::Register(Box::new(
                Registration {
                    username: "maison".to_owned(),
                    user: owner.public(),
                    issued_unix: NOW,
                }
                .sign(&owner),
            )));
            assert!(
                matches!(answer, Ok(Response::Account(_))),
                "registration {round} of the name's own key was refused: {answer:?}"
            );
        }
    });
}

/// THE ATTACK: one question about each name of a list, from one address,
/// reconnecting as often as it takes. No per-name budget sees it; this one
/// counts the address, and a fresh connection is the same address.
#[test]
fn red_team_one_address_cannot_draw_up_a_list_of_names() {
    let server = CoordServer::bind("127.0.0.1:0")
        .expect("bind")
        .with_name_questions(4);
    with_server(&server, |address, directory| {
        enrol_in(directory, &user(0x36), &device(0x36));
        let taken = directory
            .account_of(user(0x36).user_id())
            .unwrap()
            .unwrap()
            .username;
        let kinds = name_questions(&taken);
        let mut client = dial(address, &device(0x31), &user(0x31));
        let refused = Response::Refused(itsanas_coord::server::TOO_MANY_NAME_QUESTIONS.to_owned());
        for (what, question) in &kinds {
            assert_ne!(
                client.ask(question).unwrap(),
                refused,
                "{what}, inside the budget, was refused"
            );
        }
        for fresh in [false, true] {
            if fresh {
                client = dial(address, &device(0x32), &user(0x32));
            }
            for (what, question) in &kinds {
                assert_eq!(
                    client.ask(question).unwrap(),
                    refused,
                    "{what} was answered past the budget (fresh connection and key: {fresh}): it is not counted, and a list can be drawn up with it"
                );
            }
        }
        assert!(
            matches!(client.ask(&Request::Hosts).unwrap(), Response::Hosts(_)),
            "the name budget refused a request that is about no name"
        );
    });
}

/// THE ATTACK: time the answers. A name that exists costs a read more than
/// one that does not, and its container more than a decoy; over enough
/// samples the difference shows through any jitter. Held to a floor far above
/// that work, every answer about a name takes the same time.
#[test]
fn red_team_no_answer_about_a_name_leaves_before_the_floor() {
    with_coordinator(|address, directory| {
        enrol_in(directory, &user(0x41), &device(0x41));
        let held = directory
            .account_of(user(0x41).user_id())
            .unwrap()
            .unwrap()
            .username;
        let mut client = dial(address, &device(0x41), &user(0x41));
        let floor = itsanas_coord::server::NAME_ANSWER_FLOOR;
        let mut asked = vec![
            (
                "an account's container",
                Request::GetEscrow {
                    username: held.clone(),
                },
            ),
            (
                "an account found",
                Request::Lookup {
                    username: held.clone(),
                },
            ),
        ];
        asked.extend(name_questions(&held));
        for (what, request) in asked {
            let started = Instant::now();
            client.ask(&request).unwrap();
            let took = started.elapsed();
            assert!(
                took >= floor,
                "{what} was answered in {took:?}, under the {floor:?} every answer about a name waits: its time says what the name is"
            );
        }
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
    with_coordinator(|address, directory| {
        let nicolas = user(1);
        enrol_in(directory, &user(2), &device(1));
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
