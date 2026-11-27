//! What every phone shell asks the core, without the language boundary.
//!
//! Android reaches these through JNI (`itsanas-android`) and iOS through C
//! (`itsanas-ios`). The boundary crates convert arguments and errors and
//! nothing else, so a file listed, fetched or added on either phone goes
//! through the same lines, and a fix made for one is made for both.

use std::path::Path;
use std::sync::{Arc, Mutex};

use itsanas_node::Node;

use crate::shell::{Answer, Failure, Network, describe};

mod account_files;
mod deletes;
mod round;

pub use account_files::{
    PLEDGES_FRESH_FOR, bring_back, fetch, get_file, list_all, list_at, put_file,
    refresh_old_pledges, rename, versions_of, write_file,
};
pub use deletes::{
    became_word, delete_for_good, purged_trash, trash_delete, trash_failed, trash_keep, trash_list,
    trash_put, trash_record, trash_restore_in, trash_rows, unix_now,
};
pub use round::{Versions, changes, sync_node};

#[must_use]
pub fn passphrase_rule() -> String {
    serde_json::json!({
        "min": itsanas_node::node::MIN_PASSPHRASE_CHARS,
        "max": itsanas_node::node::MAX_PASSPHRASE_CHARS,
    })
    .to_string()
}

/// The coordinator a phone's nodes belong to: the official one, unless a test
/// set `slot` through [`use_coordinator`].
pub fn network_in(slot: &Mutex<Option<Network>>) -> Result<Network, Failure> {
    let test = slot
        .lock()
        .map_err(|_| Failure::Usage("the network lock was poisoned by a panic".to_owned()))?;
    Ok(test.clone().unwrap_or_else(Network::official))
}

/// What a phone's debug build is told by its end-to-end tests: every node the
/// process creates, restores or opens is pinned to `address` instead of the
/// official coordinator, through `slot`. Android's `useCoordinatorForTests`
/// and iOS's `itsanas_use_coordinator_for_tests` pass `cfg!(debug_assertions)`.
///
/// Only a debug build takes it: the reason a phone cannot name its coordinator
/// (see [`Network::pin`]) is exactly what a call that could reach a release
/// build would undo, whoever made it.
pub fn use_coordinator(
    slot: &Mutex<Option<Network>>,
    address: &str,
    device: &str,
    debug_build: bool,
) -> Answer {
    if !debug_build {
        return Err(Failure::Usage(
            "this build joins the official coordinator and no other".to_owned(),
        ));
    }
    let device = (!device.is_empty()).then(|| device.to_owned());
    if let Some(device) = &device {
        itsanas_node::coordinator::parse_device(device)?;
    }
    let mut slot = slot
        .lock()
        .map_err(|_| Failure::Usage("the network lock was poisoned by a panic".to_owned()))?;
    *slot = Some(Network {
        address: address.to_owned(),
        device,
    });
    Ok(serde_json::json!({ "address": address }).to_string())
}

/// What a phone's `open` answers, for every phone shell, on the slot given.
///
/// A node already open for `home` in this process is answered as it is, not
/// opened again. The sync service runs in the foreground and keeps the
/// process -- and the open node -- alive after the screen is closed; opening
/// the store a second time is refused by the database ("already open"), and
/// on 2026-10-31 that left the owner's phone stuck on Unlock with nothing
/// usable. The passphrase is still checked against the keystore: the screen
/// asking for it is what stands between a person and the account.
pub fn open_into(
    slot: &Mutex<Option<Arc<Node>>>,
    home: &Path,
    passphrase: &str,
    network: &Network,
    now: u64,
) -> Answer {
    let mut guard = slot
        .lock()
        .map_err(|_| Failure::Usage("the node lock was poisoned by a panic".to_owned()))?;
    if let Some(node) = guard.as_ref().filter(|node| node.home == home) {
        itsanas_node::node::Identity::secrets(home, passphrase)?;
        return Ok(describe(node));
    }
    // Another account's node, if any, is closed before this one opens.
    *guard = None;
    let node = network.pin(Node::open(home, passphrase)?)?;
    crate::shell::after_open(&node, now);
    let answer = describe(&node);
    *guard = Some(Arc::new(node));
    Ok(answer)
}

