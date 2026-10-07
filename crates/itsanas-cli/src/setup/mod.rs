//! `itsanas setup`: the steps that make a machine a working node, as data.
//!
//! HANDOVER §8 0w (2). Nicolas asked for a setup "friendly enough for testers
//! who are not us", and **resilient**: a setup that fails at step six and is
//! run again must not redo steps one to five -- above all not make a second
//! identity, or overwrite the keystore that holds the only copy of a
//! passphrase-sealed account. So every [`Step`] has three parts:
//!
//! - **check**: is it done already? Decided from the node's home and its
//!   configuration, never by asking the person again;
//! - **apply**: do it, reusing the functions the CLI commands already use
//!   (`init`, `login`, `register`, `pledge`, `folder`, `doctor`'s network half);
//! - **remedy**: when it fails, the one thing to do, in a sentence.
//!
//! # The API, for the web page of 0w (4)
//!
//! Build a [`Setup`] with the home, the [`Answers`], a [`SecretPrompt`] (the
//! native windows of [`secrets`]: the words never go through a browser), a
//! [`ServiceControl`] ([`service::Platform`] on a real machine) and a callback
//! that receives every [`Event`]. [`Setup::run`] walks [`STEPS`] in order,
//! stops at the first failure, and returns an [`Outcome`] carrying the
//! [`verify::Report`]. Events carry **no secret**: the test
//! `red_team_no_event_and_no_log_line_carries_a_recovery_word` holds every
//! one, and the `setup.log` they are also written to, against the 24 words.
//! [`plan`] says, without changing anything, which steps are done -- what a
//! page shows before the person presses a button.

pub(crate) mod answers;
pub(crate) mod secrets;
pub(crate) mod service;
pub(crate) mod sign;
mod steps;
pub(crate) mod verify;
pub(crate) mod web;

use std::{
    io::Write as _,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(crate) use answers::{Account, Answers};
pub(crate) use secrets::{Secret, SecretPrompt};
pub(crate) use service::ServiceControl;

use crate::{
    error::{CliError, Result},
    node::Node,
};

/// One step of setting up a machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// The instance and the folder to keep in step.
    Machine,
    /// A new account, or one joined from its words or a coordinator.
    Account,
    /// The file the background service reads the passphrase from.
    Secret,
    /// The coordinator, and this device enrolled with it.
    Registration,
    /// Space offered to others, against the free disk.
    Pledge,
    /// The synced folder set in the configuration.
    Folder,
    /// What the daemon does about a newer release: auto, notify, off.
    Updates,
    /// The coordinator reached, and asked to dial back.
    Connectivity,
    /// The background service installed and running, and the tray.
    Service,
    /// It all works, seen from outside.
    Verify,
}

/// Every step, in the order they run.
pub(crate) const STEPS: [Step; 10] = [
    Step::Machine,
    Step::Account,
    Step::Secret,
    Step::Registration,
    Step::Pledge,
    Step::Folder,
    Step::Updates,
    Step::Connectivity,
    Step::Service,
    Step::Verify,
];

impl Step {
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Machine => "This machine",
            Self::Account => "Your account",
            Self::Secret => "The passphrase for the background service",
            Self::Registration => "The coordinator",
            Self::Pledge => "Space offered to others",
            Self::Folder => "Your folder",
            Self::Updates => "Updates",
            Self::Connectivity => "Connectivity",
            Self::Service => "The background service",
            Self::Verify => "Does it work?",
        }
    }

    pub(crate) fn number(self) -> usize {
        STEPS.iter().position(|step| *step == self).unwrap_or(0) + 1
    }
}

/// What a person is told, as it happens. Never holds a secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Started(Step),
    /// Found done; nothing was changed.
    AlreadyDone {
        step: Step,
        detail: String,
    },
    Done {
        step: Step,
        detail: String,
    },
    Skipped {
        step: Step,
        why: String,
    },
    /// A secret is about to be asked, and where (a window, the terminal).
    NeedsSecret {
        step: Step,
        what: &'static str,
        asked_in: &'static str,
    },
    Failed {
        step: Step,
        error: String,
        remedy: String,
    },
}

