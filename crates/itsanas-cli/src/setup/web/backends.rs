//! What the page's server is given to act with: the windows, the service
//! manager, the node, the drives and the copy, each replaceable by a test or
//! a shell.

use std::{path::Path, sync::Arc, time::Duration};

use super::{
    super::{SecretPrompt, ServiceControl, secrets, service},
    account_actions::start_another,
    features::{Features, Host},
    nodes::{self, Nodes},
};
use crate::error::Result;

/// Makes the prompt of a run (`Backends::prompt`).
pub type MakePrompt = Box<dyn Fn() -> Result<Box<dyn SecretPrompt>> + Send + Sync>;
/// Makes the service control of a run (`Backends::service`).
pub type MakeService = Box<dyn Fn() -> Box<dyn ServiceControl> + Send + Sync>;

/// Where the engine asks for secrets and which service it drives, made fresh
/// for each run (the engine runs on a thread of its own, and neither needs to
/// cross threads that way). Tests pass stand-ins.
pub struct Backends {
    pub(crate) prompt: MakePrompt,
    pub(crate) service: MakeService,
    /// Is a username taken at a coordinator ([`super::super::name_taken`]).
    pub(crate) name: super::super::NameCheck,
    /// Whose account a username is, at a coordinator ([`super::super::OwnerCheck`]).
    pub(crate) owner: super::super::OwnerCheck,
    /// The largest pledge of the machines of the account that holds a username.
    pub(crate) pledge: fn(&str, &str) -> Option<u64>,
    /// Start the setup of another account, as instance `name`, in a page of
    /// its own ([`start_another`]).
    pub(crate) another: fn(&str) -> std::result::Result<(), String>,
    /// The USB keys and external disks plugged in ([`crate::drives::candidates`]).
    pub(crate) drives: ListDrives,
    /// What this release's programs take on a key, asked once, in the
    /// background: it is a download of the signed manifest.
    pub(crate) programs: fn() -> Option<u64>,
    /// Start `copy-to-key DIR --detach` for the home; what tells whether it
    /// still runs ([`crate::copy_to_key::start_detached`]).
    pub(crate) copy: StartCopy,
    /// What this shell has; the page hides the rest and the routes refuse it.
    pub(crate) features: Features,
    /// Whether this machine is a phone (see [`Answers::phone`]).
    pub(crate) phone: bool,
    /// A coordinator the person cannot change: its address and the device it
    /// is pinned to. The form's `coordinator` is then ignored, and the name
    /// check and the pin use this one -- a phone offers no way to name another
    /// network, which is what keeps the official one's pin in force.
    pub(crate) fixed_coordinator: Option<(String, Option<String>)>,
    /// Told what the page changed that the shell also draws.
    pub(crate) host: Arc<dyn Host>,
    /// The node behind the account calls: devices, machines to sync with,
    /// removing the account ([`Nodes`]).
    pub(crate) nodes: Arc<dyn Nodes>,
}

pub(super) type ListDrives = Box<dyn Fn() -> Vec<crate::drives::Candidate> + Send + Sync>;
/// Whether a started copy's process still runs; asked under a lock.
pub(crate) type Running = Box<dyn FnMut() -> bool + Send>;
pub(super) type StartCopy =
    Box<dyn Fn(&Path, &Path) -> std::result::Result<Running, String> + Send + Sync>;

/// A copy whose progress file has not changed for this long is a copy that
/// died without a word: the page offers to start another.
pub(super) const COPY_STALE: Duration = Duration::from_secs(30 * 60);

/// How long a copy started here still counts as running once its child
/// ended without a status written since ([`start_copy`]).
pub(super) const SILENT_CHILD: Duration = Duration::from_secs(2 * 60);

impl std::fmt::Debug for Backends {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Backends")
            .field("features", &self.features)
            .field("phone", &self.phone)
            .field("fixed_coordinator", &self.fixed_coordinator)
            .finish_non_exhaustive()
    }
}

impl Backends {
    /// The backends of a phone. The shell supplies what only it has: where a
    /// secret is asked, the app's own service, and what it is told of the page's
    /// changes. The rest is what a phone offers: the one network it was built
    /// for (`coordinator`, its address and the device it is pinned to), no second
    /// account, no disk to move to, no key to copy to.
    #[must_use]
    pub fn of_a_phone(
        prompt: MakePrompt,
        service: MakeService,
        host: Arc<dyn Host>,
        nodes: Arc<dyn Nodes>,
        coordinator: (String, Option<String>),
    ) -> Self {
        Self {
            prompt,
            service,
            name: super::super::name_taken,
            owner: itsanas_node::coordinator::name_owner,
            pledge: |address, username| {
                itsanas_node::coordinator::account_pledge(address, username)
                    .ok()
                    .flatten()
            },
            another: |_| Err("a phone keeps one account".to_owned()),
            drives: Box::new(Vec::new),
            programs: || None,
            copy: Box::new(|_, _| Err("a phone has no USB key to copy to".to_owned())),
            features: Features::PHONE,
            phone: true,
            fixed_coordinator: Some(coordinator),
            host,
            nodes,
        }
    }

    /// What this phone offers, when it is not what every phone does.
    #[must_use]
    pub fn with_features(mut self, features: Features) -> Self {
        self.features = features;
        self
    }

    /// Whether this phone can unlock with a fingerprint or a face: the feature
    /// is the phone's to say, not the library's.
    #[must_use]
    pub fn with_biometric(mut self, available: bool) -> Self {
        self.features.biometric = available;
        self
    }

    pub(super) fn of_this_machine(home: &Path, instance: Option<&str>) -> Self {
        let native_prompt = || -> MakePrompt {
            Box::new(|| {
                let prompt: Box<dyn SecretPrompt> =
                    Box::new(secrets::Native::new(secrets::choose()?));
                Ok(prompt)
            })
        };
        let platform_service = {
            let home = home.to_owned();
            let instance = instance.map(str::to_owned);
            move || -> MakeService {
                let (home, instance) = (home.clone(), instance.clone());
                Box::new(move || {
                    let platform: Box<dyn ServiceControl> = Box::new(
                        service::Platform::of_this_machine(&home, instance.as_deref()),
                    );
                    platform
                })
            }
        };
        Self {
            nodes: Arc::new(nodes::Opened::new(
                home.to_owned(),
                platform_service(),
                native_prompt(),
            )),
            prompt: native_prompt(),
            service: platform_service(),
            name: super::super::name_taken,
            owner: itsanas_node::coordinator::name_owner,
            pledge: |address, username| {
                itsanas_node::coordinator::account_pledge(address, username)
                    .ok()
                    .flatten()
            },
            another: start_another,
            drives: Box::new(crate::drives::candidates),
            programs: || {
                let curl = crate::update::releases();
                crate::update::Updater::of_this_build(&curl)
                    .and_then(|updater| updater.release_bytes())
                    .ok()
            },
            copy: Box::new(|home, dir| {
                let mut child =
                    crate::copy_to_key::start_detached(home, dir).map_err(|e| e.to_string())?;
                let running: Running = Box::new(move || matches!(child.try_wait(), Ok(None)));
                Ok(running)
            }),
            features: Features::THIS_COMPUTER,
            phone: false,
            fixed_coordinator: None,
            host: Arc::new(super::features::ComputerUnlock {
                home: home.to_owned(),
                instance: instance.map(str::to_owned),
            }),
        }
    }
}
