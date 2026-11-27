//! The passphrase rule, a refused passphrase asked again, and "I forgot my
//! passphrase".

use super::*;

/// A passphrase outside the rule is refused as soon as it is typed, with the
/// numbers, ahead of anything else that could be said of it, and nothing is
/// written. A short one with a line break is the case that tells the two checks
/// apart: both refuse it, and only one says how long a passphrase may be.
/// Sabotage: drop `check_new_passphrase` from `new_passphrase`.
#[test]
fn red_team_setup_refuses_a_new_passphrase_outside_the_rule_before_anything_is_written() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let (mut person, _) = Scripted::new(false);
    person.passphrase = "ab\ncd";
    let mut wanted = answers(dir.path());
    wanted.service = true;
    let (events, failed) = run(&home, wanted, &mut person);
    assert_eq!(failed, Some(Step::Account), "{events:#?}");
    assert!(
        !keystore(&home).exists(),
        "a refused passphrase sealed keys"
    );
    let text = said(&home, &events);
    assert!(
        text.contains('6') && text.contains("128"),
        "the person is not told the rule: {text}"
    );
}

/// A person whose new passphrase is refused is asked again, with the reason,
/// instead of losing a long setup at its end. Three refusals stop it.
/// Sabotage: one attempt only; drop the `AskingAgain` event.
struct Answers3(RefCell<Vec<&'static str>>);

impl SecretPrompt for Answers3 {
    fn show_and_confirm(&mut self, _: &str, _: [usize; 3]) -> Result<Option<Vec<Secret>>> {
        Ok(None)
    }
    fn new_passphrase(&mut self) -> Result<Secret> {
        let next = self.0.borrow_mut().remove(0);
        Ok(Zeroizing::new(next.to_owned()))
    }
    fn passphrase(&mut self, _: &str) -> Result<Secret> {
        Ok(Zeroizing::new(PASSPHRASE.to_owned()))
    }
    fn recovery_phrase(&mut self) -> Result<Secret> {
        Ok(Zeroizing::new(String::new()))
    }
    fn where_asked(&self) -> &'static str {
        "a test"
    }
}

#[test]
fn red_team_a_refused_passphrase_is_asked_again_with_the_reason_and_three_refusals_stop_the_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    // Not hex and not a temporary path: the transcript prints a device id, and
    // a typed "abc" was found in one about one run in seventy.
    let mut person = Answers3(RefCell::new(vec!["x!z", "five5", PASSPHRASE]));
    let (events, failed) = run(&home, answers(dir.path()), &mut person);
    assert_eq!(failed, None, "a slip lost the setup: {events:#?}");
    assert!(
        keystore(&home).exists(),
        "the third passphrase typed was not the one the keys were sealed under"
    );
    let again: Vec<&Event> = events
        .iter()
        .filter(|event| matches!(event, Event::AskingAgain { .. }))
        .collect();
    assert_eq!(
        again.len(),
        2,
        "the person was not told twice why: {events:#?}"
    );
    let text = said(&home, &events);
    assert!(
        text.contains("asking again") && !text.contains("x!z") && !text.contains("five5"),
        "the reason is missing, or what was typed was written down with it: {text}"
    );

    let other = tempfile::tempdir().expect("tempdir");
    let home = other.path().join("node");
    let mut person = Answers3(RefCell::new(vec!["a", "ab", "abc", PASSPHRASE]));
    let (events, failed) = run(&home, answers(other.path()), &mut person);
    assert_eq!(failed, Some(Step::Account), "{events:#?}");
    assert!(
        !keystore(&home).exists() && person.0.borrow().len() == 1,
        "a fourth passphrase was asked for, or keys were written under a refused one"
    );
}

/// Somebody who forgot the passphrase: the words, whose account they open, and a
/// new passphrase.
struct Forgetting {
    phrase: String,
    new: Vec<&'static str>,
}

impl SecretPrompt for Forgetting {
    fn show_and_confirm(&mut self, _: &str, _: [usize; 3]) -> Result<Option<Vec<Secret>>> {
        Ok(None)
    }
    fn new_passphrase(&mut self) -> Result<Secret> {
        Ok(Zeroizing::new(self.new.remove(0).to_owned()))
    }
    fn passphrase(&mut self, _: &str) -> Result<Secret> {
        Err(CliError::Usage(
            "forgetting asks no old passphrase".to_owned(),
        ))
    }
    fn recovery_phrase(&mut self) -> Result<Secret> {
        Ok(Zeroizing::new(self.phrase.clone()))
    }
    fn where_asked(&self) -> &'static str {
        "a test"
    }
}