impl Event {
    /// One line (or two) for a terminal or a log.
    pub(crate) fn line(&self) -> String {
        match self {
            Self::Started(step) => format!("[{}/{}] {}", step.number(), STEPS.len(), step.title()),
            Self::AlreadyDone { detail, .. } => format!("      already done: {detail}"),
            Self::Done { detail, .. } => format!("      done: {detail}"),
            Self::Skipped { why, .. } => format!("      skipped: {why}"),
            Self::NeedsSecret { what, asked_in, .. } => {
                format!("      asking for {what} in {asked_in}")
            }
            Self::Failed { error, remedy, .. } => {
                format!("      FAILED: {error}\n      what to do: {remedy}")
            }
        }
    }
}

/// How a run ended.
#[derive(Debug)]
pub(crate) struct Outcome {
    /// The step that failed, if one did; the steps after it did not run.
    pub(crate) failed: Option<Step>,
    /// The final check's findings, if it ran.
    pub(crate) report: Option<verify::Report>,
}

/// Whether a step is done, and if not, whether it applies.
#[derive(Debug)]
enum State {
    Done(String),
    Todo,
    Skip(String),
}

/// The file in a node's home where setup writes what it did, one event a
/// line: the record a person sends when it went wrong. Holds no secret.
pub(crate) const SETUP_LOG: &str = "setup.log";

/// One run of setup on one home.
pub(crate) struct Setup<'a> {
    home: PathBuf,
    answers: Answers,
    prompt: &'a mut dyn SecretPrompt,
    service: &'a dyn ServiceControl,
    events: &'a mut dyn FnMut(&Event),
    /// Held once typed or read, so one run asks at most once.
    passphrase: Option<Secret>,
    /// Setup stopped the service to open the store; it owes a start.
    stopped_service: bool,
    started_service: bool,
    report: Option<verify::Report>,
}

impl std::fmt::Debug for Setup<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Setup")
            .field("home", &self.home)
            .field("answers", &self.answers)
            .finish_non_exhaustive()
    }
}

impl<'a> Setup<'a> {
    pub(crate) fn new(
        home: &Path,
        answers: Answers,
        prompt: &'a mut dyn SecretPrompt,
        service: &'a dyn ServiceControl,
        events: &'a mut dyn FnMut(&Event),
    ) -> Self {
        Self {
            home: home.to_owned(),
            answers,
            prompt,
            service,
            events,
            passphrase: None,
            stopped_service: false,
            started_service: false,
            report: None,
        }
    }

    /// Every step in order, stopping at the first that fails.
    pub(crate) fn run(mut self) -> Outcome {
        let mut failed = None;
        for step in STEPS {
            self.emit(&Event::Started(step));
            let result = match self.check(step) {
                Ok(State::Done(detail)) => Ok(Event::AlreadyDone { step, detail }),
                Ok(State::Skip(why)) => Ok(Event::Skipped { step, why }),
                Ok(State::Todo) => self.apply(step).map(|detail| Event::Done { step, detail }),
                Err(error) => Err(error),
            };
            match result {
                Ok(event) => self.emit(&event),
                Err(error) => {
                    let remedy = self.remedy(step, &error);
                    self.emit(&Event::Failed {
                        step,
                        error: error.to_string(),
                        remedy,
                    });
                    failed = Some(step);
                    break;
                }
            }
        }
        // The state it was found in: a service setup stopped to change a
        // setting is started again, whatever happened after.
        if self.stopped_service && !self.started_service {
            let _ = self.service.start();
        }
        Outcome {
            failed,
            report: self.report,
        }
    }

