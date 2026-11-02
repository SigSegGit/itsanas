//! The engine, run on throwaway homes with a scripted person and no service.
//!
//! What is proved here is what makes setup safe to run twice: a second run
//! finds the account and leaves its keys alone, and nothing the engine says
//! -- on screen or in `setup.log` -- carries a recovery word. The windows
//! themselves are `secrets.rs`'s tests; the service definitions `service.rs`'s.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use zeroize::Zeroizing;

use super::{
    Account, Answers, Event, Keep, SETUP_LOG, Secret, SecretPrompt, ServiceControl, Setup, Step,
    sign,
};
use crate::{
    error::{CliError, Result},
    node::Node,
};

const PASSPHRASE: &str = "setup-test-passphrase";

/// What the scripted person was asked, so a test can say "asked nothing".
#[derive(Debug, Default)]
struct Asked {
    /// The phrase shown, as the window would have shown it.
    shown: Vec<String>,
    questions: usize,
}

/// A person who types back the right words, or the wrong ones, and
/// `PASSPHRASE` whenever asked, unless a test gave them another.
struct Scripted {
    asked: Rc<RefCell<Asked>>,
    wrong_words: bool,
    passphrase: &'static str,
}

impl Scripted {
    fn new(wrong_words: bool) -> (Self, Rc<RefCell<Asked>>) {
        let asked = Rc::new(RefCell::new(Asked::default()));
        (
            Self {
                asked: Rc::clone(&asked),
                wrong_words,
                passphrase: PASSPHRASE,
            },
            asked,
        )
    }
}

impl SecretPrompt for Scripted {
    fn show_and_confirm(
        &mut self,
        phrase: &str,
        positions: [usize; 3],
    ) -> Result<Option<Vec<Secret>>> {
        let mut asked = self.asked.borrow_mut();
        asked.questions += 1;
        asked.shown.push(phrase.to_owned());
        let words: Vec<&str> = phrase.split_whitespace().collect();
        Ok(Some(
            positions
                .iter()
                .map(|&position| {
                    let word = words[position - 1];
                    Zeroizing::new(if self.wrong_words {
                        format!("{word}x")
                    } else {
                        word.to_owned()
                    })
                })
                .collect(),
        ))
    }

    fn new_passphrase(&mut self) -> Result<Secret> {
        self.asked.borrow_mut().questions += 1;
        Ok(Zeroizing::new(self.passphrase.to_owned()))
    }

    fn passphrase(&mut self, _: &str) -> Result<Secret> {
        self.new_passphrase()
    }

    fn recovery_phrase(&mut self) -> Result<Secret> {
        Err(CliError::Usage(
            "no account is joined in these tests".to_owned(),
        ))
    }

    fn where_asked(&self) -> &'static str {
        "a scripted test"
    }
}

/// A machine with no service manager: setup is asked for none, and any call
/// to install one is a test failure said as one.
struct NoService {
    file: PathBuf,
}

impl ServiceControl for NoService {
    fn passphrase_file(&self) -> PathBuf {
        self.file.clone()
    }
    fn installed(&self) -> bool {
        false
    }
    fn install(&self, _: bool) -> Result<String> {
        Err(CliError::Usage(
            "setup installed a service it was told not to".to_owned(),
        ))
    }
    fn start(&self) -> Result<()> {
        Err(CliError::Usage(
            "setup started a service it was told not to".to_owned(),
        ))
    }
    fn stop(&self) -> Result<()> {
        Ok(())
    }
    fn set_autostart(&self, _: bool) -> Result<()> {
        Err(CliError::Usage(
            "setup changed a service it was told not to".to_owned(),
        ))
    }
    fn log_hint(&self) -> String {
        "the terminal".to_owned()
    }
}

fn answers(base: &Path) -> Answers {
    Answers {
        folder: Some(base.join("folder")),
        account: Some(Account::New {
            username: "camille".to_owned(),
        }),
        pledge: Some(1024 * 1024),
        service: false,
        tray: false,
        verify_for: Duration::from_secs(1),
        ..Answers::default()
    }
}

/// Run setup once; the events it emitted and the step that failed, if any.
fn run(home: &Path, answers: Answers, prompt: &mut dyn SecretPrompt) -> (Vec<Event>, Option<Step>) {
    let service = NoService {
        file: home.with_extension("passphrase"),
    };
    let mut events = Vec::new();
    let mut record = |event: &Event| events.push(event.clone());
    let outcome = Setup::new(home, answers, prompt, &service, &mut record).run();
    (events, outcome.failed)
}

/// The sealed keys: `Node`'s own path for them is private to `itsanas-node`.
fn keystore(home: &Path) -> PathBuf {
    home.join("keystore.bin")
}