/// An account made by setup in `dir`, its words, and a coordinator nobody
/// answers at: the account is looked up and the old device forgotten only when
/// somebody does, and neither may stop a recovery.
fn an_account_to_forget(dir: &Path) -> (PathBuf, String) {
    let home = dir.join("node");
    let (mut person, asked) = Scripted::new(false);
    let (events, failed) = run(&home, answers(dir), &mut person);
    assert_eq!(failed, None, "{events:#?}");
    let phrase = asked.borrow().shown[0].clone();
    let mut node = Node::open(&home, PASSPHRASE).expect("open");
    node.config.coordinator = Some("coordinator.test:9898".to_owned());
    node.save_config().expect("save");
    (home, phrase)
}

/// "I forgot my passphrase" replaces the keys from the account's own words and
/// from nothing else: words of another account, words that do not parse and a
/// passphrase that is refused three times each leave the node byte for byte as it
/// was, and the service is not even stopped. The right words give the same
/// account under a new passphrase, a new device, and the service the new
/// passphrase and a start.
/// Sabotage: drop the owner comparison; drop the `store_passphrase`; skip the
/// rename back.
#[test]
fn red_team_forgetting_the_passphrase_replaces_the_keys_only_for_the_account_s_own_words() {
    use itsanas_node::node::Identity;
    let dir = tempfile::tempdir().expect("tempdir");
    let (home, phrase) = an_account_to_forget(dir.path());
    let before = Identity::open(&home, PASSPHRASE).expect("open");
    let (user, device) = (before.user.user_id(), before.device.device_id());
    drop(before);
    let sealed = std::fs::read(keystore(&home)).expect("keystore");

    let recording = Recording {
        file: dir.path().join("service").join("passphrase"),
        calls: RefCell::new(Vec::new()),
    };
    let untouched = |why: &str| {
        assert_eq!(
            std::fs::read(keystore(&home)).expect("keystore"),
            sealed,
            "{why}: the keys were touched"
        );
        assert!(
            !std::fs::read_dir(dir.path())
                .expect("dir")
                .any(|entry| entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .contains("replaced")),
            "{why}: the old node was left set aside"
        );
        assert!(
            recording.calls.borrow().is_empty(),
            "{why}: the service was stopped for nothing"
        );
    };

    // Words of another account.
    let other_words = itsanas_crypto::MasterSecret::generate()
        .and_then(|master| master.to_recovery_phrase())
        .expect("words")
        .to_string();
    let mut person = Forgetting {
        phrase: other_words,
        new: vec!["a brand new passphrase"],
    };
    let refused = super::forgot::forgot(&home, &mut person, &recording, |_, _| {
        Ok(Some(itsanas_crypto::UserId::from_bytes(
            [0xde; itsanas_crypto::ID_LEN],
        )))
    });
    // The words are a real phrase of another account; the coordinator says the
    // name is held by yet another key, so they are refused as not this account's.
    assert!(
        refused.is_err(),
        "the words of another account replaced the keys"
    );
    untouched("words of another account");

    // Words that do not parse.
    let mut person = Forgetting {
        phrase: "one two three".to_owned(),
        new: vec!["a brand new passphrase"],
    };
    assert!(super::forgot::forgot(&home, &mut person, &recording, |_, _| Ok(None)).is_err());
    untouched("words that do not parse");

    // A passphrase refused three times.
    let mut person = Forgetting {
        phrase: phrase.clone(),
        new: vec!["a", "ab", "abc"],
    };
    assert!(super::forgot::forgot(&home, &mut person, &recording, |_, _| Ok(None)).is_err());
    untouched("a passphrase refused three times");

    // The account's own words.
    // The first passphrase is a slip, and is asked again.
    let mut person = Forgetting {
        phrase,
        new: vec!["a", "a brand new passphrase"],
    };
    let said = super::forgot::forgot(&home, &mut person, &recording, |_, _| Ok(None))
        .expect("the account's own words");
    assert!(said.contains("camille"), "{said}");
    assert!(
        Identity::open(&home, PASSPHRASE).is_err(),
        "the old passphrase still opens the keys"
    );
    let after = Identity::open(&home, "a brand new passphrase").expect("the new passphrase opens");
    assert_eq!(after.user.user_id(), user, "the words made another account");
    assert_ne!(
        after.device.device_id(),
        device,
        "the device was not renewed"
    );
    assert_eq!(after.config.username, "camille");
    assert_eq!(
        after.config.coordinator.as_deref(),
        Some("coordinator.test:9898"),
        "what was set up on this machine was lost with the keys"
    );
    assert_eq!(
        *recording.calls.borrow(),
        ["stop", "start"],
        "the service was not stopped for the swap and started after it"
    );
    assert_eq!(
        super::service::read_passphrase_file(&recording.file)
            .as_deref()
            .map(String::as_str),
        Some("a brand new passphrase"),
        "the service would start with a passphrase that opens nothing"
    );
}
