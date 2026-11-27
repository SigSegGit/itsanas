//! What this machine is given: the vault's disk, the updates choice, a
//! phone's setup, and the space kept.

use super::*;

#[test]
fn red_team_setup_puts_the_vault_on_the_disk_chosen_and_never_strands_one_that_holds_data() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let chosen = dir.path().join("big-disk").join("for-others");
    let (mut person, _) = Scripted::new(false);
    let with_vault = Answers {
        vault: Some(chosen.clone()),
        ..answers(dir.path())
    };
    let (events, failed) = run(&home, with_vault, &mut person);
    assert_eq!(
        failed, None,
        "setup with a vault folder failed: {events:#?}"
    );
    let vault_of = |home: &Path| {
        crate::config::Config::load(&Node::config_path(home))
            .expect("config")
            .vault
    };
    assert_eq!(
        vault_of(&home),
        Some(chosen.clone()),
        "the person chose a bigger disk for other people's copies and they still fill the home's"
    );
    assert!(
        chosen.join(itsanas_node::node::VAULT_MARKER).exists(),
        "the chosen vault was not marked as this node's: a later start refuses it as a stranger's"
    );

    // Someone's copy is now held there; setup run again with another place
    // must not walk away from it.
    let owner =
        itsanas_crypto::UserKeys::derive(&itsanas_crypto::MasterSecret::from_bytes([9; 32]))
            .user_id();
    itsanas_store::Vault::open(&chosen)
        .expect("vault")
        .put_chunk(
            owner,
            &itsanas_crypto::ChunkId::from_bytes([4; 32]),
            b"someone's sealed chunk",
        )
        .expect("store");
    let elsewhere = dir.path().join("other-disk");
    let (mut again, _) = Scripted::new(false);
    let moved = Answers {
        vault: Some(elsewhere.clone()),
        ..answers(dir.path())
    };
    let (_, failed) = run(&home, moved, &mut again);
    assert_eq!(
        failed,
        Some(Step::Pledge),
        "setup switched away from a vault that holds someone's copy: the copy is stranded and its owner's audits fail"
    );
    assert_eq!(
        vault_of(&home),
        Some(chosen),
        "a refused switch still rewrote the vault"
    );
    assert!(
        !elsewhere.exists(),
        "a refused switch still made the new vault"
    );
}

#[test]
fn the_updates_choice_is_written_and_a_later_run_without_it_keeps_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let (mut person, _) = Scripted::new(false);
    let chosen = Answers {
        updates: Some(crate::config::Updates::Auto),
        ..answers(dir.path())
    };
    let (events, failed) = run(&home, chosen, &mut person);
    assert_eq!(
        failed, None,
        "setup with an updates choice failed: {events:#?}"
    );
    let updates = |home: &Path| {
        crate::config::Config::load(&Node::config_path(home))
            .expect("config")
            .updates
    };
    assert_eq!(
        updates(&home),
        crate::config::Updates::Auto,
        "the member chose automatic updates and the daemon would only notify"
    );

    let (mut again, _) = Scripted::new(false);
    let (_, failed) = run(&home, answers(dir.path()), &mut again);
    assert_eq!(failed, None);
    assert_eq!(
        updates(&home),
        crate::config::Updates::Auto,
        "running setup again without the question undid the member's choice"
    );

    let parsed = super::answers::parse("updates = \"off\"\n", dir.path()).expect("parse");
    assert_eq!(parsed.updates, Some(crate::config::Updates::Off));
    assert!(
        super::answers::parse("updates = \"sometimes\"\n", dir.path()).is_err(),
        "a mistyped updates answer was taken as the default instead of refused"
    );
}

#[test]
fn red_team_a_phone_is_never_given_a_synced_folder_an_updater_or_a_listener_to_dial() {
    // A phone's files are in the app: a folder made for it would hold the same
    // files twice on a disk that is small; the app updates itself; and it
    // dials out only, so a dial-back would fail by design and say nothing.
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let mut wanted = answers(dir.path());
    wanted.phone = true;
    let folder = wanted.folder.clone().expect("a folder was given");
    let (mut person, _) = Scripted::new(false);
    let (events, failed) = run(&home, wanted, &mut person);
    assert_eq!(failed, None, "{events:#?}");
    for skipped in [Step::Machine, Step::Folder, Step::Updates] {
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::Skipped { step, .. } if *step == skipped)),
            "{skipped:?} was not skipped on a phone: {events:#?}"
        );
    }
    assert!(
        !folder.exists(),
        "a phone got a synced folder: its files would sit twice on a disk that has no room \
         for them"
    );
    assert!(
        Node::exists(&home),
        "the account was not made: the skips swallowed more than they should"
    );
}

#[test]
fn red_team_a_phone_finishes_setup_with_no_daemon_to_wait_for() {
    // The app is the phone's service and opens the node once setup is over, so
    // nothing holds the store during the service step. Waiting for a daemon
    // there failed every first run on every phone, after thirty seconds.
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let mut wanted = answers(dir.path());
    wanted.phone = true;
    wanted.service = true;
    let (mut person, _) = Scripted::new(false);
    let service = Remembering {
        file: home.with_extension("passphrase"),
        kept: RefCell::new(None),
    };
    let mut events = Vec::new();
    let mut record = |event: &Event| events.push(event.clone());
    let outcome = Setup::new(&home, wanted, &mut person, &service, &mut record).run();
    assert_eq!(
        outcome.failed, None,
        "setup stopped on a phone, so nobody can make an account there: {events:#?}"
    );
}

/// A keep is judged against what the machine earns: a computer on its own pledge,
/// a phone on the account's whole (the owner's other machines lend too).
/// Sabotage: drop the `others` in `apply_pledge`; drop the keep from the pair.
#[test]
fn red_team_a_keep_above_what_is_earned_is_refused_and_a_phone_earns_on_the_whole_account() {
    const GIB: u64 = 1 << 30;
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let (mut person, _) = Scripted::new(false);
    let (events, failed) = run(&home, answers(dir.path()), &mut person);
    assert_eq!(failed, None, "{events:#?}");
    Node::open(&home, PASSPHRASE)
        .expect("open")
        .remember_others_pledged(100 * GIB)
        .expect("remember");

    let attempt = |phone: bool, keep: Keep| {
        let mut wanted = answers(dir.path());
        wanted.account = None;
        wanted.phone = phone;
        wanted.pledge = Some(0);
        wanted.keep = keep;
        let (mut person, _) = Scripted::new(false);
        run(&home, wanted, &mut person).1
    };
    let kept = || {
        crate::config::Config::load(&Node::config_path(&home))
            .expect("config")
            .keep_bytes
    };

    // 30 / 70 of 100 GiB is 42 and some: 43 is more than the account earns.
    assert_eq!(attempt(true, Keep::Budget(43 * GIB)), Some(Step::Pledge));
    assert_eq!(kept(), None, "a refused keep was written all the same");
    assert_eq!(attempt(true, Keep::Budget(42 * GIB)), None);
    assert_eq!(
        kept(),
        Some(42 * GIB),
        "the keep a phone earns was not written"
    );
    // A computer earns on its own pledge, which is 0 here: the joining allowance
    // (10 GiB) and nothing of what the account's other machines lend.
    assert_eq!(
        attempt(false, Keep::Budget(11 * GIB)),
        Some(Step::Pledge),
        "a computer kept what only the account's other machines earn"
    );
    assert_eq!(kept(), Some(42 * GIB));
    // No limit, and nothing asked: both leave the machine as it can be.
    assert_eq!(attempt(true, Keep::Everything), None);
    assert_eq!(kept(), None, "no limit was not written");
}