/// What a phone's `status` answers, for every phone shell.
pub fn status_of(node: &Node) -> Answer {
    let stats = node.store.stats()?;
    let vault = node.vault.stats()?;
    let coverage = node.store.coverage(itsanas_discover::now_unix())?;
    let absent = itsanas_store::absent_count(&node.store, &node.vault)?;
    // The wait starts here too: a phone whose scheduled sync never runs would
    // otherwise have no date to age from, and read as fine for ever.
    crate::note_phone_contact(&node.home, false, itsanas_discover::now_unix());

    Ok(serde_json::json!({
        "username": node.config.username,
        "userId": node.store.owner().to_string(),
        "deviceId": node.store.device_id().to_string(),
        "files": stats.files,
        "here": stats.files,
        "notHere": absent,
        "bytesOnDisk": stats.bytes_on_disk,
        "vaultBytes": vault.bytes,
        // Not `vaultBytes`: this account's own chains relayed here are in that.
        "heldForOthers": node.held_for_others()?,
        "enrolled": itsanas_node::coordinator::registered_with(&node.home).is_some(),
        "keepBytes": node.config.keep_bytes,
        "pledgeBytes": node.config.pledge_bytes,
        "peers": node.config.peers,
        "copiesElsewhere": coverage.complete_elsewhere,
        "onlyHere": coverage.only_here,
        "liveChunks": coverage.live_chunks,
        "copiesWanted": crate::SAFE_COPIES,
        "safe": crate::account_is_safe(&coverage),
        "failing": crate::machines_failing(node, itsanas_discover::now_unix())?,
        "vaultRefuses": crate::vault_refuses(node).is_some(),
        "lowDisk": itsanas_node::pledges::low_on_disk(&node.home),
        "silentFor": crate::phone_silence(&node.home, itsanas_discover::now_unix()),
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HANDOVER §8 3f, end to end over sockets: the laptop meets only the Pi,
    /// the VM meets only the Pi, and the VM still applies the laptop's file:
    /// its armed roster waits on a chain from a device it never met until the
    /// Pi hands it the laptop's claim. Sabotage: the Pi answers no claims.
    #[test]
    fn red_team_a_machine_learns_a_sibling_it_never_met_through_a_third() {
        crate::testing::with_directory(|coordinator, _| {
            let dir = tempfile::tempdir().expect("tempdir");
            let (pi, phrase) =
                crate::testing::phone(&dir.path().join("pi"), coordinator, "camille", None);
            let (mut laptop, _) = crate::testing::phone(
                &dir.path().join("laptop"),
                coordinator,
                "camille",
                Some(&phrase),
            );
            let (mut vm, _) = crate::testing::phone(
                &dir.path().join("vm"),
                coordinator,
                "camille",
                Some(&phrase),
            );
            let source = dir.path().join("source");
            std::fs::write(&source, b"written on the laptop").expect("source");
            put_file(
                &laptop,
                "from-the-laptop.txt",
                source.to_str().expect("utf-8"),
            )
            .expect("put");

            crate::testing::serving(&pi, |at| {
                laptop.config.peers.push(at.to_string());
                sync_node(&laptop, false, unix_now()).expect("the laptop's round");
                vm.config.peers.push(at.to_string());
                // The first round learns the claim, the next applies what waited.
                for _ in 0..2 {
                    sync_node(&vm, false, unix_now()).expect("the VM's round");
                }
            });
            let applied = vm.store.list().expect("the VM's index");
            assert!(
                applied.iter().any(|path| path == "from-the-laptop.txt"),
                "the VM never applied a file from the laptop it reaches only through the Pi: an account's machines that never meet never converge ({applied:?})"
            );
        });
    }

    /// IOS.md hand test step 3, end to end over sockets: a file of the
    /// computer, deleted on the phone, put back, deleted again and then deleted
    /// for good from the phone's trash, leaves the computer at the next round.
    /// The iPhone run found it still listed there (CI, 2026-11-18); this
    /// shows the phone's side and the computer's drain are not the cause.
    #[test]
    fn a_file_deleted_for_good_on_a_phone_leaves_the_computer() {
        crate::testing::with_directory(|coordinator, _| {
            let dir = tempfile::tempdir().expect("tempdir");
            let (desk, phrase) =
                crate::testing::phone(&dir.path().join("desk"), coordinator, "camille", None);
            let (mut phone, _) = crate::testing::phone(
                &dir.path().join("phone"),
                coordinator,
                "camille",
                Some(&phrase),
            );
            let source = dir.path().join("source");
            std::fs::write(&source, b"to be deleted from the phone").expect("source");
            put_file(&desk, "to-trash.txt", source.to_str().expect("utf-8")).expect("put");

            crate::testing::serving(&desk, |at| {
                phone.config.peers.push(at.to_string());
                sync_node(&phone, false, unix_now()).expect("the phone's first round");
                trash_put(&phone, "to-trash.txt", 30).expect("delete");
                trash_restore_in(&phone, "to-trash.txt").expect("restore");
                trash_put(&phone, "to-trash.txt", 30).expect("delete again");
                trash_delete(&phone, "to-trash.txt").expect("delete for good");
                sync_node(&phone, false, unix_now()).expect("the round that tells the computer");
            });
            // What the computer's daemon does at each beat with what was pushed to it.
            itsanas_net::session::drain_vault(&desk.store, &desk.vault).expect("the drain");
            let listed =
                itsanas_store::catalogue(&desk.store, &desk.vault).expect("the computer's listing");
            assert!(
                !listed.files.iter().any(|file| file.path == "to-trash.txt"),
                "a file deleted for good on the phone is still on the computer: the person was told it left every machine ({:?})",
                listed
                    .files
                    .iter()
                    .map(|file| &file.path)
                    .collect::<Vec<_>>()
            );
        });
    }

    /// #409's iPhone run: a computer pledging nothing, as a new one does,
    /// refused every segment its own phone pushed, so a file deleted for good
    /// on the phone stayed on it. A pledge is room lent to other accounts; the
    /// account's own log is taken whatever it says. And a stranger pushing
    /// the account's segment is still refused here, by the claim book
    /// (`service.rs` tests the roster on a service with none). Sabotage: the
    /// pledge checked for siblings too.
    #[test]
    fn red_team_a_computer_that_pledges_nothing_still_takes_its_phones_delete() {
        crate::testing::with_directory(|coordinator, _| {
            let dir = tempfile::tempdir().expect("tempdir");
            let (desk, phrase) =
                crate::testing::phone(&dir.path().join("desk"), coordinator, "camille", None);
            let (mut phone, _) = crate::testing::phone(
                &dir.path().join("phone"),
                coordinator,
                "camille",
                Some(&phrase),
            );
            let (stranger, _) =
                crate::testing::phone(&dir.path().join("stranger"), coordinator, "dominique", None);
            let source = dir.path().join("source");
            std::fs::write(&source, b"to be deleted from the phone").expect("source");
            put_file(&desk, "to-trash.txt", source.to_str().expect("utf-8")).expect("put");

            let server = itsanas_net::PeerServer::bind("127.0.0.1:0").expect("bind");
            let stranger_offer =
                crate::testing::serving_pledged(&server, &desk, itsanas_net::Pledge::NONE, |at| {
                    phone.config.peers.push(at.to_string());
                    sync_node(&phone, false, unix_now()).expect("the phone's first round");
                    trash_put(&phone, "to-trash.txt", 30).expect("delete");
                    trash_delete(&phone, "to-trash.txt").expect("delete for good");
                    sync_node(&phone, false, unix_now())
                        .expect("the round that tells the computer");
                    // The phone's own tip, offered by a device of another
                    // account in the phone's account's name.
                    let tip = phone
                        .store
                        .segments()
                        .expect("the phone's chain")
                        .pop()
                        .expect("a segment");
                    let mut client = itsanas_net::PeerClient::connect(
                        at,
                        &stranger.device,
                        stranger.store.owner(),
                        None,
                    )
                    .expect("the stranger connects");
                    client.store_segment(&tip).expect("an answer")
                });
            assert!(
                matches!(stranger_offer, itsanas_net::Offer::Refused(_)),
                "a host at pledge 0 took a segment of its account from a device outside its roster: anybody can fill its disk in its owner's name ({stranger_offer:?})"
            );
            itsanas_net::session::drain_vault(&desk.store, &desk.vault).expect("the drain");
            let listed =
                itsanas_store::catalogue(&desk.store, &desk.vault).expect("the computer's listing");
            assert!(
                !listed.files.iter().any(|file| file.path == "to-trash.txt"),
                "a computer that pledges nothing kept a file its own phone deleted for good: the phone told the person it left every machine"
            );
        });
    }

    /// HANDOVER §9, "A withdrawn device keeps its old claim": a withdrawal
    /// reached the coordinator alone, so a machine of the account that reads
    /// none went on admitting the withdrawn one. The laptop withdraws the old
    /// phone, the old phone still presents its live claim to the laptop, and
    /// the VM, with no coordinator, learns the withdrawal from the laptop.
    /// Sabotage: the withdrawal is not kept on the laptop; or the old phone's
    /// live claim replaces it there.
    #[test]
    fn red_team_a_withdrawal_reaches_a_machine_that_reads_no_coordinator() {
        crate::testing::with_directory(|coordinator, _| {
            let dir = tempfile::tempdir().expect("tempdir");
            let (laptop, phrase) =
                crate::testing::phone(&dir.path().join("laptop"), coordinator, "camille", None);
            let (mut vm, _) = crate::testing::phone(
                &dir.path().join("vm"),
                coordinator,
                "camille",
                Some(&phrase),
            );
            let (mut old, _) = crate::testing::phone(
                &dir.path().join("old"),
                coordinator,
                "camille",
                Some(&phrase),
            );
            let gone = old.store.device_id();
            vm.store
                .admit_member(&gone, false)
                .expect("the old phone was a member");
            // Whatever the coordinator answers: the old phone never enrolled.
            let _ = itsanas_node::coordinator::withdraw_device(&laptop, gone, unix_now());

            vm.config.coordinator = None;
            old.config.coordinator = None;
            crate::testing::serving(&laptop, |at| {
                old.config.peers.push(at.to_string());
                let _ = sync_node(&old, true, unix_now());
                vm.config.peers.push(at.to_string());
                sync_node(&vm, true, unix_now()).expect("the VM's round");
            });
            assert!(
                vm.store.is_revoked(&gone).expect("roster"),
                "a machine that reads no coordinator still admits a device the account withdrew: its writes are applied"
            );
            assert!(
                !vm.store.may_apply_chain_of(&gone).expect("roster"),
                "the VM still applies the withdrawn phone's chain"
            );
        });
    }

    /// A host of another account learnt a withdrawal from the coordinator
    /// alone, and took the withdrawn device's old claim until then. A machine
    /// of the account passes its withdrawals on to the hosts it meets now, and
    /// the host refuses the old claim. Sabotage: pass nothing on; or the host
    /// takes no withdrawal but the caller's own.
    #[test]
    fn red_team_a_host_of_another_account_learns_a_withdrawal_from_a_sibling() {
        crate::testing::with_directory(|coordinator, _| {
            let dir = tempfile::tempdir().expect("tempdir");
            let (laptop, phrase) =
                crate::testing::phone(&dir.path().join("laptop"), coordinator, "camille", None);
            let (old, _) = crate::testing::phone(
                &dir.path().join("old"),
                coordinator,
                "camille",
                Some(&phrase),
            );
            let (host, _) =
                crate::testing::phone(&dir.path().join("host"), coordinator, "bob", None);
            let gone = old.store.device_id();
            let _ = itsanas_node::coordinator::withdraw_device(&laptop, gone, unix_now());
            let dial = |node: &itsanas_node::Node, at: std::net::SocketAddr| {
                itsanas_net::PeerClient::connect(
                    at,
                    &node.device,
                    node.store.owner(),
                    Some(host.store.device_id()),
                )
                .expect("the host answers")
            };
            crate::testing::serving(&host, |at| {
                laptop
                    .introduce(&mut dial(&laptop, at), unix_now())
                    .expect("the laptop introduces itself");
                let presented = dial(&old, at).present_claim(old.claim_bytes(unix_now()));
                assert!(
                    presented.is_err(),
                    "a host of another account took the withdrawn phone's old claim after a sibling had passed the withdrawal on: it stores for it again"
                );
            });
        });
    }

    /// HANDOVER §9, "Tail truncation": a host can serve a consistent prefix
    /// of a sibling's chain, every segment genuine and the last ones withheld,
    /// and a machine that met only that host listed the account as complete.
    /// The laptop writes twice, the Pi met it after the first only, and the
    /// VM syncs through the Pi alone; the laptop's signed presence says two
    /// segments, so the VM's listing says it is short and names the laptop.
    /// Sabotage: the listing ignores what devices signed.
    #[test]
    fn red_team_a_host_serving_the_start_of_a_chain_is_caught() {
        crate::testing::with_directory(|coordinator, _| {
            let dir = tempfile::tempdir().expect("tempdir");
            let (mut laptop, phrase) =
                crate::testing::phone(&dir.path().join("laptop"), coordinator, "camille", None);
            let (mut pi, _) = crate::testing::phone(
                &dir.path().join("pi"),
                coordinator,
                "camille",
                Some(&phrase),
            );
            let (mut vm, _) = crate::testing::phone(
                &dir.path().join("vm"),
                coordinator,
                "camille",
                Some(&phrase),
            );
            itsanas_node::coordinator::register_with(&laptop, None, unix_now())
                .expect("the laptop enrols");
            let source = dir.path().join("source");
            std::fs::write(&source, b"the first file").expect("source");
            put_file(&laptop, "first.txt", source.to_str().expect("utf-8")).expect("put");
            crate::testing::serving(&laptop, |at| {
                pi.config.peers.push(at.to_string());
                sync_node(&pi, false, unix_now()).expect("the Pi's round");
            });
            std::fs::write(&source, b"the second file").expect("source");
            put_file(&laptop, "second.txt", source.to_str().expect("utf-8")).expect("put");
            laptop.config.announce = Some("127.0.0.1:9".to_owned());
            itsanas_node::coordinator::announce(&laptop, "127.0.0.1:9", unix_now())
                .expect("the laptop publishes where it is, and how long its log is");

            pi.config.peers.clear();
            crate::testing::serving(&pi, |at| {
                vm.config.peers.push(at.to_string());
                sync_node(&vm, true, unix_now()).expect("the VM's round");
            });
            let listing = itsanas_store::catalogue(&vm.store, &vm.vault).expect("listing");
            assert!(
                listing.files.iter().any(|file| file.path == "first.txt"),
                "the fixture's Pi did not hand the VM the laptop's first file"
            );
            assert!(
                !listing.complete && listing.behind.contains(&laptop.store.device_id()),
                "a host served only the start of the laptop's log and the VM's listing reads as the whole account: {:?}",
                listing.behind
            );
        });
    }
}
