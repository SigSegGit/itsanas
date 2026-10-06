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
    Account, Answers, Event, SETUP_LOG, Secret, SecretPrompt, ServiceControl, Setup, Step, sign,
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
/// `PASSPHRASE` whenever asked.
struct Scripted {
    asked: Rc<RefCell<Asked>>,
    wrong_words: bool,
}

impl Scripted {
    fn new(wrong_words: bool) -> (Self, Rc<RefCell<Asked>>) {
        let asked = Rc::new(RefCell::new(Asked::default()));
        (
            Self {
                asked: Rc::clone(&asked),
                wrong_words,
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
        Ok(Zeroizing::new(PASSPHRASE.to_owned()))
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
    text.split(|c: char| !c.is_ascii_alphabetic())
        .filter(|token| !token.is_empty())
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
    for name in ["one", "two"] {
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