    fn emit(&mut self, event: &Event) {
        (self.events)(event);
        // Only into a home that exists: before the account step there is
        // none, and creating one here would make an empty "node" that
        // `init` then refuses.
        if self.home.is_dir()
            && let Ok(mut log) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.home.join(SETUP_LOG))
        {
            let _ = writeln!(log, "{} {}", itsanas_discover::now_unix(), event.line());
        }
    }

    fn check(&mut self, step: Step) -> Result<State> {
        match step {
            Step::Machine => Ok(self.machine_state()),
            Step::Account => self.account_state(),
            Step::Secret => Ok(self.secret_state()),
            Step::Registration => self.registration_state(),
            Step::Pledge => self.pledge_state(),
            Step::Folder => self.folder_state(),
            Step::Updates => self.updates_state(),
            Step::Connectivity => self.connectivity_state(),
            Step::Service => Ok(self.service_state()),
            Step::Verify => Ok(State::Todo),
        }
    }

    fn apply(&mut self, step: Step) -> Result<String> {
        match step {
            Step::Machine => self.apply_machine(),
            Step::Account => self.apply_account(),
            Step::Secret => self.apply_secret(),
            Step::Registration => self.apply_registration(),
            Step::Pledge => self.apply_pledge(),
            Step::Folder => self.apply_folder(),
            Step::Updates => self.apply_updates(),
            Step::Connectivity => self.apply_connectivity(),
            Step::Service => self.apply_service(),
            Step::Verify => self.apply_verify(),
        }
    }

    /// The one thing to do when `step` failed with `error`.
    fn remedy(&self, step: Step, error: &CliError) -> String {
        match (step, error) {
            (_, CliError::Unlock) => {
                "that passphrase does not open this machine's keys; run setup again with the \
                 right one"
                    .to_owned()
            }
            (Step::Account, CliError::NodeHomeEmpty(home)) => format!(
                "{} is an empty directory -- a disk that is not mounted looks like this; mount \
                 it, or remove the directory, and run setup again",
                home.display()
            ),
            (Step::Account, _) => "nothing was written; correct what the line above says and \
                                   run setup again"
                .to_owned(),
            (Step::Machine | Step::Folder, _) => {
                "choose a folder you can write to (folder = ... in the answers) and run setup \
                 again"
                    .to_owned()
            }
            (Step::Secret, _) => format!(
                "check that you can write in {} and run setup again",
                self.service
                    .passphrase_file()
                    .parent()
                    .map_or_else(String::new, |dir| dir.display().to_string())
            ),
            (Step::Registration, _) => {
                "check the coordinator's address and that it runs; if it admits members by \
                 invitation, ask one for a code (`itsanas invite`) and give it as invite = ..."
                    .to_owned()
            }
            (Step::Pledge, _) => {
                "offer less: `itsanas space` shows what this disk can give".to_owned()
            }
            (Step::Connectivity, _) => {
                "a name that does not resolve is DNS, a refused connection a closed port, a \
                 timeout usually a firewall; fix that and run setup again"
                    .to_owned()
            }
            (Step::Updates, _) => {
                "check that you can write in this node's configuration file and run setup again"
                    .to_owned()
            }
            (Step::Service, _) => format!(
                "read {}; or start the node by hand with `itsanas daemon`",
                self.service.log_hint()
            ),
            (Step::Verify, _) => self.report.as_ref().map_or_else(
                || "run setup again".to_owned(),
                |report| {
                    report
                        .failures()
                        .iter()
                        .map(|finding| finding.remedy.clone())
                        .collect::<Vec<_>>()
                        .join("; ")
                },
            ),
        }
    }

    /// The passphrase, asked at most once per run -- and not at all when the
    /// service's own file already opens the keys, which is what lets a run on
    /// a working machine finish without a single question.
    fn passphrase(&mut self, step: Step) -> Result<Secret> {
        if let Some(held) = &self.passphrase {
            return Ok(held.clone());
        }
        if let Some(stored) = service::read_passphrase_file(&self.service.passphrase_file())
            && itsanas_node::node::Identity::open(&self.home, &stored).is_ok()
        {
            self.passphrase = Some(stored.clone());
            return Ok(stored);
        }
        self.emit(&Event::NeedsSecret {
            step,
            what: "this machine's passphrase",
            asked_in: self.prompt.where_asked(),
        });
        let typed = self
            .prompt
            .passphrase("This machine's passphrase, the one that unlocks its keys:")?;
        itsanas_node::node::Identity::open(&self.home, &typed)?;
        self.passphrase = Some(typed.clone());
        Ok(typed)
    }

    /// Open the node, stopping the background service first if it holds the
    /// store: one process at a time may (HANDOVER §9).
    fn open_node(&mut self, step: Step) -> Result<Node> {
        let store = Node::store_path(&self.home);
        if itsanas_store::Store::is_locked(&store) {
            if !self.service.installed() {
                return Err(CliError::Usage(
                    "a daemon started by hand holds this node; stop it (Ctrl+C where it runs) \
                     and run setup again"
                        .to_owned(),
                ));
            }
            self.service.stop()?;
            self.stopped_service = true;
            if !wait_until(Duration::from_secs(20), || {
                !itsanas_store::Store::is_locked(&store)
            }) {
                return Err(CliError::Usage(
                    "the background service was asked to stop and still holds the node after 20 s"
                        .to_owned(),
                ));
            }
        }
        let passphrase = self.passphrase(step)?;
        Node::open(&self.home, &passphrase)
    }
}

