//! The account: a second run leaves it alone, no word is ever said, and a
//! name, a coordinator or words that are not this account's are refused
//! before any key is written.

use super::*;

#[test]
fn red_team_setup_run_again_never_remakes_the_account_or_touches_the_keystore() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let (mut first, _) = Scripted::new(false);
    let (events, failed) = run(&home, answers(dir.path()), &mut first);
    assert_eq!(
        failed, None,
        "a first setup on an empty home failed: {events:#?}"
    );
    let sealed = std::fs::read(keystore(&home)).expect("keystore");
    let device = Node::open(&home, PASSPHRASE)
        .expect("open")
        .store
        .device_id();

    let (mut second, asked) = Scripted::new(false);
    let (events, failed) = run(&home, answers(dir.path()), &mut second);
    assert_eq!(
        failed, None,
        "setup run again on a set-up machine failed: {events:#?}"
    );
    assert_eq!(
        asked.borrow().questions,
        0,
        "setup run again asked for a secret on a machine where everything was done: a person \
         would be shown new recovery words for an account that already has some"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::AlreadyDone {
                step: Step::Account,
                ..
            }
        )),
        "the account was not found done on the second run: {events:#?}"
    );
    assert_eq!(
        std::fs::read(keystore(&home)).expect("keystore"),
        sealed,
        "setup run again rewrote the keystore: the only copy of this machine's keys was \
         replaced, and the words on paper may no longer open it"
    );
    assert_eq!(
        Node::open(&home, PASSPHRASE)
            .expect("open")
            .store
            .device_id(),
        device,
        "setup run again gave this machine a second identity"
    );

    // Another account asked for is refused, not made beside the first.
    let mut other = answers(dir.path());
    other.account = Some(Account::New {
        username: "somebody-else".to_owned(),
    });
    let (mut third, _) = Scripted::new(false);
    let (_, failed) = run(&home, other, &mut third);
    assert_eq!(
        failed,
        Some(Step::Account),
        "setup asked for another account on a set-up machine did not stop at the account"
    );
    assert_eq!(
        std::fs::read(keystore(&home)).expect("keystore"),
        sealed,
        "asking for another account overwrote the keys of the one this machine has"
    );
}

#[test]
fn red_team_no_event_and_no_log_line_carries_a_recovery_word() {
    // Two accounts, so a word that is in the engine's own sentences ("account"
    // and "machine" are recovery words too) is told from a leaked one: only a
    // word of *this* run's phrase that the *other* run never said is a leak.
    let dir = tempfile::tempdir().expect("tempdir");
    let mut runs = Vec::new();
    // Not "one"/"two": both are recovery words, and a path printed with the
    // home in it read as a leak whenever the phrase drew that word (1 in ~85).
    for name in ["hqa", "hqb"] {
        let home = dir.path().join(name);
        let mut answers = answers(&dir.path().join(format!("{name}-base")));
        answers.folder = Some(dir.path().join(format!("{name}-folder")));
        let (mut person, asked) = Scripted::new(false);
        let (events, failed) = run(&home, answers, &mut person);
        assert_eq!(failed, None, "setup failed: {events:#?}");
        let phrase = asked
            .borrow()
            .shown
            .first()
            .cloned()
            .expect("words were shown");
        runs.push((phrase, words_of(&said(&home, &events))));
    }
    for (index, (phrase, spoken)) in runs.iter().enumerate() {
        let other = &runs[1 - index].1;
        for word in phrase.split_whitespace() {
            assert!(
                !spoken.contains(word) || other.contains(word),
                "the recovery word {word:?} is in what setup printed or wrote to {SETUP_LOG}: \
                 anyone who reads a log a person sends for help could take the account"
            );
        }
    }
}

