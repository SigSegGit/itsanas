//! The account's devices, from a phone: list them, withdraw one, and get past
//! the five-device limit without a command line.
//!
//! # Why the phone needed this
//!
//! At [`MAX_DEVICES_PER_ACCOUNT`] live devices a new machine is refused, and
//! until 2026-10-01 the refusal told the person to run
//! `itsanas device forget <id>`. An account whose machines are all phones has
//! no such command, so a lost phone held its slot for ever and the account
//! could never add another. Everything here is the command line's own core
//! (`itsanas_node::coordinator`): the same listing, the same withdrawal, the
//! same refusal to withdraw the device asking. What is the phone's own is the
//! wording, and one stricter rule: the app withdraws only a full id it was
//! shown, never one typed or abbreviated.
//!
//! The bodies take a `&Node` and are tested against a real coordinator
//! (`tests` below); the JNI shims in `lib.rs` only lock the node and call them.

use std::sync::Mutex;

use itsanas_crypto::{DeviceId, ID_LEN};
use itsanas_node::Node;
use itsanas_node::coordinator::{self, MAX_DEVICES_PER_ACCOUNT as LIMIT, Secret};
use serde_json::{Value, json};

use crate::Failure;

/// The full ids each phone was last refused a slot over, keyed by that
/// phone's own device id.
///
/// A phone that is not enrolled cannot list the account's devices (the
/// coordinator answers `Devices` only to a live device), and the devices it
/// can still see are those heard from this week -- which leaves out exactly
/// the lost phones it came to withdraw. The refusal names all of them, so
/// what it named is remembered here and counts as shown. Keyed rather than a
/// single list so two nodes in one process (the tests) cannot see each
/// other's.
static CAP_NAMED: Mutex<Vec<(DeviceId, Vec<DeviceId>)>> = Mutex::new(Vec::new());

fn remembered(mine: DeviceId) -> Vec<DeviceId> {
    CAP_NAMED
        .lock()
        .ok()
        .and_then(|named| {
            named
                .iter()
                .find(|(phone, _)| *phone == mine)
                .map(|(_, ids)| ids.clone())
        })
        .unwrap_or_default()
}

fn remember(mine: DeviceId, ids: Vec<DeviceId>) {
    if let Ok(mut named) = CAP_NAMED.lock() {
        named.retain(|(phone, _)| *phone != mine);
        if !ids.is_empty() {
            named.push((mine, ids));
        }
    }
}

fn no_coordinator(node: &Node) -> Result<(), Failure> {
    if node.config.coordinator.is_none() {
        return Err(Failure::Usage(
            "no coordinator is configured. Set one first, with the address \
             whoever invited you gave you."
                .to_owned(),
        ));
    }
    Ok(())
}

/// The account's devices, this phone marked, as JSON for the devices screen.
///
/// `complete` is false when the coordinator would not list every enrolled
/// device (this phone is not enrolled, or the coordinator is older than
/// `Devices`): then the list holds the devices seen this week plus any the
/// last refusal named, and the screen says so rather than pretending.
pub(crate) fn list(node: &Node) -> Result<Value, Failure> {
    no_coordinator(node)?;
    let mine = node.store.device_id();
    let entry = |device: DeviceId, address: Option<String>| {
        json!({
            "id": device.to_string(),
            "short": device.short(),
            "address": address,
            "thisPhone": device == mine,
        })
    };

    let (mut devices, complete) = match coordinator::enrolled(node) {
        Ok(Some(enrolled)) => (
            enrolled
                .iter()
                .map(|row| {
                    let mut value = entry(row.device, row.address.clone());
                    value["pledgedBytes"] = json!(row.pledged_bytes);
                    value["silentForSeconds"] = json!(row.silent_for);
                    value
                })
                .collect::<Vec<_>>(),
            true,
        ),
        // Refused (not enrolled), too old, or not answering: what can be seen.
        first => match coordinator::devices(node, node.store.owner()) {
            Ok(seen) => (
                seen.into_iter()
                    .map(|(device, address)| {
                        entry(device, (!address.is_empty()).then_some(address))
                    })
                    .collect(),
                false,
            ),
            Err(second) => return Err(first.err().unwrap_or(second).into()),
        },
    };

    // What a refusal named stands in for a listing this phone cannot get, and
    // only for that: once the coordinator lists every device, a remembered id
    // it does not list is gone (withdrawn from elsewhere), and offering it
    // would be a Withdraw button for nothing (found by `itsanas-redteam`).
    if complete {
        remember(mine, Vec::new());
    } else {
        for device in remembered(mine) {
            let id = device.to_string();
            if !devices.iter().any(|value| value["id"] == id.as_str()) {
                devices.push(entry(device, None));
            }
        }
    }

    Ok(json!({
        "devices": devices,
        "complete": complete,
        "limit": LIMIT,
        "thisPhone": mine.to_string(),
    }))
}

