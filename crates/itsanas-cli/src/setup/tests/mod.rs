//! The engine, run on throwaway homes with a scripted person and no service.
//!
//! What is proved here is what makes setup safe to run twice: a second run
//! finds the account and leaves its keys alone, and nothing the engine says
//! -- on screen or in `setup.log` -- carries a recovery word. The windows
//! themselves are `secrets.rs`'s tests; the service definitions `service/texts.rs`'s.
//!
//! This module holds the scripted person and the stand-in services; each
//! concern's tests are a module of their own.

mod accounts;
mod machine_choices;
mod passphrase;
mod service_home;

use std::{
    cell::RefCell,
    collections::BTreeSet,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use zeroize::Zeroizing;

use super::{
    Account, Answers, DEFAULT_COORDINATOR, Event, Keep, SETUP_LOG, Secret, SecretPrompt,
    ServiceControl, Setup, Step, answers, forgot, pinned_for, service, sign, steps,
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