#[test]
fn red_team_words_typed_back_wrong_write_no_account() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let (mut careless, asked) = Scripted::new(true);
    let (events, failed) = run(&home, answers(dir.path()), &mut careless);
    assert_eq!(
        failed,
        Some(Step::Account),
        "words typed back wrong were accepted: {events:#?}"
    );
    assert_eq!(
        asked.borrow().shown.len(),
        super::steps::CONFIRM_ATTEMPTS,
        "the words were not shown again after a wrong answer"
    );
    assert!(
        !Node::exists(&home),
        "an account was written although its words were never typed back right: its owner \
         leaves with a paper that restores nothing"
    );
}

#[test]
fn red_team_a_username_already_taken_is_refused_before_any_key_is_written() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let mut wanted = answers(dir.path());
    wanted.coordinator = Some("coordinator.test:9898".to_owned());
    let (mut person, asked) = Scripted::new(false);
    let service = NoService {
        file: home.with_extension("passphrase"),
    };
    let mut events = Vec::new();
    let mut record = |event: &Event| events.push(event.clone());
    let outcome = Setup::new(&home, wanted, &mut person, &service, &mut record)
        .with_name_check(|_, name| Ok(name == "camille"))
        .run();
    assert_eq!(
        outcome.failed,
        Some(Step::Account),
        "a name the coordinator already holds was let through the account step: {events:#?}"
    );
    assert!(
        !keystore(&home).exists() && asked.borrow().questions == 0,
        "keys were made, or words shown, for a name the network will refuse: the person \
         writes 24 words for an account that can never register"
    );
    assert!(
        said(&home, &events).contains("already taken"),
        "the refusal does not say the name is taken: {events:#?}"
    );
}

/// A name the page's check had no time to catch: refused by the engine before a
/// word is shown, with the rule. Sabotage: drop the check in `apply_account`.
#[test]
fn red_team_a_username_no_coordinator_would_take_is_refused_before_any_key_is_written() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let mut wanted = answers(dir.path());
    wanted.account = Some(Account::New {
        username: "marie_1".to_owned(),
    });
    wanted.coordinator = Some("coordinator.test:9898".to_owned());
    let (mut person, asked) = Scripted::new(false);
    let service = NoService {
        file: home.with_extension("passphrase"),
    };
    let mut events = Vec::new();
    let mut record = |event: &Event| events.push(event.clone());
    let outcome = Setup::new(&home, wanted, &mut person, &service, &mut record)
        .with_name_check(|_, _| Ok(false))
        .run();
    assert_eq!(outcome.failed, Some(Step::Account), "{events:#?}");
    assert!(
        !keystore(&home).exists() && asked.borrow().questions == 0,
        "words shown for a name registration will refuse with a remedy about something else"
    );
    assert!(
        said(&home, &events).contains("lowercase"),
        "the refusal does not say what a name may be: {events:#?}"
    );
}

#[test]
fn red_team_the_built_in_coordinator_is_pinned_and_another_address_is_not() {
    let pinned = super::pinned_for(super::DEFAULT_COORDINATOR).expect(
        "the built-in coordinator has no pinned device: anything answering at its name is trusted",
    );
    assert!(
        crate::coordinator::parse_device(pinned).is_ok(),
        "the pinned coordinator device id is not one: every member's registration would fail"
    );
    assert_eq!(
        super::pinned_for("other.example:9898"),
        None,
        "the built-in pin was applied to another coordinator, which then could never be used"
    );
}

/// No next step without a positive check: a coordinator that cannot be asked
/// does not make a name free. Sabotage: `Err(_)` passes in `apply_account`.
#[test]
fn red_team_an_unreachable_coordinator_writes_no_key_on_a_new_account() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let mut wanted = answers(dir.path());
    wanted.coordinator = Some("coordinator.test:9898".to_owned());
    let (mut person, asked) = Scripted::new(false);
    let service = NoService {
        file: home.with_extension("passphrase"),
    };
    let mut events = Vec::new();
    let mut record = |event: &Event| events.push(event.clone());
    let outcome = Setup::new(&home, wanted, &mut person, &service, &mut record)
        .with_name_check(|_, _| Err(CliError::Usage("no route to host".to_owned())))
        .run();
    assert_eq!(outcome.failed, Some(Step::Account), "{events:#?}");
    assert!(
        !keystore(&home).exists() && asked.borrow().questions == 0,
        "an account was started under a name nobody could say was free: the person writes \
         24 words for a name the network may already give to somebody else"
    );
    assert!(
        said(&home, &events).contains("could not be asked"),
        "the refusal does not say that the network could not be asked: {events:#?}"
    );
}