/// Withdraw the device `id` names from this account.
///
/// Refused, with nothing sent: an id that is not a full one (64 hex
/// characters); this phone itself; and an id that is not one of this
/// account's devices as [`list`] shows them. The last is the phone's own rule:
/// the coordinator files a withdrawal for any device id the account signs, so
/// a stray id would be revoked silently and for ever; a device of another
/// account is refused here and again by the coordinator, which is the bound
/// that holds. This phone itself is refused by
/// `coordinator::withdraw_device`, the command line's rule. The screen offers
/// only listed ids, so a refusal here is a bug in the shell, said out loud.
pub(crate) fn withdraw(node: &Node, id: &str, now: u64) -> Result<Value, Failure> {
    let id = id.trim();
    if id.len() != 2 * ID_LEN {
        return Err(Failure::Usage(format!(
            "a device is withdrawn by its full id, {} characters; {id:?} is {}",
            2 * ID_LEN,
            id.len()
        )));
    }
    let wanted: DeviceId = id
        .parse()
        .map_err(|_| Failure::Usage(format!("{id:?} is not a device id")))?;

    let shown = list(node)?;
    let known = shown["devices"]
        .as_array()
        .is_some_and(|devices| devices.iter().any(|value| value["id"] == id));
    if !known {
        return Err(Failure::Usage(format!(
            "{} is not one of this account's devices as the coordinator lists them; \
             nothing was withdrawn.",
            wanted.short()
        )));
    }

    // Refuses this phone itself, the rule the command line applies too.
    coordinator::withdraw_device(node, wanted, now)?;

    let mine = node.store.device_id();
    let mut left = remembered(mine);
    left.retain(|device| *device != wanted);
    remember(mine, left);

    Ok(json!({
        "withdrew": wanted.to_string(),
        "said": "Withdrawn. Nothing will dial it again and its slot is free. \
                 The withdrawal is final for that device: to use it again, \
                 remove the app's data on it and log in afresh. Someone who \
                 has that device and its passphrase can still read the account.",
    }))
}