fn words_of(text: &str) -> BTreeSet<String> {
    // Split on what is not a letter or digit, then drop tokens with a digit:
    // a hex id printed by setup ("3add9f...") split on its digits read as the
    // recovery word "add" whenever the phrase drew it (CI, 2026-10-19).
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty() && token.chars().all(|c| c.is_ascii_alphabetic()))
        .map(str::to_lowercase)
        .collect()
}

/// Everything the engine said in one run: its events and its log.
fn said(home: &Path, events: &[Event]) -> String {
    let mut text: String = events.iter().map(|event| event.line() + "\n").collect();
    text.push_str(&std::fs::read_to_string(home.join(SETUP_LOG)).unwrap_or_default());
    text
}

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

/// A service that is there and does what it is told, recording it.
struct Recording {
    file: PathBuf,
    calls: RefCell<Vec<&'static str>>,
}

impl ServiceControl for Recording {
    fn passphrase_file(&self) -> PathBuf {
        self.file.clone()
    }
    fn installed(&self) -> bool {
        true
    }
    fn install(&self, _: bool) -> Result<String> {
        self.calls.borrow_mut().push("install");
        Ok("installed".to_owned())
    }
    fn start(&self) -> Result<()> {
        self.calls.borrow_mut().push("start");
        Ok(())
    }
    fn stop(&self) -> Result<()> {
        self.calls.borrow_mut().push("stop");
        Ok(())
    }
    fn set_autostart(&self, on: bool) -> Result<()> {
        self.calls
            .borrow_mut()
            .push(if on { "autostart on" } else { "autostart off" });
        Ok(())
    }
    fn log_hint(&self) -> String {
        String::new()
    }
}

struct Typing(&'static str);

impl SecretPrompt for Typing {
    fn show_and_confirm(&mut self, _: &str, _: [usize; 3]) -> Result<Option<Vec<Secret>>> {
        Ok(None)
    }
    fn new_passphrase(&mut self) -> Result<Secret> {
        Ok(Zeroizing::new(self.0.to_owned()))
    }
    fn passphrase(&mut self, _: &str) -> Result<Secret> {
        Ok(Zeroizing::new(self.0.to_owned()))
    }
    fn recovery_phrase(&mut self) -> Result<Secret> {
        Ok(Zeroizing::new(String::new()))
    }
    fn where_asked(&self) -> &'static str {
        "a test"
    }
}

#[test]
fn red_team_sign_out_forgets_the_passphrase_and_sign_in_needs_the_right_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    Node::create(&home, PASSPHRASE, "camille").expect("create");
    let sealed = std::fs::read(keystore(&home)).expect("keystore");
    let service = Recording {
        file: dir.path().join("service").join("passphrase"),
        calls: RefCell::new(Vec::new()),
    };
    super::service::write_passphrase_file(&service.file, PASSPHRASE).expect("write");

    sign::sign_out(&home, &service).expect("sign out");
    assert!(
        !service.file.exists(),
        "signed out, and the passphrase file is still there: the machine handed to somebody \
         else still unlocks the account at the next logon"
    );
    assert_eq!(
        *service.calls.borrow(),
        ["autostart off", "stop"],
        "sign out left the service to start again at logon, with no passphrase to unlock"
    );
    assert_eq!(
        std::fs::read(keystore(&home)).expect("keystore"),
        sealed,
        "sign out touched the keys, which it promises to keep"
    );

    service.calls.borrow_mut().clear();
    let refused = sign::sign_in(&home, &service, &mut Typing("not the passphrase"));
    assert!(
        matches!(refused, Err(CliError::Unlock)),
        "a wrong passphrase was accepted at sign in: {refused:?}"
    );
    assert!(
        !service.file.exists() && service.calls.borrow().is_empty(),
        "a wrong passphrase was saved for the service, which would then fail at every logon"
    );

    sign::sign_in(&home, &service, &mut Typing(PASSPHRASE)).expect("sign in");
    assert_eq!(
        super::service::read_passphrase_file(&service.file)
            .as_deref()
            .map(String::as_str),
        Some(PASSPHRASE),
        "signed in, and the service has no passphrase to start with"
    );
    assert_eq!(
        *service.calls.borrow(),
        ["autostart on", "start"],
        "signed in, and the service was not started again"
    );
}

#[test]
fn red_team_the_service_is_never_installed_for_a_home_it_would_not_run() {
    // The task, the unit and the plist run the node `--instance` names, not
    // the `--home` setup was given: installed for a throwaway home, the
    // service would start some other node, or none, and setup would say done.
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    let service = Recording {
        file: dir.path().join("service").join("passphrase"),
        calls: RefCell::new(Vec::new()),
    };
    let mut wanted = answers(dir.path());
    wanted.service = true;
    let (mut person, _) = Scripted::new(false);
    let mut events = Vec::new();
    let mut record = |event: &Event| events.push(event.clone());
    let outcome = Setup::new(&home, wanted, &mut person, &service, &mut record).run();
    assert_eq!(
        outcome.failed,
        Some(Step::Service),
        "setup on a home no service runs did not stop at the service: {events:#?}"
    );
    assert!(
        !service.calls.borrow().contains(&"install"),
        concat!(
            "a service was installed for a home it does not run: the person is told the ",
            "machine works while no daemon ever opens this node"
        )
    );
}

