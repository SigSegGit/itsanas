//! What a stranger learns: a recovery fetch and a lookup that read the same
//! whatever exists.

use super::*;

/// THE ATTACK: ask for the recovery container of every name on a list,
/// one question each, which no per-name budget counts. `Missing` for a
/// name with nothing sorted the list into accounts that exist and hold a
/// container (the ones worth an offline guessing attack) and the rest,
/// and the client then said "no recovery container for this name" at
/// once, where a wrong passphrase took an Argon2id run to say so.
#[test]
fn red_team_a_recovery_fetch_reads_the_same_whether_the_name_exists_or_not() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("directory.redb");
    let real = itsanas_crypto::Keystore::lock(
        "a passphrase",
        "itsanas/keystore/escrow",
        &[7u8; crate::directory::RECOVERY_PAYLOAD_LEN],
        itsanas_crypto::KdfParams::RECOMMENDED,
    )
    .expect("lock")
    .to_bytes();
    let fetch = |directory: &Directory, name: &str| {
        let service = CoordService::new(directory);
        let request = Request::GetEscrow {
            username: name.to_owned(),
        };
        match ask(&service, &request, device(98).device_id()) {
            Response::Escrow(blob) => blob,
            other => panic!("{name}: a name was answered {other:?}, not a container"),
        }
    };

    let answers = {
        let directory = Directory::open(&path).expect("directory");
        enrol(&directory, &user(41), &device(41));
        enrol(&directory, &user(42), &device(42));
        let holder = directory.account_of(user(41).user_id()).unwrap().unwrap();
        directory
            .put_escrow(user(41).user_id(), &real)
            .expect("escrow");
        directory
            .set_escrow_enabled(user(41).user_id(), true)
            .expect("escrow on");
        let without = directory.account_of(user(42).user_id()).unwrap().unwrap();
        let names = [holder.username, without.username, "personne".to_owned()];
        let answers: Vec<Vec<u8>> = names.iter().map(|name| fetch(&directory, name)).collect();
        assert_eq!(
            answers[0], real,
            "the holder's own container was not handed back"
        );
        for (name, answer) in names.iter().zip(&answers).skip(1) {
            assert_eq!(
                answer.len(),
                real.len(),
                "{name}: a decoy of another size than a real container gives itself away"
            );
            assert_eq!(
                answer[..11],
                real[..11],
                "{name}: a decoy whose header (version, Argon2id cost) differs gives itself away"
            );
            let decoy = itsanas_crypto::Keystore::from_bytes(answer).expect("a container");
            assert!(
                decoy
                    .unlock("a passphrase", "itsanas/keystore/escrow")
                    .is_err(),
                "{name}: a decoy opened"
            );
            assert_eq!(
                &fetch(&directory, name),
                answer,
                "{name}: asked twice, it changed"
            );
        }
        assert_ne!(answers[1], answers[2], "two names were handed one decoy");
        (names, answers)
    };

    let reopened = Directory::open(&path).expect("reopen");
    for (name, answer) in answers.0.iter().zip(&answers.1) {
        assert_eq!(
            &fetch(&reopened, name),
            answer,
            "{name}: the answer changed across a restart, which a real container never does"
        );
    }
}

/// A `Lookup` is open to anybody with a free keypair, and exists to turn a
/// name into a key. What else an account row holds is not
/// the asker's business: when somebody joined, and whether they hold an
/// escrow blob, which `GetEscrow` also tells but counts.
#[test]
fn red_team_a_stranger_learns_nothing_from_a_lookup_but_the_key() {
    let directory = directory();
    let (joined_long_ago, joined_today) = (user(31), user(32));
    let register = |name: &str, who: &itsanas_crypto::UserKeys, at: u64| {
        directory
            .register(
                &crate::directory::Registration {
                    username: name.to_owned(),
                    user: who.public(),
                    issued_unix: at,
                }
                .sign(who),
                at,
            )
            .expect("register");
    };
    register("ancienne", &joined_long_ago, NOW - PRESENCE_KEPT_FOR);
    register("nouvelle", &joined_today, NOW);
    directory
        .put_escrow(joined_long_ago.user_id(), b"sealed")
        .expect("escrow");
    directory
        .set_escrow_enabled(joined_long_ago.user_id(), true)
        .expect("escrow on");

    let service = CoordService::new(&directory);
    let stranger = device(99).device_id();
    let lookup = |name: &str| {
        let request = Request::Lookup {
            username: name.to_owned(),
        };
        match ask(&service, &request, stranger) {
            Response::Account(account) => *account,
            other => panic!("expected the account of {name}, got {other:?}"),
        }
    };

    let found = lookup("ancienne");
    assert_eq!(
        (found.username.as_str(), found.user),
        ("ancienne", joined_long_ago.public()),
        "the key is what a lookup is for, and without it nobody can be reached by name"
    );
    assert_eq!(
        found.registered_unix, 0,
        "a stranger learnt the day an account joined, from its name alone"
    );
    assert!(
        !found.escrow_enabled,
        "a stranger can list which accounts hold an escrow blob without spending any of \
         the budget GetEscrow counts, so which names are worth a guessing attack"
    );
    let other = lookup("nouvelle");
    assert_eq!(
        (other.registered_unix, other.escrow_enabled),
        (found.registered_unix, found.escrow_enabled),
        "two accounts answer differently, and the difference is what a stranger reads"
    );

    let held = directory.account("ancienne").unwrap().unwrap();
    assert_eq!(
        (held.registered_unix, held.escrow_enabled),
        (NOW - PRESENCE_KEPT_FOR, true),
        "answering zero wrote zero: the joining allowance reads this date"
    );
    assert_eq!(
        ask(
            &service,
            &Request::GetEscrow {
                username: "ancienne".to_owned()
            },
            stranger
        ),
        Response::Escrow(b"sealed".to_vec()),
        "hiding the flag stopped the blob being handed out: recovery from nothing is gone"
    );
}
