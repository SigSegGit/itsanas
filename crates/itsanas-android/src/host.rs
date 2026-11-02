//! The phone's side of the page: where a secret is asked, how the service is
//! driven, and what the app is told when the page changes something it draws.
//!
//! The setup engine runs on a thread of its own and asks for a secret by
//! calling [`PhonePrompt`], which calls the Kotlin `Host` object and parks the
//! engine thread there until a Compose screen, laid over the web view, answers.
//! A thread the JVM did not start cannot look an application class up by name
//! (it sees the system loader only), so `Host` is found once, in `pageStart`,
//! which a Java thread calls, and kept as a global reference.
//!
//! Safe `jni` calls only: `scripts/check-unsafe.py` allows no `unsafe` block.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use itsanas_cli::shell::{CliError, Host, Result, Secret, SecretPrompt, ServiceControl};
use jni::objects::{GlobalRef, JClass, JString, JValue};
use jni::{JNIEnv, JavaVM};
use zeroize::Zeroizing;

/// The class the Kotlin side answers from.
const HOST_CLASS: &str = "fr/ngas/itsanas/Host";

/// What `pageStart` keeps for every later upcall.
struct Upcalls {
    vm: JavaVM,
    class: GlobalRef,
}

static UPCALLS: Mutex<Option<Arc<Upcalls>>> = Mutex::new(None);

/// Take the machine and the `Host` class from a Java thread.
///
/// # Errors
/// The class is missing (a build whose Kotlin and Rust disagree) or the VM
/// cannot be reached.
pub(crate) fn remember(env: &mut JNIEnv) -> std::result::Result<(), String> {
    let vm = env.get_java_vm().map_err(|e| e.to_string())?;
    let class = env.find_class(HOST_CLASS).map_err(|e| e.to_string())?;
    let class = env.new_global_ref(class).map_err(|e| e.to_string())?;
    *UPCALLS.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(Upcalls { vm, class }));
    Ok(())
}

fn upcalls() -> Result<Arc<Upcalls>> {
    UPCALLS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .ok_or_else(|| CliError::Usage("the app's screens are not ready".to_owned()))
}

fn broken(what: impl std::fmt::Display) -> CliError {
    CliError::Usage(format!("the app's screen failed: {what}"))
}

impl Upcalls {
    /// Call `Host.<name>`; `Some(text)` when it answers a string, `None` for
    /// null (the person went Back) or for a method that returns nothing.
    fn call(&self, name: &str, signature: &str, args: &[&str]) -> Result<Option<Secret>> {
        let mut env = self.vm.attach_current_thread().map_err(broken)?;
        let class = env.new_local_ref(self.class.as_obj()).map_err(broken)?;
        let class = JClass::from(class);
        let mut strings = Vec::with_capacity(args.len());
        for arg in args {
            strings.push(env.new_string(arg).map_err(broken)?);
        }
        let values: Vec<JValue> = strings.iter().map(|s| JValue::Object(s)).collect();
        let outcome = env.call_static_method(&class, name, signature, &values);
        // A Kotlin exception left pending would be thrown into the next JNI
        // call of this thread; it is cleared and told as a fault.
        if env.exception_check().unwrap_or(true) {
            let _ = env.exception_clear();
            return Err(broken(format!("{name} threw")));
        }
        let value = outcome.map_err(broken)?;
        if signature.ends_with(")V") {
            return Ok(None);
        }
        let object = value.l().map_err(broken)?;
        if object.is_null() {
            return Ok(None);
        }
        let text: String = env
            .get_string(&JString::from(object))
            .map_err(broken)?
            .into();
        Ok(Some(Zeroizing::new(text)))
    }

    /// `Host.<name>(Int, Int, Int, String)`-free variant for the three
    /// positions of the confirmation.
    fn confirm(&self, phrase: &str, at: [usize; 3]) -> Result<Option<Secret>> {
        let mut env = self.vm.attach_current_thread().map_err(broken)?;
        let class = env.new_local_ref(self.class.as_obj()).map_err(broken)?;
        let class = JClass::from(class);
        let phrase = env.new_string(phrase).map_err(broken)?;
        let position = |i: usize| JValue::Int(i32::try_from(at[i]).unwrap_or(0));
        let outcome = env.call_static_method(
            &class,
            "showAndConfirm",
            "(Ljava/lang/String;III)Ljava/lang/String;",
            &[
                JValue::Object(&phrase),
                position(0),
                position(1),
                position(2),
            ],
        );
        if env.exception_check().unwrap_or(true) {
            let _ = env.exception_clear();
            return Err(broken("showAndConfirm threw"));
        }
        let object = outcome.map_err(broken)?.l().map_err(broken)?;
        if object.is_null() {
            return Ok(None);
        }
        let text: String = env
            .get_string(&JString::from(object))
            .map_err(broken)?
            .into();
        Ok(Some(Zeroizing::new(text)))
    }
}

/// The sentence a closed screen ends the engine with: the one a closed window
/// ends it with on a computer.
fn left() -> CliError {
    CliError::Usage(
        "the screen was closed; nothing was saved. Start again when you are ready".to_owned(),
    )
}

/// Where the engine asks for secrets on a phone: a screen over the web view.
///
/// `page` is the page this engine was started for; `None` for the Unlock screen's
/// forgotten passphrase, which has no page. Leaving a page does not stop its
/// engine, which may still be checking the connection, and a question it asked
/// afterwards would draw a second set of 24 words over whatever is on screen now.
#[derive(Debug, Default)]
pub(crate) struct PhonePrompt {
    page: Option<u64>,
}

