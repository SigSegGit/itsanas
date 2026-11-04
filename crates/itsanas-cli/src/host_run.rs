//! When a shell's hosting loop runs: `itsanas_policy::hosting` decides, this
//! starts and stops the daemon's own loop to match. A desktop's runs until it is
//! stopped; a phone's conditions change under it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use itsanas_node::Node;
use itsanas_policy::{Conditions, HostFacts, HostPlan, HostWork, Scope, Settings};

use crate::daemon::{Hosting, host_until};

/// The loop while it runs.
#[derive(Debug)]
pub struct HostRun {
    work: HostWork,
    every: Duration,
    scope: Scope,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

/// This process's loop. A phone holds one node, so one loop; the page's sign-out
/// stops it without knowing which shell started it.
pub static CURRENT: Mutex<Option<HostRun>> = Mutex::new(None);

/// Stop this process's loop, and wait for it ([`stop`]).
pub fn stop_current() {
    stop(&mut CURRENT.lock().unwrap_or_else(PoisonError::into_inner));
}

/// Bring the loop in line with what the policy says now, and say what it said.
/// A loop that ended by itself (a port taken, a vault gone) is started again by
/// the next look.
pub fn tick(
    current: &mut Option<HostRun>,
    node: &Arc<Node>,
    conditions: Conditions,
    settings: Settings,
    local_network: bool,
) -> HostPlan {
    let facts = HostFacts {
        // Unreadable counts as holding: the cost of answering for nothing is a
        // listener, and of the other way a copy somebody counts on.
        still_holding: node.held_for_others().map_or(true, |held| held > 0),
        metered_used: 0,
    };
    let plan = itsanas_policy::hosting(conditions, settings, facts);
    let scope = itsanas_policy::plan(conditions, settings).scope;
    let unchanged = current.as_ref().is_some_and(|run| {
        !run.thread.is_finished()
            && run.work == plan.work
            && Some(run.every) == plan.every
            && run.scope == scope
    });
    if !unchanged {
        stop(current);
        if let Some(every) = plan.every.filter(|_| plan.work != HostWork::Nothing) {
            *current = Some(start(node, plan.work, every, scope, local_network));
        }
    }
    plan
}

fn start(
    node: &Arc<Node>,
    work: HostWork,
    every: Duration,
    scope: Scope,
    local_network: bool,
) -> HostRun {
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let (node, stop) = (Arc::clone(node), Arc::clone(&stop));
        std::thread::spawn(move || {
            let hosting = Hosting {
                every,
                scope,
                answers_only: work == HostWork::AnswersOnly,
                local_network,
                shutdown: &stop,
            };
            if let Err(error) = host_until(&node, &hosting) {
                eprintln!("itsanas: hosting did not start: {error}");
            }
        })
    };
    HostRun {
        work,
        every,
        scope,
        stop,
        thread,
    }
}

/// Stop the loop, and wait for it: the store stays open until it has ended, and
/// the next opening of this account would be refused. Bounded by what a session
/// may last.
pub fn stop(current: &mut Option<HostRun>) {
    if let Some(run) = current.take() {
        run.stop.store(true, Ordering::SeqCst);
        let _ = run.thread.join();
    }
}

