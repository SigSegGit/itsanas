//! The phone's side of the page: where a secret is asked, and what the app is
//! told when the page changes something it draws.
//!
//! Android's engine calls Kotlin directly (`itsanas-android/src/host.rs`).
//! Calling Swift from Rust would mean C function pointers and an `unsafe`
//! call each time; instead the engine leaves its question in a mailbox and
//! waits, and the app, which polls `itsanas_next_request` while the page is
//! on screen, shows a native field and answers with `itsanas_reply`. The
//! secret never passes through the web view, as on Android.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::keychain;
use itsanas_cli::shell::{CliError, Host, Result, Secret, SecretPrompt, ServiceControl};
use zeroize::Zeroizing;

/// One question, or one piece of news, for the app.
#[derive(Debug)]
struct Request {
    id: u64,
    kind: &'static str,
    args: Vec<Secret>,
    reply: bool,
}

/// What is waiting on either side.
#[derive(Debug)]
struct Mailbox {
    next: u64,
    pending: VecDeque<Request>,
    /// The questions an engine thread is still waiting on. A reply to any
    /// other id is dropped: it cannot answer a later question.
    waiting: BTreeSet<u64>,
    replies: BTreeMap<u64, Option<Secret>>,
}

static MAILBOX: Mutex<Mailbox> = Mutex::new(Mailbox {
    next: 1,
    pending: VecDeque::new(),
    waiting: BTreeSet::new(),
    replies: BTreeMap::new(),
});
static ANSWERED: Condvar = Condvar::new();

/// How often a waiting engine looks whether its page is still the current one.
const LOOK_AGAIN: Duration = Duration::from_millis(250);

fn mailbox() -> MutexGuard<'static, Mailbox> {
    MAILBOX.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Leave news for the app; nobody waits for an answer.
pub(crate) fn tell(kind: &'static str, args: &[&str]) {
    let mut mail = mailbox();
    let id = mail.next;
    mail.next += 1;
    mail.pending.push_back(Request {
        id,
        kind,
        args: args
            .iter()
            .map(|arg| Zeroizing::new((*arg).to_owned()))
            .collect(),
        reply: false,
    });
}

/// A new page starts with an empty mailbox. News left by the page before it
/// (`closed`, above all) would otherwise be read as this page's, and close it
/// the moment it opens. Questions end with their engine, which has been told
/// its page is gone.
pub(crate) fn forget_old_news() {
    mailbox().pending.clear();
}

/// Ask the app and wait for its answer: `None` when the person went Back.
///
/// Gives up as soon as `page` is no longer the current page, so an engine
/// whose page was left does not sit on a question nobody will see.
fn ask(page: u64, kind: &'static str, args: &[&str]) -> Result<Option<Secret>> {
    let mut mail = mailbox();
    let id = mail.next;
    mail.next += 1;
    mail.waiting.insert(id);
    mail.pending.push_back(Request {
        id,
        kind,
        args: args
            .iter()
            .map(|arg| Zeroizing::new((*arg).to_owned()))
            .collect(),
        reply: true,
    });
    loop {
        if let Some(answer) = mail.replies.remove(&id) {
            return Ok(answer);
        }
        if crate::GENERATION.load(Ordering::SeqCst) != page {
            mail.waiting.remove(&id);
            mail.pending.retain(|request| request.id != id);
            return Err(left());
        }
        mail = ANSWERED
            .wait_timeout(mail, LOOK_AGAIN)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

/// The next thing for the app, as JSON, or `{}` when there is none.
pub(crate) fn next_request() -> String {
    let Some(request) = mailbox().pending.pop_front() else {
        return "{}".to_owned();
    };
    let args: Vec<&str> = request.args.iter().map(|arg| arg.as_str()).collect();
    serde_json::json!({
        "id": request.id,
        "kind": request.kind,
        "args": args,
        "reply": request.reply,
    })
    .to_string()
}

/// The app's answer to question `id`; `None` for Back. Whether it was taken.
pub(crate) fn reply(id: u64, answer: Option<Secret>) -> bool {
    let mut mail = mailbox();
    // Taken off `waiting` here, not when the engine reads it: a second reply
    // sent before the engine woke would otherwise replace the first.
    if !mail.waiting.remove(&id) {
        return false;
    }
    mail.replies.insert(id, answer);
    ANSWERED.notify_all();
    true
}

fn left() -> CliError {
    CliError::Usage(
        "the screen was closed; nothing was saved. Start again when you are ready".to_owned(),
    )
}

/// Where the engine of page `page` asks its secrets.
#[derive(Debug)]
pub(crate) struct PhonePrompt {
    page: u64,
}

impl PhonePrompt {
    pub(crate) fn of_page(page: u64) -> Self {
        Self { page }
    }

    fn ask(&self, kind: &'static str, args: &[&str]) -> Result<Secret> {
        ask(self.page, kind, args)?.ok_or_else(left)
    }
}

impl SecretPrompt for PhonePrompt {
    fn show_and_confirm(
        &mut self,
        phrase: &str,
        positions: [usize; 3],
    ) -> Result<Option<Vec<Secret>>> {
        let at = positions.map(|position| position.to_string());
        let typed = self.ask("showAndConfirm", &[phrase, &at[0], &at[1], &at[2]])?;
        Ok(Some(
            typed
                .split_whitespace()
                .map(|word| Zeroizing::new(word.to_owned()))
                .collect(),
        ))
    }

    fn new_passphrase(&mut self) -> Result<Secret> {
        self.ask("askNewPassphrase", &[])
    }

    fn passphrase(&mut self, purpose: &str) -> Result<Secret> {
        self.ask("askPassphrase", &[purpose])
    }

    fn show_words(&mut self, phrase: &str) -> Result<()> {
        self.ask("showWords", &[phrase])?;
        Ok(())
    }

    fn recovery_phrase(&mut self) -> Result<Secret> {
        self.ask("askRecoveryPhrase", &[])
    }

    fn where_asked(&self) -> &'static str {
        "a screen of the app"
    }
}

/// The app is its own service, and the Keychain keeps the passphrase. The
/// engine stores it as a step of setup: refusing here failed the step, and a
/// new account with it.
#[derive(Debug, Default)]
pub(crate) struct PhoneService;

impl ServiceControl for PhoneService {
    fn passphrase_file(&self) -> PathBuf {
        PathBuf::new()
    }

    fn stored_passphrase(&self) -> Option<Secret> {
        keychain::get()
    }

    fn store_passphrase(&self, passphrase: &Secret) -> Result<String> {
        keychain::set(passphrase)
            .map_err(|why| CliError::Usage(format!("the Keychain refused it: {why}")))?;
        Ok("the phone's Keychain holds it".to_owned())
    }

    fn forget_passphrase(&self) -> Result<&'static str> {
        keychain::forget();
        Ok("the phone's Keychain forgot it")
    }

    fn installed(&self) -> bool {
        true
    }

    fn install(&self, _tray: bool) -> Result<String> {
        Ok("the app is its own service".to_owned())
    }

    fn start(&self) -> Result<()> {
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        Ok(())
    }

    fn set_autostart(&self, _on: bool) -> Result<()> {
        Ok(())
    }

    fn log_hint(&self) -> String {
        "the device log (Console on a Mac, or idevicesyslog)".to_owned()
    }

    fn holds_the_store_itself(&self) -> bool {
        true
    }
}