/// What a joining person types as their words.
struct Joiner {
    phrase: String,
    passphrases: Rc<RefCell<usize>>,
}

impl SecretPrompt for Joiner {
    fn show_and_confirm(&mut self, _: &str, _: [usize; 3]) -> Result<Option<Vec<Secret>>> {
        Ok(None)
    }
    fn new_passphrase(&mut self) -> Result<Secret> {
        *self.passphrases.borrow_mut() += 1;
        Ok(Zeroizing::new(PASSPHRASE.to_owned()))
    }
    fn passphrase(&mut self, _: &str) -> Result<Secret> {
        self.new_passphrase()
    }
    fn recovery_phrase(&mut self) -> Result<Secret> {
        Ok(Zeroizing::new(self.phrase.clone()))
    }
    fn where_asked(&self) -> &'static str {
        "a test"
    }
}

thread_local! {
    static OWNER: RefCell<Option<itsanas_crypto::UserId>> = const { RefCell::new(None) };
}

fn user_of(phrase: &str) -> itsanas_crypto::UserId {
    let master = itsanas_crypto::MasterSecret::from_recovery_phrase(phrase).expect("words");
    itsanas_crypto::UserKeys::derive(&master).user_id()
}

/// Joining with the words of another account would bind this key to a name that
/// is not its own. Refused before a passphrase is asked; the account's own words,
/// and a coordinator that does not know the name, go on.
/// Sabotage: drop the owner comparison in `refuse_words_of_another_account`.
#[test]
fn red_team_the_words_of_another_account_are_refused_before_a_passphrase_is_asked() {
    let phrase = itsanas_crypto::MasterSecret::generate()
        .and_then(|master| master.to_recovery_phrase())
        .expect("words")
        .to_string();
    for (owner, joins) in [
        (
            Some(itsanas_crypto::UserId::from_bytes(
                [0xde; itsanas_crypto::ID_LEN],
            )),
            false,
        ),
        (Some(user_of(&phrase)), true),
        (None, true),
    ] {
        OWNER.with(|slot| *slot.borrow_mut() = owner);
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("node");
        let mut wanted = answers(dir.path());
        wanted.account = Some(Account::Join {
            username: "camille".to_owned(),
            from: None,
        });
        wanted.coordinator = Some("coordinator.test:9898".to_owned());
        let passphrases = Rc::new(RefCell::new(0));
        let mut person = Joiner {
            phrase: phrase.clone(),
            passphrases: Rc::clone(&passphrases),
        };
        let service = NoService {
            file: home.with_extension("passphrase"),
        };
        let mut events = Vec::new();
        let mut record = |event: &Event| events.push(event.clone());
        let outcome = Setup::new(&home, wanted, &mut person, &service, &mut record)
            .with_owner_check(|_, _| Ok(OWNER.with(|owner| *owner.borrow())))
            .run();
        if joins {
            assert!(
                keystore(&home).exists(),
                "a person joining with their own words was refused ({owner:?}): {events:#?}"
            );
        } else {
            assert_eq!(outcome.failed, Some(Step::Account), "{events:#?}");
            assert!(
                !keystore(&home).exists() && *passphrases.borrow() == 0,
                "words of another account were taken: a passphrase was asked, or keys were \
                 written, under a name that belongs to somebody else"
            );
            assert!(
                said(&home, &events).contains("not the words of the account"),
                "the refusal does not say whose words these are not: {events:#?}"
            );
        }
    }
}