/// Poll `done` every quarter second until it holds or `limit` passes.
pub(crate) fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    loop {
        if done() {
            return true;
        }
        if started.elapsed() >= limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Which steps are done on `home`, without changing anything or asking
/// anything: what a page shows before the person presses a button.
pub(crate) fn plan(
    home: &Path,
    answers: &Answers,
    service: &dyn ServiceControl,
) -> Vec<(Step, String)> {
    struct Silent;
    impl SecretPrompt for Silent {
        fn show_and_confirm(&mut self, _: &str, _: [usize; 3]) -> Result<Option<Vec<Secret>>> {
            Err(CliError::Usage("not asked while planning".to_owned()))
        }
        fn new_passphrase(&mut self) -> Result<Secret> {
            self.passphrase("")
        }
        fn passphrase(&mut self, _: &str) -> Result<Secret> {
            Err(CliError::Usage("not asked while planning".to_owned()))
        }
        fn recovery_phrase(&mut self) -> Result<Secret> {
            self.passphrase("")
        }
        fn where_asked(&self) -> &'static str {
            "nowhere"
        }
    }
    let mut silent = Silent;
    let mut ignore = |_: &Event| {};
    let mut setup = Setup::new(home, answers.clone(), &mut silent, service, &mut ignore);
    STEPS
        .iter()
        .map(|&step| {
            let said = match setup.check(step) {
                Ok(State::Done(detail)) => format!("done: {detail}"),
                Ok(State::Skip(why)) => format!("skipped: {why}"),
                Ok(State::Todo) => "to do".to_owned(),
                Err(error) => format!("cannot tell: {error}"),
            };
            (step, said)
        })
        .collect()
}

/// `itsanas setup`, from the command line.
///
/// `--answers FILE` runs unattended; `--text` asks in this terminal; without
/// either, a desktop gets the web page of [`web`], and a session with no
/// desktop (SSH, no display) the terminal, said as it happens.
pub(crate) fn command(
    home: &Path,
    instance: Option<&str>,
    text: bool,
    answers_file: Option<&Path>,
    phrase_file: Option<PathBuf>,
) -> Result<()> {
    if answers_file.is_none() && !text {
        if web::desktop_here() {
            return web::wizard(home, instance);
        }
        println!(
            "No desktop here (an SSH session, or no display): asking in this terminal instead. \
             `itsanas setup --text` does this on purpose."
        );
        println!();
    }
    let platform = service::Platform::of_this_machine(home, instance);
    if answers_file.is_none() && crate::node::Node::exists(home) {
        // Said before any question, so a person re-running setup sees that
        // what is done stays done and only the rest will be asked.
        let found = Answers {
            instance: instance.map(str::to_owned),
            ..Answers::default()
        };
        println!("This machine so far:");
        for (step, said) in plan(home, &found, &platform) {
            println!("  {:<44} {said}", step.title());
        }
        println!();
    }
    let answers = match answers_file {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|source| CliError::Io {
                path: path.to_owned(),
                source,
            })?;
            let mut answers = answers::parse(&text, &crate::config::user_home())?;
            answers.instance = instance.map(str::to_owned);
            answers
        }
        None => answers::ask_in_terminal(home, instance)?,
    };
    let mut prompt: Box<dyn SecretPrompt> = if answers_file.is_some() {
        Box::new(secrets::Unattended { phrase_file })
    } else {
        Box::new(secrets::Native::new(secrets::choose()?))
    };
    let mut print = |event: &Event| println!("{}", event.line());
    let outcome = Setup::new(home, answers, prompt.as_mut(), &platform, &mut print).run();
    println!();
    if let Some(report) = &outcome.report {
        for finding in &report.findings {
            println!("  {}", finding.line());
        }
        println!();
    }
    match outcome.failed {
        None => {
            println!(
                "This machine is set up. Run `itsanas setup` again at any time: it checks each \
                 step and redoes only what is missing."
            );
            Ok(())
        }
        Some(step) => Err(CliError::Usage(format!(
            "setup stopped at step {} ({}). Fix what it says above, then run `itsanas setup` \
             again: the steps already done are kept and not asked again.",
            step.number(),
            step.title()
        ))),
    }
}

#[cfg(test)]
mod tests;