impl PhonePrompt {
    pub(crate) fn of_page(page: u64) -> Self {
        Self { page: Some(page) }
    }

    /// The screens, unless this engine's page has been left: then it ends as it
    /// does when the person closes the screen.
    fn screens(&self) -> Result<Arc<Upcalls>> {
        if self
            .page
            .is_some_and(|page| crate::GENERATION.load(Ordering::SeqCst) != page)
        {
            return Err(left());
        }
        upcalls()
    }
}

impl SecretPrompt for PhonePrompt {
    fn show_and_confirm(
        &mut self,
        phrase: &str,
        positions: [usize; 3],
    ) -> Result<Option<Vec<Secret>>> {
        let typed = self.screens()?.confirm(phrase, positions)?.ok_or_else(left)?;
        Ok(Some(
            typed
                .split_whitespace()
                .map(|word| Zeroizing::new(word.to_owned()))
                .collect(),
        ))
    }

    fn new_passphrase(&mut self) -> Result<Secret> {
        self.screens()?
            .call("askNewPassphrase", "()Ljava/lang/String;", &[])?
            .ok_or_else(left)
    }

    fn passphrase(&mut self, purpose: &str) -> Result<Secret> {
        self.screens()?
            .call(
                "askPassphrase",
                "(Ljava/lang/String;)Ljava/lang/String;",
                &[purpose],
            )?
            .ok_or_else(left)
    }

    /// The words are read, not proved: a phone shows them and waits for Done,
    /// where the default would ask three back.
    fn show_words(&mut self, phrase: &str) -> Result<()> {
        self.screens()?.call("showWords", "(Ljava/lang/String;)V", &[phrase])?;
        Ok(())
    }

    fn recovery_phrase(&mut self) -> Result<Secret> {
        self.screens()?
            .call("askRecoveryPhrase", "()Ljava/lang/String;", &[])?
            .ok_or_else(left)
    }

    fn where_asked(&self) -> &'static str {
        "a screen of the app"
    }
}

/// The app's own service, the one `SyncService` is: started and stopped by the
/// app, never installed, and the passphrase kept behind its keystore.
#[derive(Debug, Default)]
pub(crate) struct PhoneService;

impl ServiceControl for PhoneService {
    /// Never read: the three methods that would use it are replaced below.
    fn passphrase_file(&self) -> PathBuf {
        PathBuf::new()
    }

    fn stored_passphrase(&self) -> Option<Secret> {
        upcalls()
            .ok()?
            .call("storedPassphrase", "()Ljava/lang/String;", &[])
            .ok()
            .flatten()
    }

    fn store_passphrase(&self, passphrase: &Secret) -> Result<String> {
        upcalls()?.call(
            "storePassphrase",
            "(Ljava/lang/String;)V",
            &[passphrase.as_str()],
        )?;
        Ok("the phone's keystore holds it".to_owned())
    }

    fn forget_passphrase(&self) -> Result<&'static str> {
        upcalls()?.call("forgetPassphrase", "()V", &[])?;
        Ok("the phone's keystore forgot it")
    }

    fn installed(&self) -> bool {
        true
    }

    fn install(&self, _tray: bool) -> Result<String> {
        Ok("the app is its own service".to_owned())
    }

    fn start(&self) -> Result<()> {
        upcalls()?.call("serviceStart", "()V", &[])?;
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        upcalls()?.call("serviceStop", "()V", &[])?;
        Ok(())
    }

    fn set_autostart(&self, _on: bool) -> Result<()> {
        Ok(())
    }

    fn log_hint(&self) -> String {
        "adb logcat -s itsanas".to_owned()
    }
}

/// What the app is told, and what it holds for the page.
#[derive(Debug, Default)]
pub(crate) struct PhoneHost;

impl Host for PhoneHost {
    fn event(&self, name: &str, value: &str) {
        if let Ok(up) = upcalls() {
            let _ = up.call(
                "event",
                "(Ljava/lang/String;Ljava/lang/String;)V",
                &[name, value],
            );
        }
    }

    fn value(&self, name: &str) -> Option<String> {
        let up = upcalls().ok()?;
        let said = up
            .call("value", "(Ljava/lang/String;)Ljava/lang/String;", &[name])
            .ok()??;
        Some(said.as_str().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An engine outlives its page. If it may still ask, its 24 words replace the
    /// ones on screen, and the person writes down or types back the wrong set.
    /// Sabotage: ask without looking at the page.
    #[test]
    fn red_team_an_engine_whose_page_was_left_asks_nothing() {
        let page = crate::GENERATION.fetch_add(1, Ordering::SeqCst);
        let said = PhonePrompt::of_page(page)
            .show_and_confirm("two words", [1, 2, 3])
            .err()
            .expect("a left page's engine had an answer")
            .to_string();
        assert!(
            said.contains("the screen was closed"),
            "a left page's engine went on to ask for its words: {said}"
        );
        let said = PhonePrompt::of_page(page)
            .new_passphrase()
            .err()
            .expect("a left page's engine had an answer")
            .to_string();
        assert!(
            said.contains("the screen was closed"),
            "a left page's engine went on to ask for a passphrase: {said}"
        );
        let said = PhonePrompt::default()
            .new_passphrase()
            .err()
            .expect("no screens exist in a test")
            .to_string();
        assert!(
            said.contains("not ready"),
            "the Unlock screen, which has no page, was refused its question: {said}"
        );
    }
}