/// Whether the loop is running.
#[must_use]
pub fn running(current: &Option<HostRun>) -> bool {
    current
        .as_ref()
        .is_some_and(|run| !run.thread.is_finished())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{phone, with_directory};
    use itsanas_net::PeerClient;
    use itsanas_net::session;
    use itsanas_policy::{Attention, Network, Power};
    use std::time::Instant;

    const PATIENCE: Duration = Duration::from_secs(40);

    fn on(power: Power) -> Conditions {
        Conditions {
            network: Network::Unmetered,
            power,
            attention: Attention::Background,
        }
    }

    fn owner_round(owner: &Node, address: &str) -> session::AuditReport {
        let mut client = PeerClient::connect(
            itsanas_net::AddressList(address),
            &owner.device,
            owner.store.owner(),
            None,
        )
        .expect("the phone answers on its listener");
        client
            .present_claim(owner.claim_bytes(itsanas_discover::now_unix()))
            .expect("the phone takes the claim");
        let _ = session::push(&owner.store, &mut client);
        session::audit(&owner.store, &mut client, session::CHALLENGES_PER_ROUND)
            .expect("the phone answers an audit")
    }

    /// A phone runs the daemon's loop: another account's owner dials it, stores
    /// a file's chunks on it and audits them, and on battery it answers the
    /// audit and takes in nothing more. What a phone has never been able to do.
    /// Sabotage: make `tick` start nothing; set `answers_only` to false; take
    /// `answers_only` out of `room_at`.
    #[test]
    fn red_team_a_phone_hosts_another_account_and_on_battery_only_answers() {
        with_directory(|coordinator, _| {
            let dir = tempfile::tempdir().expect("scratch");
            let (mut host, _) = phone(&dir.path().join("host"), coordinator, "hostphone", None);
            let (mut owner, _) = phone(&dir.path().join("owner"), coordinator, "theowner", None);
            // What earns it room on a host: an account offering nothing stores nothing.
            owner.config.pledge_bytes = 100 << 20;
            let port = std::net::TcpListener::bind("127.0.0.1:0")
                .and_then(|socket| socket.local_addr())
                .expect("a free port")
                .port();
            let address = format!("127.0.0.1:{port}");
            host.config.listen.clone_from(&address);
            host.config.pledge_bytes = 100 << 20;
            // Pinned, as a phone's always is: unpinned, a coordinator's word
            // about another account's device is not taken and it stores nothing.
            host.config.coordinator_device = Some(
                itsanas_crypto::DeviceKeys::from_seed(&itsanas_crypto::SecretBytes::new(
                    [0xC0; 32],
                ))
                .device_id()
                .to_string(),
            );
            let now = itsanas_discover::now_unix();
            itsanas_node::coordinator::register_with(&host, None, now).expect("host enrols");
            itsanas_node::coordinator::register_with(&owner, None, now).expect("owner enrols");
            owner
                .store
                .write_file("a.bin", &vec![7u8; 200_000])
                .expect("a file");
            let host = Arc::new(host);

            let mut run = None;
            let plan = tick(
                &mut run,
                &host,
                on(Power::Charging),
                Settings::default(),
                false,
            );
            assert_eq!(plan.work, HostWork::Everything);
            assert!(running(&run), "the policy said host and nothing started");

            let deadline = Instant::now() + PATIENCE;
            let mut held = 0;
            while held == 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(500));
                if let Ok(mut client) = PeerClient::connect(
                    itsanas_net::AddressList(address.as_str()),
                    &owner.device,
                    owner.store.owner(),
                    None,
                ) {
                    let _ = client.present_claim(owner.claim_bytes(itsanas_discover::now_unix()));
                    let _ = session::push(&owner.store, &mut client);
                }
                held = host.held_for_others().expect("read");
            }
            assert!(
                held > 0,
                "another account's chunks never reached the phone: it offers space for nothing"
            );

            let plan = tick(
                &mut run,
                &host,
                on(Power::OnBattery),
                Settings::default(),
                false,
            );
            assert_eq!(plan.work, HostWork::AnswersOnly);
            let audit = owner_round(&owner, &address);
            assert!(
                audit.asked > 0 && audit.failed == 0,
                "on battery a phone must still answer for what it holds: {audit:?}"
            );
            owner
                .store
                .write_file("b.bin", &vec![9u8; 200_000])
                .expect("another file");
            let before = host.held_for_others().expect("read");
            owner_round(&owner, &address);
            assert_eq!(
                host.held_for_others().expect("read"),
                before,
                "on battery a phone took in new data: the owner's mode was answers only"
            );

            stop(&mut run);
            assert!(!running(&run));
            assert!(
                std::net::TcpStream::connect(&address).is_err(),
                "the listener outlived the loop: a stopped phone still answers"
            );
        });
    }

    /// Signing out on the page ends the loop, whichever shell started it, or it
    /// would go on answering for an account that left. Sabotage: drop
    /// `stop_current` from `logout_into`.
    #[test]
    fn red_team_signing_out_stops_the_loop() {
        with_directory(|coordinator, _| {
            let dir = tempfile::tempdir().expect("scratch");
            let home = dir.path().join("host");
            let (mut host, _) = phone(&home, coordinator, "leaving", None);
            host.config.listen = "127.0.0.1:0".to_owned();
            host.config.pledge_bytes = 100 << 20;
            let host = Arc::new(host);
            let slot: crate::shell::Slot = Mutex::new(Some(Arc::clone(&host)));
            {
                let mut run = CURRENT.lock().expect("lock");
                tick(
                    &mut run,
                    &host,
                    on(Power::Charging),
                    Settings::default(),
                    false,
                );
                assert!(running(&run));
            }
            drop(host);
            let _ = crate::account::logout_into(&slot, &home, itsanas_discover::now_unix());
            assert!(
                !running(&CURRENT.lock().expect("lock")),
                "signed out, and the loop still answers for the account that left"
            );
        });
    }

    /// Off, with nothing held, nothing runs, and an earlier run is stopped.
    /// Sabotage: return early from `tick` on `HostWork::Nothing` without `stop`.
    #[test]
    fn red_team_a_personal_device_runs_no_loop() {
        with_directory(|coordinator, _| {
            let dir = tempfile::tempdir().expect("scratch");
            let (mut host, _) = phone(&dir.path().join("host"), coordinator, "personal", None);
            host.config.listen = "127.0.0.1:0".to_owned();
            let host = Arc::new(host);
            let mut run = None;
            tick(
                &mut run,
                &host,
                on(Power::Charging),
                Settings::default(),
                false,
            );
            assert!(running(&run));
            let off = Settings {
                contribute: false,
                ..Settings::default()
            };
            let plan = tick(&mut run, &host, on(Power::Charging), off, false);
            assert_eq!(plan.work, HostWork::Nothing);
            assert!(
                run.is_none(),
                "switched off, the loop kept a listener open for nobody"
            );
        });
    }
}
