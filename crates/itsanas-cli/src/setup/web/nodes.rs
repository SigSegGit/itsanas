//! Where the page's account calls (devices, machines to sync with, removing an
//! account) get the node.
//!
//! A phone's app holds the node open in its process, so the page asks it for
//! that one. A computer's background service holds the store instead, and only
//! one process may: the page then stops the service for the length of the call,
//! opens the node with the passphrase the service keeps (or one typed in a
//! native window), and gives it back. Either way the page never sees a key.

use std::{
    path::PathBuf,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use itsanas_node::Node;

use super::{MakePrompt, MakeService};
use crate::{
    failure::Failure,
    setup::{sign, wait_until},
};

/// The slot a node is held in, shared with `account::logout_into`.
pub type Slot = Mutex<Option<std::sync::Arc<Node>>>;

/// The node behind the page.
pub trait Nodes: Send + Sync {
    /// The slot the node is taken from.
    fn slot(&self) -> &Slot;
    /// Make sure the node is in the slot: a shell that holds it open has
    /// nothing to do; a computer opens it.
    fn open(&self) -> Result<(), Failure>;
    /// The call is over and the node stays: a computer closes it and lets the
    /// service run again.
    fn release(&self) {}
    /// The node is leaving for good: the service must not be started again.
    fn keep_stopped(&self) {}
}

/// A shell that keeps its node open in a slot of its own.
#[derive(Debug)]
pub struct Held(pub &'static Slot);

impl Nodes for Held {
    fn slot(&self) -> &Slot {
        self.0
    }

    fn open(&self) -> Result<(), Failure> {
        let held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if held.is_some() {
            Ok(())
        } else {
            Err(Failure::Closed)
        }
    }
}

/// A computer: the node is opened for the length of a call.
pub(super) struct Opened {
    home: PathBuf,
    service: MakeService,
    prompt: MakePrompt,
    slot: Slot,
    /// The service was running when the node was opened, so it is started
    /// again when the call is over.
    restart: AtomicBool,
}

impl std::fmt::Debug for Opened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opened")
            .field("home", &self.home)
            .finish_non_exhaustive()
    }
}

impl Opened {
    pub(super) fn new(home: PathBuf, service: MakeService, prompt: MakePrompt) -> Self {
        Self {
            home,
            service,
            prompt,
            slot: Mutex::new(None),
            restart: AtomicBool::new(false),
        }
    }
}

impl Nodes for Opened {
    fn slot(&self) -> &Slot {
        &self.slot
    }

    fn open(&self) -> Result<(), Failure> {
        let mut held = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if held.is_some() {
            return Ok(());
        }
        if !Node::exists(&self.home) {
            return Err(Failure::Usage(format!(
                "there is no account at {} yet",
                self.home.display()
            )));
        }
        let service = (self.service)();
        let store = Node::store_path(&self.home);
        if itsanas_store::Store::is_locked(&store) {
            service.stop()?;
            if !wait_until(Duration::from_secs(20), || {
                !itsanas_store::Store::is_locked(&store)
            }) && !crate::lifecycle::stop_and_wait(&self.home)
            {
                let _ = service.start();
                return Err(Failure::Usage(
                    "the background service still holds this account; nothing was changed. \
                     Try again in a moment."
                        .to_owned(),
                ));
            }
            self.restart.store(service.installed(), Ordering::SeqCst);
        }
        let sealed = || {
            crate::setup::unlock::is_on(&self.home)
                .then(|| {
                    crate::setup::unlock::opened(&self.home, crate::setup::unlock::platform_gate)
                        .ok()
                })
                .flatten()
        };
        let passphrase = match service.stored_passphrase().or_else(sealed) {
            Some(stored) => stored,
            None => (self.prompt)()?.passphrase(&sign::account_purpose(
                &self.home,
                "to look at this account",
            ))?,
        };
        match Node::open(&self.home, &passphrase) {
            Ok(node) => {
                *held = Some(std::sync::Arc::new(node));
                Ok(())
            }
            Err(error) => {
                drop(held);
                self.release();
                Err(error.into())
            }
        }
    }

    fn release(&self) {
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = None;
        if self.restart.swap(false, Ordering::SeqCst) {
            let _ = (self.service)().start();
        }
    }

    fn keep_stopped(&self) {
        self.restart.store(false, Ordering::SeqCst);
    }
}