/// What the app is told of the page's changes.
#[derive(Debug, Default)]
pub(crate) struct PhoneHost;

impl Host for PhoneHost {
    fn event(&self, name: &str, value: &str) {
        tell("event", &[name, value]);
    }

    fn value(&self, _name: &str) -> Option<String> {
        None
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex as TestLock;

    /// The mailbox and the page generation are process-wide: tests take turns.
    pub(crate) static SERIAL: TestLock<()> = TestLock::new(());

    fn drain() {
        while next_request() != "{}" {}
    }

    #[test]
    fn a_question_reaches_the_app_and_its_answer_reaches_the_engine() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        drain();
        let page = crate::GENERATION.load(Ordering::SeqCst);
        let engine = std::thread::spawn(move || PhonePrompt::of_page(page).new_passphrase());
        let request = loop {
            let said = next_request();
            if said != "{}" {
                break serde_json::from_str::<serde_json::Value>(&said).expect("json");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(request["kind"], "askNewPassphrase", "{request}");
        assert_eq!(request["reply"], true, "{request}");
        let id = request["id"].as_u64().expect("an id");
        assert!(reply(id, Some(Zeroizing::new("chosen words".to_owned()))));
        let got = engine.join().expect("engine").expect("an answer");
        assert_eq!(
            got.as_str(),
            "chosen words",
            "the passphrase typed is not the one the account is made with"
        );
    }

    #[test]
    fn a_remembered_passphrase_comes_back_until_it_is_forgotten() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let service = PhoneService;
        service
            .store_passphrase(&Zeroizing::new("kept".to_owned()))
            .expect("setup's step fails, and the account with it, when this is refused");
        assert_eq!(
            service.stored_passphrase().as_deref().map(String::as_str),
            Some("kept"),
            "the phone asks a passphrase it was told to remember"
        );
        service.forget_passphrase().expect("forget");
        assert!(
            service.stored_passphrase().is_none(),
            "a signed-out phone can still open the account"
        );
    }

    #[test]
    fn red_team_a_stray_reply_cannot_answer_the_next_question() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        drain();
        // An answer sent before any question, or twice to the same one, must
        // not sit in the mailbox: the next passphrase asked would be it, and
        // an account would be made with a secret its owner never typed.
        assert!(
            !reply(4242, Some(Zeroizing::new("stale".to_owned()))),
            "a reply to a question nobody asked was taken"
        );
        let page = crate::GENERATION.load(Ordering::SeqCst);
        let engine = std::thread::spawn(move || PhonePrompt::of_page(page).new_passphrase());
        let id = loop {
            let said = next_request();
            if said != "{}" {
                let request: serde_json::Value = serde_json::from_str(&said).expect("json");
                break request["id"].as_u64().expect("an id");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(reply(id, Some(Zeroizing::new("typed".to_owned()))));
        assert!(
            !reply(id, Some(Zeroizing::new("again".to_owned()))),
            "a second reply to an answered question was taken"
        );
        let got = engine.join().expect("engine").expect("an answer");
        assert_eq!(got.as_str(), "typed", "a stray reply answered the question");
        assert!(
            mailbox().replies.is_empty(),
            "a reply is left over for the next question"
        );
    }

    #[test]
    fn red_team_an_iphone_engine_whose_page_was_left_asks_nothing() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        drain();
        let page = crate::GENERATION.fetch_add(1, Ordering::SeqCst);
        let said = PhonePrompt::of_page(page)
            .new_passphrase()
            .expect_err("a left page's engine had an answer")
            .to_string();
        assert!(
            said.contains("the screen was closed"),
            "a left page's engine went on waiting for a passphrase: {said}"
        );
        assert_eq!(
            next_request(),
            "{}",
            "a left page's question is still shown to the person"
        );
    }
}