/// Enrol this phone, or say it is at the limit and which devices to choose
/// from.
///
/// The limit is not an error to the app: it is a screen. An `Ok` with
/// `atCap: true` carries the named devices and a sentence that names the
/// button below it, not a command the phone does not have. Every other
/// failure is the same `Err` the command line prints.
pub(crate) fn register(node: &Node, invite: Option<&Secret>, now: u64) -> Result<Value, Failure> {
    no_coordinator(node)?;
    let mine = node.store.device_id();

    match coordinator::register_with(node, invite, now) {
        Ok(()) => {}
        Err(refused) => {
            let Some(named) = coordinator::cap_named(&refused) else {
                return Err(refused.into());
            };
            // The coordinator names at most a few more than the limit and ends
            // with "and N more" for an account far above it (enrolled before
            // the limit existed). The phone cannot count what it was not told.
            let partial = refused.to_string().contains(" more");
            remember(mine, named.clone());
            return Ok(json!({
                "enrolled": false,
                "atCap": true,
                "limit": LIMIT,
                "devices": named
                    .iter()
                    .map(|device| json!({
                        "id": device.to_string(),
                        "short": device.short(),
                        "address": Value::Null,
                        "thisPhone": false,
                    }))
                    .collect::<Vec<_>>(),
                "partial": partial,
                "said": format!(
                    "This account already has as many live devices as it may, {LIMIT}. \
                     This phone was not added and nothing was sent; the others keep \
                     working. Withdraw one you no longer use below, then join again.{}",
                    if partial {
                        " It has more than are named here: the rest are named the \
                         next time joining is refused."
                    } else {
                        ""
                    }
                ),
            }));
        }
    }
    remember(mine, Vec::new());

    let listen = node.config.listen.clone();
    // An address that could not be published does not undo the enrolment,
    // exactly as on the command line.
    let announced = coordinator::announce(node, &listen, now).ok();

    Ok(json!({
        "enrolled": true,
        "atCap": false,
        "username": node.config.username,
        "coordinator": node.config.coordinator,
        "announced": announced,
    }))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};

    use itsanas_coord::Directory;
    use itsanas_coord::server::CoordServer;
    use itsanas_crypto::{DeviceKeys, SecretBytes};

    use super::*;

    const PASSPHRASE: &str = "a passphrase for a test and nowhere else";
    const NOW: u64 = 1_700_000_000;

    /// Stops the server even when the body panics, so a failed assertion is
    /// reported as one and not as a hang.
    struct StopOnDrop<'a>(&'a AtomicBool, SocketAddr);

    impl Drop for StopOnDrop<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
            let _ = std::net::TcpStream::connect(self.1);
        }
    }

    fn with_directory<T>(body: impl FnOnce(SocketAddr, &Directory) -> T) -> T {
        let dir = tempfile::tempdir().expect("temp dir");
        let directory = Directory::open(dir.path().join("directory.redb")).expect("directory");
        let server = CoordServer::bind("127.0.0.1:0").expect("bind");
        let address = server.local_addr().expect("address");
        let shutdown = AtomicBool::new(false);
        let coordinator_device = DeviceKeys::from_seed(&SecretBytes::new([0xC0; 32]));

        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _ = server.serve_until(&directory, &coordinator_device, &shutdown, |_| {});
            });
            let _stop = StopOnDrop(&shutdown, address);
            body(address, &directory)
        })
    }

    /// A phone of the account `phrase` names (a new account `username` when
    /// `None`), pointed at `coordinator`, not yet enrolled.
    fn phone(
        home: &Path,
        coordinator: SocketAddr,
        username: &str,
        phrase: Option<&str>,
    ) -> (Node, String) {
        let (mut node, phrase) = match phrase {
            None => {
                let (node, phrase) = Node::create(home, PASSPHRASE, username).expect("create");
                (node, phrase.0.to_string())
            }
            Some(phrase) => (
                Node::restore(home, PASSPHRASE, username, phrase).expect("restore"),
                phrase.to_owned(),
            ),
        };
        node.config.coordinator = Some(coordinator.to_string());
        node.save_config().expect("save config");
        (node, phrase)
    }

    fn live(directory: &Directory, device: DeviceId) -> bool {
        directory
            .claim_for(device)
            .expect("read")
            .is_some_and(|signed| !signed.claim.revoked)
    }

    /// What a refusal said; an acceptance fails the test.
    fn usage(outcome: Result<Value, Failure>) -> String {
        match outcome {
            Err(refused) => refused.message(),
            Ok(answer) => panic!("accepted: {answer}"),
        }
    }

    /// THE WITHDRAW BUTTON'S BOUNDS. Of an enrolled phone's three wrong
    /// requests -- an abbreviated id, an id that is no device of the account,
    /// this phone itself -- none withdraws anything, and the right one
    /// withdraws exactly that device. Another account's device is refused
    /// too (by the coordinator as well; the phone refuses first).
    /// Sabotage: drop the length check and resolve a short id against the
    /// listing (as the CLI's `resolve_device` does); drop the self check in
    /// `coordinator::withdraw_device`; drop the `known` check (the stray id
    /// is then filed as revoked).
    #[test]
    fn red_team_the_phone_withdraws_only_a_listed_full_id_and_never_itself() {
        with_directory(|address, directory| {
            let dir = tempfile::tempdir().expect("temp dir");
            let (me, phrase) = phone(&dir.path().join("me"), address, "nicolas", None);
            let (other, _) = phone(&dir.path().join("other"), address, "nicolas", Some(&phrase));
            let (stranger, _) = phone(&dir.path().join("stranger"), address, "voisin", None);
            for node in [&me, &other, &stranger] {
                coordinator::register_with(node, None, NOW).expect("enrol");
            }
            let mine = me.store.device_id();
            let theirs = other.store.device_id();
            let foreign = stranger.store.device_id();
            let nobody = DeviceKeys::from_seed(&SecretBytes::new([0x77; 32])).device_id();

            let said = usage(withdraw(&me, &theirs.short(), NOW + 1));
            assert!(said.contains("full id"), "{said}");
            assert!(
                live(directory, theirs),
                "an abbreviated id withdrew a device"
            );

            let said = usage(withdraw(&me, &mine.to_string(), NOW + 1));
            assert!(said.contains("that is this device"), "{said}");
            assert!(live(directory, mine), "the phone withdrew itself");

            usage(withdraw(&me, &nobody.to_string(), NOW + 1));
            assert!(
                directory.claim_for(nobody).expect("read").is_none(),
                "an id that is no device of the account was filed as withdrawn"
            );

            usage(withdraw(&me, &foreign.to_string(), NOW + 1));
            assert!(
                live(directory, foreign),
                "another account's device was withdrawn"
            );

            let done = withdraw(&me, &theirs.to_string(), NOW + 2).expect("withdraw");
            assert_eq!(done["withdrew"], theirs.to_string());
            assert!(!live(directory, theirs), "the listed device is still live");
            assert!(
                live(directory, mine),
                "withdrawing another took this phone too"
            );
        });
    }

    /// THE ALL-PHONE ACCOUNT AT THE LIMIT. Five phones enrolled and never
    /// heard from again (lost, reset); the sixth, the only one left, is not
    /// enrolled and sees none of them. Joining answers with the five ids and
    /// a sentence naming the screen, not a command; withdrawing one of those
    /// ids works from the unenrolled phone, and joining again takes the slot.
    /// Sabotage: let `register` return the refusal as an error (no ids, a CLI
    /// command in the words); forget what the refusal named (the withdrawal
    /// is then refused as unknown and the account is stuck).
    #[test]
    fn red_team_an_all_phone_account_at_the_limit_can_free_a_slot_from_the_phone() {
        with_directory(|address, directory| {
            let dir = tempfile::tempdir().expect("temp dir");
            let mut phrase: Option<String> = None;
            let mut lost = Vec::new();
            for index in 0..LIMIT {
                let (node, said) = phone(
                    &dir.path().join(format!("p{index}")),
                    address,
                    "nicolas",
                    phrase.as_deref(),
                );
                coordinator::register_with(&node, None, NOW).expect("under the limit");
                lost.push(node.store.device_id());
                phrase = Some(said);
            }

            let (sixth, _) = phone(
                &dir.path().join("last"),
                address,
                "nicolas",
                phrase.as_deref(),
            );
            let answer =
                register(&sixth, None, NOW + 1).expect("the limit is a screen, not an error");
            assert_eq!(answer["atCap"], true, "{answer}");
            let named: Vec<&str> = answer["devices"]
                .as_array()
                .expect("devices")
                .iter()
                .filter_map(|device| device["id"].as_str())
                .collect();
            for device in &lost {
                assert!(
                    named.contains(&device.to_string().as_str()),
                    "the refusal does not name {device:?}: {answer}"
                );
            }
            let said = answer["said"].as_str().expect("said");
            assert!(
                !said.contains("itsanas "),
                "the phone was told to run a command it does not have: {said}"
            );
            assert!(
                !live(directory, sixth.store.device_id()),
                "a sixth device was enrolled"
            );

            withdraw(&sixth, &lost[0].to_string(), NOW + 2)
                .expect("an unenrolled phone could not withdraw a device the refusal named");
            assert!(!live(directory, lost[0]), "the named device is still live");

            let joined = register(&sixth, None, NOW + 3).expect("register");
            assert_eq!(
                joined["enrolled"], true,
                "the freed slot did not let the phone in: {joined}"
            );
            assert!(
                live(directory, sixth.store.device_id()),
                "the phone is not enrolled"
            );
        });
    }

    /// What a refusal named is offered only while the phone cannot list the
    /// account. Refused at the limit, the phone remembers five ids; one is
    /// withdrawn from another device and the phone is enrolled by another
    /// path. Its list is then the coordinator's, with no Withdraw button for
    /// the device already gone (found by `itsanas-redteam`). Sabotage: merge
    /// the remembered ids into a complete listing too.
    #[test]
    fn red_team_a_device_withdrawn_elsewhere_is_not_offered_from_an_old_refusal() {
        with_directory(|address, _| {
            let dir = tempfile::tempdir().expect("temp dir");
            let mut phrase: Option<String> = None;
            let mut five = Vec::new();
            for index in 0..LIMIT {
                let (node, said) = phone(
                    &dir.path().join(format!("p{index}")),
                    address,
                    "nicolas",
                    phrase.as_deref(),
                );
                coordinator::register_with(&node, None, NOW).expect("under the limit");
                phrase = Some(said);
                five.push(node);
            }
            let (sixth, _) = phone(
                &dir.path().join("sixth"),
                address,
                "nicolas",
                phrase.as_deref(),
            );
            let answer = register(&sixth, None, NOW + 1).expect("register");
            assert_eq!(answer["atCap"], true, "{answer}");

            let gone = five[0].store.device_id();
            coordinator::withdraw_device(&five[1], gone, NOW + 2).expect("withdraw elsewhere");
            coordinator::register_with(&sixth, None, NOW + 3).expect("enrol by another path");

            let shown = list(&sixth).expect("list");
            assert_eq!(shown["complete"], true, "{shown}");
            let ids: Vec<&str> = shown["devices"]
                .as_array()
                .expect("devices")
                .iter()
                .filter_map(|device| device["id"].as_str())
                .collect();
            assert!(
                !ids.contains(&gone.to_string().as_str()),
                "a device withdrawn elsewhere is still offered: {shown}"
            );
            assert_eq!(ids.len(), LIMIT, "{shown}");
        });
    }

    /// An account far above the limit (enrolled before it existed): the
    /// coordinator names only some devices and says how many more. The phone
    /// says the list is partial and guesses no count (found by
    /// `itsanas-redteam`: it said "already has 8" of 10). Sabotage: report
    /// `partial` as false.
    #[test]
    fn red_team_a_refusal_naming_only_some_devices_says_so() {
        with_directory(|address, directory| {
            directory.bound_devices(false);
            let dir = tempfile::tempdir().expect("temp dir");
            let mut phrase: Option<String> = None;
            for index in 0..10 {
                let (node, said) = phone(
                    &dir.path().join(format!("p{index}")),
                    address,
                    "nicolas",
                    phrase.as_deref(),
                );
                coordinator::register_with(&node, None, NOW).expect("no bound yet");
                phrase = Some(said);
            }
            directory.bound_devices(true);

            let (eleventh, _) = phone(
                &dir.path().join("eleventh"),
                address,
                "nicolas",
                phrase.as_deref(),
            );
            let answer = register(&eleventh, None, NOW + 1).expect("register");
            assert_eq!(answer["atCap"], true, "{answer}");
            assert_eq!(answer["partial"], true, "{answer}");
            let said = answer["said"].as_str().expect("said");
            assert!(
                said.contains("more than are named"),
                "a partial list is presented as the whole: {said}"
            );
        });
    }

    /// The devices screen: every device of the account, this phone marked
    /// once, the list said to be complete.
    #[test]
    fn the_list_marks_this_phone_among_the_account_s_devices() {
        with_directory(|address, _| {
            let dir = tempfile::tempdir().expect("temp dir");
            let (me, phrase) = phone(&dir.path().join("me"), address, "nicolas", None);
            let (other, _) = phone(&dir.path().join("other"), address, "nicolas", Some(&phrase));
            for node in [&me, &other] {
                coordinator::register_with(node, None, NOW).expect("enrol");
            }

            let shown = list(&me).expect("list");
            let devices = shown["devices"].as_array().expect("devices");
            assert_eq!(devices.len(), 2, "{shown}");
            assert_eq!(shown["complete"], true, "{shown}");
            let marked: Vec<&Value> = devices.iter().filter(|d| d["thisPhone"] == true).collect();
            assert_eq!(marked.len(), 1, "{shown}");
            assert_eq!(marked[0]["id"], me.store.device_id().to_string());
        });
    }
}