#[test]
fn red_team_joining_from_a_coordinator_refuses_a_passphrase_the_service_file_cannot_hold() {
    let base = tempfile::tempdir().expect("tempdir");
    let home = base.path().join("home");
    let service = Recording {
        file: base.path().join("passphrase.txt"),
        calls: RefCell::new(Vec::new()),
    };
    let mut answers = answers(base.path());
    answers.account = Some(Account::Join {
        username: "camille".to_owned(),
        // Nothing listens here: the refusal must come before any network.
        from: Some("127.0.0.1:1".to_owned()),
    });
    answers.service = true;
    // A line break is refused on every system, so this runs the same on all.
    let mut prompt = Typing("line\nbreak");
    let mut events = Vec::new();
    let mut record = |event: &Event| events.push(event.clone());
    let outcome = Setup::new(&home, answers, &mut prompt, &service, &mut record).run();
    assert_eq!(outcome.failed, Some(Step::Account));
    assert!(
        events.iter().any(
            |event| matches!(event, Event::Failed { error, .. } if error.contains("line break"))
        ),
        "a passphrase the service file cannot hold reached the coordinator: the keystore is \
         restored under it and the Secret step then fails on every run"
    );
    assert!(
        !Node::exists(&home),
        "a keystore was written under a passphrase no service can use"
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

/// A service that remembers the passphrase its own way, as a phone does behind
/// its keystore: it must never be handed a file.
struct Remembering {
    file: PathBuf,
    kept: RefCell<Option<String>>,
}

impl ServiceControl for Remembering {
    fn passphrase_file(&self) -> PathBuf {
        self.file.clone()
    }
    fn stored_passphrase(&self) -> Option<Secret> {
        self.kept.borrow().clone().map(Zeroizing::new)
    }
    fn store_passphrase(&self, passphrase: &Secret) -> Result<String> {
        *self.kept.borrow_mut() = Some(passphrase.to_string());
        Ok("the keystore holds it".to_owned())
    }
    fn forget_passphrase(&self) -> Result<&'static str> {
        *self.kept.borrow_mut() = None;
        Ok("the remembered copy is forgotten")
    }
    fn installed(&self) -> bool {
        true
    }
    fn install(&self, _: bool) -> Result<String> {
        Ok("installed".to_owned())
    }
    fn start(&self) -> Result<()> {
        Ok(())
    }
    fn stop(&self) -> Result<()> {
        Ok(())
    }
    fn set_autostart(&self, _: bool) -> Result<()> {
        Ok(())
    }
    fn log_hint(&self) -> String {
        String::new()
    }
}

#[test]
fn red_team_a_service_that_remembers_the_passphrase_its_own_way_is_never_given_a_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("node");
    Node::create(&home, PASSPHRASE, "camille").expect("create");
    let service = Remembering {
        file: dir.path().join("service").join("passphrase"),
        kept: RefCell::new(Some(PASSPHRASE.to_owned())),
    };

    sign::sign_out(&home, &service).expect("sign out");
    assert_eq!(
        service.stored_passphrase(),
        None,
        "signed out, and the remembered passphrase is still there: the phone lent to somebody \
         else unlocks the account"
    );

    sign::sign_in(&home, &service, &mut Typing(PASSPHRASE)).expect("sign in");
    assert_eq!(
        service.stored_passphrase().as_deref().map(String::as_str),
        Some(PASSPHRASE),
        "signed in, and nothing remembers the passphrase for the service"
    );
    assert!(
        !service.file.exists() && !service.file.with_extension("new").exists(),
        "sign in wrote a passphrase file for a service that keeps it elsewhere: a copy of the \
         secret outside the keystore, readable by whatever runs as the app"
    );
}

#[test]
fn the_default_passphrase_methods_read_write_and_forget_today_s_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let service = NoService {
        file: dir.path().join("service").join("passphrase"),
    };
    assert_eq!(service.stored_passphrase(), None);
    let secret = Zeroizing::new(PASSPHRASE.to_owned());
    service.store_passphrase(&secret).expect("store");
    assert_eq!(
        super::service::read_passphrase_file(&service.file)
            .as_deref()
            .map(String::as_str),
        Some(PASSPHRASE),
        "a desktop's remembered passphrase is not in the file the service reads"
    );
    assert_eq!(
        service.stored_passphrase().as_deref().map(String::as_str),
        Some(PASSPHRASE)
    );
    assert_eq!(
        service.forget_passphrase().expect("forget"),
        "the passphrase file is deleted"
    );
    assert_eq!(
        service.forget_passphrase().expect("forget again"),
        "there was no passphrase file to delete"
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
