//! Each step's check and apply, apart from the engine that runs them.

use std::time::Duration;

use itsanas_crypto::MasterSecret;
use itsanas_node::node::Identity;
use zeroize::Zeroizing;

use super::{
    Account, Event, Setup, State, Step, answers::default_folder, secrets, service, verify,
    wait_until,
};
use crate::{
    config::{Config, format_size},
    error::{CliError, Result},
    node::Node,
};

/// How many times the words may be typed back wrong before setup stops.
pub(super) const CONFIRM_ATTEMPTS: usize = 3;

impl Setup<'_> {
    fn config(&self) -> Option<Config> {
        Config::load(&Node::config_path(&self.home)).ok()
    }

    /// The folder to keep in step: the answer, else what is configured, else
    /// one per node under the user's home.
    fn wanted_folder(&self) -> std::path::PathBuf {
        self.answers
            .folder
            .clone()
            .or_else(|| self.config().and_then(|config| config.folder))
            .unwrap_or_else(|| {
                default_folder(
                    &crate::config::user_home(),
                    self.answers.instance.as_deref(),
                )
            })
    }

    // -- Machine --------------------------------------------------------------

    pub(super) fn machine_state(&self) -> State {
        let folder = self.wanted_folder();
        if folder.is_dir() {
            State::Done(format!(
                "node in {}, folder {}",
                self.home.display(),
                folder.display()
            ))
        } else {
            State::Todo
        }
    }

    pub(super) fn apply_machine(&mut self) -> Result<String> {
        let folder = self.wanted_folder();
        std::fs::create_dir_all(&folder).map_err(|source| CliError::Io {
            path: folder.clone(),
            source,
        })?;
        Ok(format!(
            "node in {}, folder {} created",
            self.home.display(),
            folder.display()
        ))
    }

    // -- Account --------------------------------------------------------------

    pub(super) fn account_state(&self) -> Result<State> {
        if !Node::exists(&self.home) {
            return Ok(State::Todo);
        }
        let config = Config::load(&Node::config_path(&self.home))?;
        if let Some(wanted) = &self.answers.account
            && wanted.username() != config.username
        {
            // Never "fixed" by making the account asked for: that is a second
            // identity beside the first, which is the failure this check is
            // here to make impossible.
            return Err(CliError::Usage(format!(
                "{} already holds the account {:?}, not {:?}. A second account on this machine \
                 is a second instance: `itsanas --instance NAME setup`",
                self.home.display(),
                config.username,
                wanted.username()
            )));
        }
        Ok(State::Done(format!(
            "account {:?} is on this machine; its keys were not touched",
            config.username
        )))
    }

    pub(super) fn apply_account(&mut self) -> Result<String> {
        let Some(account) = self.answers.account.clone() else {
            return Err(CliError::Usage(
                "no account on this machine, and none in the answers: account = \"new\" or \
                 \"join\", with username = \"...\""
                    .to_owned(),
            ));
        };
        crate::refuse_stranded(
            self.home.clone(),
            &crate::config::user_home(),
            crate::config::other_user_home().as_deref(),
        )?;
        // A taken name is refused here, before a key is written: learnt at the
        // registration step instead, it left a whole account on this disk
        // under a name the network would never give it. An unreachable
        // coordinator does not stop the step -- registration says so later.
        if let (Account::New { username }, Some(address)) = (&account, self.wanted_coordinator())
            && let Ok(true) = (self.name_check)(&address, username)
        {
            return Err(CliError::Usage(format!(
                "the username {username:?} is already taken at {address}: choose another \
                 (nothing was written)"
            )));
        }
        let node = match &account {
            Account::New { username } => self.create_account(username)?,
            Account::Join {
                username,
                from: None,
            } => self.join_from_words(username)?,
            Account::Join {
                username,
                from: Some(address),
            } => {
                let asked_in = self.prompt.where_asked();
                self.emit(&Event::NeedsSecret {
                    step: Step::Account,
                    what: "the passphrase the recovery container was sealed with",
                    asked_in,
                });
                let secret = self.prompt.passphrase(
                    "The passphrase the recovery container was sealed with (that of the machine \
                     that lodged it):",
                )?;
                if self.answers.service {
                    // The same check new_passphrase makes, and for the same
                    // reason: said before the keystore is written under a
                    // passphrase the service's file could not hold, which
                    // would leave a node with no service on every re-run.
                    service::fits_service_file(std::env::consts::OS, &secret)?;
                }
                let node = crate::recover_from_coordinator(
                    &self.home,
                    username,
                    address,
                    self.answers.coordinator_device.as_deref(),
                    &secret,
                )?;
                self.passphrase = Some(secret);
                node
            }
        };
        Ok(format!(
            "account {:?} on this machine, as device {}",
            account.username(),
            node.store.device_id()
        ))
    }

    /// Show the words, have three typed back, and only then write the account.
    ///
    /// The order matters: a node written before the words are confirmed is a
    /// node a re-run finds "done", with words its owner never wrote down.
    fn create_account(&mut self, username: &str) -> Result<Node> {
        let master = MasterSecret::generate()?;
        let phrase = master.to_recovery_phrase()?;
        let mut confirmed = false;
        for attempt in 1..=CONFIRM_ATTEMPTS {
            let positions = secrets::random_positions()?;
            let asked_in = self.prompt.where_asked();
            self.emit(&Event::NeedsSecret {
                step: Step::Account,
                what: if attempt == 1 {
                    "the 24 recovery words, then three of them back"
                } else {
                    "the 24 recovery words again: a word typed back was not the one shown"
                },
                asked_in,
            });
            match self.prompt.show_and_confirm(&phrase, positions)? {
                None => break,
                Some(typed) if secrets::words_match(&phrase, &positions, &typed) => {
                    confirmed = true;
                    break;
                }
                Some(_) if attempt == CONFIRM_ATTEMPTS => {
                    return Err(CliError::Usage(format!(
                        "{CONFIRM_ATTEMPTS} times, the words typed back were not the ones shown; \
                         nothing was written. Run setup again, ready to keep the words"
                    )));
                }
                Some(_) => {}
            }
        }
        let passphrase = self.new_passphrase()?;
        let mut node = Node::restore(&self.home, &passphrase, username, &phrase)?;
        crate::settle_listen_port(&mut node)?;
        self.passphrase = Some(passphrase);
        if !confirmed {
            println!("(The words were not typed back: nobody was there to ask.)");
        }
        Ok(node)
    }

    fn new_passphrase(&mut self) -> Result<secrets::Secret> {
        let asked_in = self.prompt.where_asked();
        self.emit(&Event::NeedsSecret {
            step: Step::Account,
            what: "a passphrase for this machine's keys, twice",
            asked_in,
        });
        let passphrase = self.prompt.new_passphrase()?;
        if self.answers.service {
            // Said now, before the account is written under a passphrase the
            // service's file could not hold.
            service::fits_service_file(std::env::consts::OS, &passphrase)?;
        }
        Ok(passphrase)
    }

    fn join_from_words(&mut self, username: &str) -> Result<Node> {
        let asked_in = self.prompt.where_asked();
        self.emit(&Event::NeedsSecret {
            step: Step::Account,
            what: "the account's 24 recovery words",
            asked_in,
        });
        let typed = self.prompt.recovery_phrase()?;
        let words = Zeroizing::new(crate::phrase_words(&typed));
        // Checked before a passphrase is chosen: a typo found after that is a
        // second round of typing for nothing.
        MasterSecret::from_recovery_phrase(&words)?;
        let passphrase = self.new_passphrase()?;
        let mut node = Node::restore(&self.home, &passphrase, username, &words)?;
        crate::settle_listen_port(&mut node)?;
        self.passphrase = Some(passphrase);
        Ok(node)
    }

    // -- Secret ---------------------------------------------------------------

    pub(super) fn secret_state(&self) -> State {
        if !self.answers.service {
            return State::Skip(
                "no background service asked for, so no file needs the passphrase".to_owned(),
            );
        }
        let path = self.service.passphrase_file();
        match service::read_passphrase_file(&path) {
            // Opened, not merely present: a file left from before
            // `itsanas passphrase` changed it starts a daemon that cannot
            // unlock, which is a setup that is not done.
            Some(stored) if Identity::open(&self.home, &stored).is_ok() => {
                State::Done(format!("{} opens this machine's keys", path.display()))
            }
            _ => State::Todo,
        }
    }

    pub(super) fn apply_secret(&mut self) -> Result<String> {
        let passphrase = self.passphrase(Step::Secret)?;
        let path = self.service.passphrase_file();
        service::write_passphrase_file(&path, &passphrase)?;
        Ok(format!(
            "{} holds it, readable by this account alone (anything running as you can read it: \
             the trade every background service makes)",
            path.display()
        ))
    }

    // -- Registration ---------------------------------------------------------

    pub(super) fn wanted_coordinator(&self) -> Option<String> {
        self.answers
            .coordinator
            .clone()
            .or_else(|| self.config().and_then(|config| config.coordinator))
    }

    pub(super) fn registration_state(&self) -> Result<State> {
        let Some(wanted) = self.wanted_coordinator() else {
            return Ok(State::Skip(
                "no coordinator: machines on one network still find each other, and \
                 `itsanas coordinator HOST:PORT` reaches the others later"
                    .to_owned(),
            ));
        };
        let config = Config::load(&Node::config_path(&self.home))?;
        let device_kept = self
            .answers
            .coordinator_device
            .as_ref()
            .is_none_or(|device| config.coordinator_device.as_ref() == Some(device));
        let announce_kept = match &self.answers.announce {
            None => true,
            Some(announce) => crate::config::parse_announce(announce).ok() == config.announce,
        };
        let registered = crate::registered_with(&self.home);
        Ok(
            if config.coordinator.as_deref() == Some(wanted.as_str())
                && device_kept
                && announce_kept
                && registered.as_deref() == Some(wanted.as_str())
            {
                State::Done(format!("registered with {wanted}"))
            } else {
                State::Todo
            },
        )
    }

    pub(super) fn apply_registration(&mut self) -> Result<String> {
        crate::refuse_if_departed(&self.home)?;
        let wanted = self
            .wanted_coordinator()
            .ok_or_else(|| CliError::Usage("no coordinator to register with".to_owned()))?;
        let mut node = self.open_node(Step::Registration)?;
        let device = self
            .answers
            .coordinator_device
            .clone()
            .or_else(|| node.config.coordinator_device.clone());
        crate::apply_coordinator(&mut node.config, &wanted, device.as_deref())?;
        if let Some(announce) = &self.answers.announce {
            node.config.announce = Some(crate::config::parse_announce(announce)?);
        }
        node.save_config()?;
        let published = crate::register_and_announce(&node, self.answers.invite.as_deref())?;
        Ok(match published {
            Ok(address) => {
                format!("registered with {wanted}; this machine is published at {address}")
            }
            Err(why) => format!("registered with {wanted}; no address could be published: {why}"),
        })
    }

    // -- Pledge ---------------------------------------------------------------

    /// The bargain in one line, from the split this machine's configuration
    /// holds (`Split::DEFAULT` unless the network set a stricter one) --
    /// never a number written here, which is how documents drifted from the
    /// code once (`scripts/check-bargain.py`).
    fn bargain(config: &Config, pledge: u64) -> String {
        format!(
            "offers {} to the other members, which earns {} for your own files (split {}, own/network)",
            format_size(pledge),
            format_size(Node::allowed_for(config, pledge)),
            config.split
        )
    }

    pub(super) fn pledge_state(&self) -> Result<State> {
        let config = Config::load(&Node::config_path(&self.home))?;
        Ok(match self.answers.pledge {
            None if config.pledge_bytes > 0 => {
                State::Done(Self::bargain(&config, config.pledge_bytes))
            }
            None => State::Skip(
                "nothing offered yet, so this account has only the joining allowance; \
                 `itsanas pledge 10G` offers some later"
                    .to_owned(),
            ),
            Some(bytes) if bytes == config.pledge_bytes => {
                State::Done(Self::bargain(&config, bytes))
            }
            Some(_) => State::Todo,
        })
    }

    pub(super) fn apply_pledge(&mut self) -> Result<String> {
        let bytes = self.answers.pledge.unwrap_or_default();
        let mut node = self.open_node(Step::Pledge)?;
        let warning = crate::set_pledge(&mut node, bytes)?;
        let free = fs4::available_space(&self.home).map_or_else(
            |_| String::new(),
            |free| format!("; {} still free on this disk", format_size(free)),
        );
        Ok(format!(
            "{}{free}{}",
            Self::bargain(&node.config, bytes),
            warning.map_or_else(String::new, |warning| format!(". {warning}"))
        ))
    }

    // -- Folder ---------------------------------------------------------------

    pub(super) fn folder_state(&self) -> Result<State> {
        let config = Config::load(&Node::config_path(&self.home))?;
        let wanted = std::path::absolute(self.wanted_folder()).map_err(|source| CliError::Io {
            path: self.wanted_folder(),
            source,
        })?;
        Ok(if config.folder.as_deref() == Some(wanted.as_path()) {
            State::Done(format!("{} is kept in step", wanted.display()))
        } else {
            State::Todo
        })
    }

    pub(super) fn apply_folder(&mut self) -> Result<String> {
        let wanted = self.wanted_folder();
        let mut node = self.open_node(Step::Folder)?;
        let (absolute, first_pass) = crate::set_folder(&mut node, &wanted)?;
        Ok(format!(
            "{} is kept in step; {first_pass}",
            absolute.display()
        ))
    }

    // -- Updates --------------------------------------------------------------

    /// Said the same way in every state, so a person reads what it means.
    fn updates_said(updates: crate::config::Updates) -> String {
        match updates {
            crate::config::Updates::Auto => {
                "new signed releases are installed by themselves, once a day".to_owned()
            }
            crate::config::Updates::Notify => {
                "a new release is announced in `itsanas status`; `itsanas update` installs it"
                    .to_owned()
            }
            crate::config::Updates::Off => "this machine never looks for a new release".to_owned(),
        }
    }

    pub(super) fn updates_state(&self) -> Result<State> {
        let config = Config::load(&Node::config_path(&self.home))?;
        Ok(match self.answers.updates {
            Some(wanted) if wanted != config.updates => State::Todo,
            _ => State::Done(Self::updates_said(config.updates)),
        })
    }

    /// The configuration file alone, not the keys: the daemon reads this
    /// setting again before each daily look, so nothing needs a restart.
    pub(super) fn apply_updates(&mut self) -> Result<String> {
        let path = Node::config_path(&self.home);
        let mut config = Config::load(&path)?;
        config.updates = self.answers.updates.unwrap_or(config.updates);
        config.save(&path)?;
        Ok(Self::updates_said(config.updates))
    }

    // -- Connectivity ---------------------------------------------------------

    pub(super) fn connectivity_state(&self) -> Result<State> {
        let config = Config::load(&Node::config_path(&self.home))?;
        Ok(if config.coordinator.is_some() {
            // Never "done": reachability is a fact about now, not a setting.
            State::Todo
        } else {
            State::Skip("no coordinator to reach".to_owned())
        })
    }

    pub(super) fn apply_connectivity(&mut self) -> Result<String> {
        let passphrase = self.passphrase(Step::Connectivity)?;
        let identity = Identity::open(&self.home, &passphrase)?;
        let check = crate::network_check(&identity);
        let address = check.coordinator.clone().unwrap_or_default();
        let peers = check.outbound.map_err(|why| {
            CliError::Usage(format!(
                "the coordinator at {address} could not be reached: {why}"
            ))
        })?;
        let reached = format!(
            "{address} answered; {} other machine(s) of this account have published an address",
            peers.elsewhere
        );
        // The dial-back proves something only while a daemon listens: before
        // the service runs, it is the final check's to ask.
        let listening = itsanas_store::Store::is_locked(Node::store_path(&self.home));
        let Some(inbound) = check.inbound.filter(|_| listening) else {
            return Ok(format!(
                "{reached}; the dial-back is checked once the service listens"
            ));
        };
        let finding = verify::reach_finding(&inbound, identity.config.announce.as_deref());
        if finding.verdict == verify::Verdict::Failed {
            return Err(CliError::Usage(format!(
                "{}. {}",
                finding.detail, finding.remedy
            )));
        }
        Ok(format!("{reached}; {}", finding.detail))
    }

    // -- Service --------------------------------------------------------------

    pub(super) fn service_state(&self) -> State {
        if !self.answers.service {
            return State::Skip(
                "not asked for: start the node with `itsanas daemon`, or run setup again with \
                 service = true"
                    .to_owned(),
            );
        }
        let running = itsanas_store::Store::is_locked(Node::store_path(&self.home));
        if running && self.service.installed() {
            State::Done(format!("it runs (log: {})", self.service.log_hint()))
        } else {
            State::Todo
        }
    }

    pub(super) fn apply_service(&mut self) -> Result<String> {
        self.refuse_a_home_the_service_would_not_run()?;
        let store = Node::store_path(&self.home);
        if itsanas_store::Store::is_locked(&store) && !self.service.installed() {
            return Err(CliError::Usage(
                "a daemon started by hand holds this node; stop it (Ctrl+C where it runs) and \
                 run setup again, which then installs the service"
                    .to_owned(),
            ));
        }
        let said = self.service.install(self.answers.tray)?;
        self.service.set_autostart(true)?;
        self.service.start()?;
        self.started_service = true;
        if !wait_until(Duration::from_secs(30), || {
            itsanas_store::Store::is_locked(&store)
        }) {
            return Err(CliError::Usage(format!(
                "{said} is installed and started, and no daemon took this node within 30 s"
            )));
        }
        Ok(format!("{said}; the daemon runs"))
    }

    /// The service names no home of its own: the task, the unit and the plist
    /// run the node `--instance NAME` (or no instance) resolves to, as the
    /// installers' do. Set up with `--home` somewhere else, it would start a
    /// daemon on another node -- or on none -- and call that a success.
    fn refuse_a_home_the_service_would_not_run(&self) -> Result<()> {
        let served = match self.answers.instance.as_deref() {
            Some(name) => crate::config::instance_home(name)?,
            None => crate::config::unnamed_home(&crate::config::user_home())?,
        };
        let same = |a: &std::path::Path, b: &std::path::Path| match (
            std::path::absolute(a),
            std::path::absolute(b),
        ) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        if same(&self.home, &served) {
            return Ok(());
        }
        Err(CliError::Usage(format!(
            concat!(
                "the background service runs the node in {}, and this one is in {}; run setup ",
                "with `--instance NAME` instead of `--home`, or set service = false and start ",
                "this node with `itsanas --home {} daemon`"
            ),
            served.display(),
            self.home.display(),
            self.home.display()
        )))
    }

    // -- Verify ---------------------------------------------------------------

    pub(super) fn apply_verify(&mut self) -> Result<String> {
        let config = Config::load(&Node::config_path(&self.home))?;
        let identity = if config.coordinator.is_some() {
            let passphrase = self.passphrase(Step::Verify)?;
            Some(Identity::open(&self.home, &passphrase)?)
        } else {
            None
        };
        let report = verify::run(&verify::Inputs {
            home: &self.home,
            identity: identity.as_ref(),
            daemon_expected: self.answers.service,
            folder: config.folder.as_deref(),
            deadline: self.answers.verify_for,
            log_hint: self.service.log_hint(),
        });
        let summary = report.summary();
        let failures: Vec<String> = report
            .failures()
            .iter()
            .map(|finding| format!("{}: {}", finding.what, finding.detail))
            .collect();
        self.report = Some(report);
        if failures.is_empty() {
            Ok(summary)
        } else {
            Err(CliError::Usage(format!(
                "{summary}; {}",
                failures.join("; ")
            )))
        }
    }
}
