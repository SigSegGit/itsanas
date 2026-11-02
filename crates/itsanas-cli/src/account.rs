//! Getting an account onto a machine and off it again: what is asked before
//! anything is written, and what a logout leaves behind.
//!
//! Every shell's Settings page reaches these through `/api/remove`; the bodies
//! are tested against a real coordinator (`tests` below). They answer with the
//! JSON a shell can parse, and with a [`Failure`] whose sentence is the one a
//! person sees.
//!
//! # Why a logout renames before it deletes
//!
//! Deleting a home file by file can stop half way, and a home whose keystore
//! is gone and whose `vault/.itsanas-vault` survived refuses every later login
//! ("belongs to another device"), for ever, with nothing in the app to clear
//! it. A rename takes the whole home out of [`Node::exists`]'s sight at once,
//! and whatever a failed delete leaves is swept by the next login.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use itsanas_crypto::{DeviceId, MasterSecret, PhraseProblem, UserId, UserKeys};
use itsanas_node::node::{Identity, VAULT_MARKER, check_new_passphrase, phrase_words};
use itsanas_node::pledges::{self, DiskSpace};
use itsanas_node::{Config, Node, coordinator};
use serde_json::{Value, json};

use crate::failure::{Answer, Failure};
use crate::rounds::{run_round, targets};

/// The coordinator a node on this phone belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Network {
    pub address: String,
    /// The device to pin it to; `None` only for a test's coordinator.
    pub device: Option<String>,
}

impl Network {
    #[must_use]
    pub fn official() -> Self {
        Self {
            address: itsanas_node::coordinator::DEFAULT_COORDINATOR.to_owned(),
            device: Some(itsanas_node::coordinator::DEFAULT_COORDINATOR_DEVICE.to_owned()),
        }
    }

    /// Point `node` at this coordinator, pinned to its device, and save.
    ///
    /// The app offers no way to name another coordinator: a field for one let
    /// anybody who could talk a user into typing an address send that user's
    /// account to a network of their choosing. Done on restore and open, so a
    /// node made by an older build that could be pointed elsewhere comes back
    /// to the official network the next time it is unlocked.
    pub fn pin(&self, mut node: Node) -> Result<Node, Failure> {
        node.config.coordinator = Some(self.address.clone());
        node.config.coordinator_device.clone_from(&self.device);
        node.save_config()?;
        Ok(node)
    }
}

/// A restored node, on `network`'s coordinator.
pub fn restore_account(
    home: &std::path::Path,
    passphrase: &str,
    username: &str,
    phrase: &str,
    network: &Network,
) -> Result<Node, Failure> {
    network.pin(Node::restore(home, passphrase, username, phrase)?)
}

pub fn describe(node: &Node) -> String {
    serde_json::json!({
        "userId": node.store.owner().to_string(),
        "deviceId": node.store.device_id().to_string(),
        "username": node.config.username,
    })
    .to_string()
}

/// Beside the home, an old home on its way out.
const REMOVING_PREFIX: &str = "node.removing-";

/// Beside the home, the device ids a logout could not withdraw, one a line, for
/// the next login to withdraw. Not a secret: a device id is on every claim.
const WITHDRAW_AFTER_LOGIN: &str = "withdraw-after-login";

pub fn lock(
    slot: &Mutex<Option<Node>>,
) -> Result<std::sync::MutexGuard<'_, Option<Node>>, Failure> {
    slot.lock()
        .map_err(|_| Failure::Usage("the node lock was poisoned by a panic".to_owned()))
}

// ------------------------------------------------- asked before anything else

/// Whether `username` can be taken at the coordinator at `address`. A name no
/// coordinator would register is not sent, and a coordinator that cannot be
/// asked is an answer, not a failure: the screen says so and offers a retry.
#[must_use]
pub fn check_name(address: &str, username: &str) -> String {
    if let Some(why) = coordinator::username_refusal(username) {
        return json!({ "valid": false, "why": why, "taken": "unknown" }).to_string();
    }
    let taken = match coordinator::name_taken(address, username) {
        Ok(true) => "yes",
        Ok(false) => "no",
        Err(_) => "unknown",
    };
    json!({ "valid": true, "why": Value::Null, "taken": taken }).to_string()
}

/// The sentence for a phrase that does not parse. Names a position and never a
/// word: this goes on a screen, and into whatever a person sends for help.
fn phrase_refusal(problem: PhraseProblem) -> String {
    match problem {
        PhraseProblem::UnknownWord(position) => {
            format!("word {position} is not one of the words a recovery phrase is made of")
        }
        PhraseProblem::WordCount(found) => {
            format!("a recovery phrase is 24 words, and this has {found}")
        }
        PhraseProblem::Checksum => {
            "these 24 words do not fit together: one is misread, or two are swapped".to_owned()
        }
    }
}

/// The account `phrase` opens, from the words alone.
fn user_of(words: &str) -> Result<UserId, Failure> {
    let master =
        MasterSecret::from_recovery_phrase(words).map_err(itsanas_node::NodeError::from)?;
    Ok(UserKeys::derive(&master).user_id())
}

/// The words alone, as they are typed, for a native screen's live check:
/// `{"ok":bool,"wordIndex":n|null,"count":n}`. `wordIndex` counts from 1 and
/// names the first word that is in no recovery phrase; a phrase still being
/// typed is not `ok` but names no word either.
#[must_use]
pub fn check_words(phrase: &str) -> String {
    let words = phrase_words(phrase);
    let count = words.split_whitespace().count();
    let problem = MasterSecret::phrase_problem(&words);
    let position = match problem {
        Some(PhraseProblem::UnknownWord(position)) => Some(position),
        _ => None,
    };
    json!({ "ok": problem.is_none(), "wordIndex": position, "count": count }).to_string()
}

/// The words and the name of an existing account, before anything is written.
///
/// The name decides nothing about which account this is (the words alone do),
/// which is why a mistyped one is worth stopping: it would bind this key to that
/// name for ever at its first registration, or read as somebody else's.
#[must_use]
pub fn check_restore(address: &str, username: &str, phrase: &str) -> String {
    let words = phrase_words(phrase);
    let count = words.split_whitespace().count();
    if let Some(problem) = MasterSecret::phrase_problem(&words) {
        let position = match problem {
            PhraseProblem::UnknownWord(position) => Some(position),
            PhraseProblem::WordCount(_) | PhraseProblem::Checksum => None,
        };
        return json!({
            "words": "bad",
            "wordIndex": position,
            "count": count,
            "name": "unknown",
            "why": phrase_refusal(problem),
        })
        .to_string();
    }

    let (name, why) = match coordinator::username_refusal(username) {
        Some(why) => ("invalid", Some(why)),
        None => match (coordinator::name_owner(address, username), user_of(&words)) {
            (Ok(Some(owner)), Ok(mine)) if owner == mine => ("yours", None),
            (Ok(Some(_)), _) => ("other", None),
            (Ok(None), _) => ("none", None),
            (Err(_), _) => ("unknown", None),
        },
    };
    json!({
        "words": "ok",
        "wordIndex": Value::Null,
        "count": count,
        "name": name,
        "why": why,
    })
    .to_string()
}

/// A fresh identity's words, written nowhere. The account is made by logging in
/// with them, once they are shown and checked: a rotation or a kill before that
/// loses nothing, because nothing exists yet.
pub fn new_phrase() -> Answer {
    let phrase = MasterSecret::generate()
        .and_then(|master| master.to_recovery_phrase())
        .map_err(itsanas_node::NodeError::from)?;
    Ok(json!({ "phrase": phrase.as_str() }).to_string())
}

/// This node's words again, from its keystore and its passphrase.
pub fn words(home: &Path, passphrase: &str) -> Answer {
    let phrase = Identity::recovery_phrase(home, passphrase)?;
    Ok(json!({ "phrase": phrase.as_str() }).to_string())
}

/// Re-seal the keystore. It reads the keystore file alone, so it works while
/// the sync service holds the node.
pub fn change_passphrase(home: &Path, current: &str, new: &str) -> Answer {
    Node::change_passphrase(home, current, new)?;
    Ok(json!({ "changed": true }).to_string())
}

// ------------------------------------------------------------ logging in

/// What a login is given, as the person typed it.
#[derive(Clone, Copy, Debug)]
pub struct Login<'a> {
    pub home: &'a Path,
    pub username: &'a str,
    pub phrase: &'a str,
    pub passphrase: &'a str,
}

/// How much of the account to keep on a phone logging in: silently, unlike a
/// pledge, because it protects the phone's own disk and offers nobody anything.
/// With no budget a restore of a large account downloads until the disk is
/// full. Judged at a pledge of 0, which is what is in force until the person
/// chooses another on the Space step, and left unset when the disk would not
/// say: a limit made up from nothing would be worse than none.
fn set_keep_budget(node: &mut Node, disk: DiskSpace) {
    let (Some(free), Some(total)) = (disk.free, disk.total) else {
        return;
    };
    let usable = pledges::phone_usable(total, free, 0);
    let others = Node::others_pledged(&node.home);
    node.config.keep_bytes = Some(pledges::phone_keep(&node.config, usable, others, 0));
    // Best effort, as the next setter saves it: the node in memory has it.
    let _ = node.save_config();
}

/// What the phone does once its keystore is written, by either way of getting
/// one: withdraw whatever a logout could not, and start with a budget.
fn settle_in(node: &mut Node, disk: DiskSpace, now: u64) {
    set_keep_budget(node, disk);
    withdraw_pending(node, now);
}

/// Restore an account on this phone: the words, in any of the forms a person
/// saved them, and a new passphrase. Used for a new account too, once its words
/// were shown and checked, exactly as the command line's setup does.
pub fn login_into(
    slot: &Mutex<Option<Node>>,
    login: Login<'_>,
    disk: DiskSpace,
    network: &Network,
    now: u64,
) -> Answer {
    let mut guard = lock(slot)?;
    sweep_removed(login.home);
    let mut node = restore_account(
        login.home,
        login.passphrase,
        login.username,
        &phrase_words(login.phrase),
        network,
    )?;
    settle_in(&mut node, disk, now);
    let answer = describe(&node);
    *guard = Some(node);
    Ok(answer)
}

/// "I forgot my passphrase": the words replace the node on this phone.
///
/// Everything that can refuse is asked before anything is touched: the words,
/// the new passphrase, and whether the coordinator gives this account's name to
/// these words. A coordinator that cannot be asked, or has no such name, does
/// not stop it, because the words are the proof of the account and the phone
/// must be recoverable offline. The old home is only set aside until the new one
/// exists, so a restore that fails puts it back.
pub fn restore_over(
    slot: &Mutex<Option<Node>>,
    login: Login<'_>,
    disk: DiskSpace,
    network: &Network,
    now: u64,
) -> Answer {
    let words = phrase_words(login.phrase);
    if let Some(problem) = MasterSecret::phrase_problem(&words) {
        return Err(Failure::Usage(format!(
            "{}. Nothing on this phone was changed.",
            phrase_refusal(problem)
        )));
    }
    check_new_passphrase(login.passphrase)?;

    let config = Config::load(&Node::config_path(login.home))?;
    if config.username.is_empty() {
        return Err(Failure::Usage(
            "this phone's settings do not say which account it holds, so the words cannot be \
             checked against it. Nothing on this phone was changed."
                .to_owned(),
        ));
    }
    if let Ok(Some(owner)) = coordinator::name_owner(&network.address, &config.username)
        && owner != user_of(&words)?
    {
        return Err(Failure::Usage(format!(
            "these words belong to another account than {}. Nothing on this phone was changed.",
            config.username
        )));
    }
    let old_device =
        std::fs::read_to_string(Node::vault_path(login.home, &config).join(VAULT_MARKER))
            .ok()
            .and_then(|text| text.trim().parse::<DeviceId>().ok());

    let mut guard = lock(slot)?;
    // The sync service may hold it; the store cannot move while it is open.
    *guard = None;
    sweep_removed(login.home);
    let aside = set_aside(login.home, now)?;
    let mut node = match restore_account(
        login.home,
        login.passphrase,
        &config.username,
        &words,
        network,
    ) {
        Ok(node) => node,
        Err(failed) => {
            let _ = std::fs::remove_dir_all(login.home);
            let _ = std::fs::rename(&aside, login.home);
            return Err(failed);
        }
    };
    let _ = std::fs::remove_dir_all(&aside);
    if let Some(old_device) = old_device {
        remember_to_withdraw(login.home, old_device);
    }
    settle_in(&mut node, disk, now);
    let answer = describe(&node);
    *guard = Some(node);
    Ok(answer)
}

// ------------------------------------------------------------ logging out

/// One best-effort round, then what logging out would lose. Closes nothing.
///
/// `onlyHere` is the figure that matters and it comes after the round on
/// purpose: a phone that can reach a machine should not be told its files are
/// about to be lost when pushing them was all it took.
pub fn logout_check(node: &Node, metered: bool) -> Answer {
    let scope = if metered {
        itsanas_net::session::Scope::Metadata
    } else {
        itsanas_net::session::Scope::Everything
    };
    let _ = node.bound_writes();
    let round = run_round(node, &targets(node), scope);
    let coverage = node.store.coverage(itsanas_discover::now_unix())?;
    Ok(json!({
        "reached": round.reached,
        "onlyHere": coverage.only_here,
        "liveChunks": coverage.live_chunks,
        "copiesElsewhere": coverage.complete_elsewhere,
        "heldForOthers": node.held_for_others()?,
        "username": node.config.username,
    })
    .to_string())
}

/// What the machines and the coordinator were told.
#[derive(Clone, Copy, Debug)]
struct Farewell {
    told: usize,
    departed: bool,
    withdrawn: bool,
}

/// Tell each of these machines this device is going away, so they stop counting
/// its copies at once rather than after a week of silence.
fn tell_machines(node: &Node, targets: &[(String, Option<DeviceId>)]) -> usize {
    let mut told = 0;
    for (target, pinned) in targets {
        if let Ok(mut client) = itsanas_net::PeerClient::connect(
            itsanas_net::AddressList(target.as_str()),
            &node.device,
            node.store.owner(),
            *pinned,
        ) && client.leaving().is_ok()
        {
            told += 1;
        }
    }
    told
}

/// Say goodbye, as far as the network lets: never fails, because nobody is kept
/// on a phone by a network being down. The withdrawal comes last, since a
/// withdrawn device can no longer say anything for itself.
fn farewell(node: &Node, now: u64) -> Farewell {
    let told = tell_machines(node, &targets(node));
    let departed = coordinator::depart(node, now).unwrap_or(false);
    let withdrawn = coordinator::withdraw_self(node, now).is_ok();
    Farewell {
        told,
        departed,
        withdrawn,
    }
}

/// Tell the machines, withdraw this device, close and erase the node.
///
/// Holds the lock to the end: it waits for a running round, and `open_into`
/// cannot slip a new node into the home on its way out. A coordinator that could
/// not be told leaves the old id for the next login to withdraw.
pub fn logout_into(slot: &Mutex<Option<Node>>, home: &Path, now: u64) -> Answer {
    let mut guard = lock(slot)?;
    if guard.as_ref().is_none_or(|node| node.home != home) {
        return Err(Failure::Closed);
    }
    let Some(node) = guard.take() else {
        return Err(Failure::Closed);
    };

    let said = farewell(&node, now);
    let device = node.store.device_id();
    drop(node);
    if !said.withdrawn {
        remember_to_withdraw(home, device);
    }
    erase_home(home, now)?;
    Ok(json!({
        "told": said.told,
        "departed": said.departed,
        "withdrawn": said.withdrawn,
    })
    .to_string())
}

/// Delete the whole account at the coordinator, then log this phone out. Only
/// the account's own name, typed, gets this far; a refusal leaves everything.
pub fn delete_account(slot: &Mutex<Option<Node>>, home: &Path, confirm: &str, now: u64) -> Answer {
    let mut guard = lock(slot)?;
    if guard.as_ref().is_none_or(|node| node.home != home) {
        return Err(Failure::Closed);
    }
    let Some(node) = guard.as_ref() else {
        return Err(Failure::Closed);
    };
    if confirm.is_empty() || confirm != node.config.username {
        return Err(Failure::Usage(
            "the name typed is not this account's. Nothing was sent, and nothing was deleted."
                .to_owned(),
        ));
    }
    // Asked first: once the account is gone the coordinator lists no machines.
    let targets = targets(node);
    coordinator::delete_account(node, now)?;
    tell_machines(node, &targets);

    // No withdrawal and no marker: the account has no devices left to free.
    *guard = None;
    erase_home(home, now)?;
    Ok(json!({ "deleted": true }).to_string())
}

// ------------------------------------------------------------- the old home

/// Move `home` out of the way, so that from here on there is no node there.
fn set_aside(home: &Path, now: u64) -> Result<PathBuf, Failure> {
    let parent = home
        .parent()
        .ok_or_else(|| Failure::Usage(format!("{} has no parent", home.display())))?;
    let mut stamp = now;
    let mut aside = parent.join(format!("{REMOVING_PREFIX}{stamp}"));
    while aside.exists() {
        stamp += 1;
        aside = parent.join(format!("{REMOVING_PREFIX}{stamp}"));
    }
    std::fs::rename(home, &aside)
        .map_err(|error| Failure::Usage(format!("could not remove {}: {error}", home.display())))?;
    Ok(aside)
}

/// Remove the node at `home`, keys and all. Atomic as far as a later login can
/// see: what a failed delete leaves is [`sweep_removed`]'s.
fn erase_home(home: &Path, now: u64) -> Result<(), Failure> {
    let aside = set_aside(home, now)?;
    let _ = std::fs::remove_dir_all(aside);
    Ok(())
}

/// Finish any erase that was interrupted, beside `home`.
fn sweep_removed(home: &Path) {
    let Some(parent) = home.parent() else {
        return;
    };
    for entry in std::fs::read_dir(parent).into_iter().flatten().flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(REMOVING_PREFIX)
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

// -------------------------------------------- what a logout could not withdraw

fn pending_withdrawals(home: &Path) -> Option<PathBuf> {
    home.parent()
        .map(|parent| parent.join(WITHDRAW_AFTER_LOGIN))
}

/// Remember `device` for the next login. Added to whatever is there: two logouts
/// offline leave two slots to free.
fn remember_to_withdraw(home: &Path, device: DeviceId) {
    let Some(path) = pending_withdrawals(home) else {
        return;
    };
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&device.to_hex());
    text.push('\n');
    let _ = std::fs::write(path, text);
}

/// Withdraw what earlier logouts could not, best effort. An id is kept only when
/// the coordinator did not answer: another account's is refused there, and asking
/// again would be refused the same way.
fn withdraw_pending(node: &Node, now: u64) {
    let Some(path) = pending_withdrawals(&node.home) else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| {
            line.trim()
                .parse::<DeviceId>()
                .is_ok_and(|device| coordinator::withdraw_old_device(node, device, now).is_err())
        })
        .collect();
    if kept.is_empty() {
        let _ = std::fs::remove_file(path);
    } else {
        let _ = std::fs::write(path, kept.join("\n") + "\n");
    }
}

#[cfg(test)]
mod tests {
    /// The native screen must flag a misspelt word by its number and accept
    /// only a whole phrase, or a person types 24 words to be told "no".
    /// Sabotage: make `check_words` answer ok whatever it is given.
    #[test]
    fn checking_words_names_the_misspelt_one_and_accepts_only_a_whole_phrase() {
        let said: serde_json::Value =
            serde_json::from_str(&super::check_words("abandon zzzz")).unwrap();
        assert_eq!(
            said["wordIndex"], 2,
            "the wrong word was not pointed at: {said}"
        );
        assert_eq!(said["ok"], false, "two words passed as a phrase: {said}");
        let few: serde_json::Value = serde_json::from_str(&super::check_words("abandon")).unwrap();
        assert!(few["wordIndex"].is_null() && few["ok"] == false, "{few}");
    }

    use std::fmt::Write as _;

    use itsanas_crypto::UserKeys;

    use super::*;
    use crate::testing::{NOW, PASSPHRASE, live, phone, with_directory};

    const NEW_PASSPHRASE: &str = "a new passphrase for this phone";
    const GIB: u64 = 1 << 30;

    fn network(address: std::net::SocketAddr) -> Network {
        Network {
            address: address.to_string(),
            device: None,
        }
    }

    /// A disk that answers, roomy enough to matter in no test that does not ask.
    fn roomy() -> DiskSpace {
        DiskSpace {
            free: Some(100 * GIB),
            total: Some(128 * GIB),
        }
    }

    fn answer(text: &str) -> Value {
        serde_json::from_str(text).expect("JSON")
    }

    /// The words as the command line prints them, numbers and all.
    fn grid_of(phrase: &str) -> String {
        let mut grid = String::new();
        for (index, word) in phrase.split_whitespace().enumerate() {
            let _ = writeln!(grid, "{:>2}. {word}", index + 1);
        }
        grid
    }

    fn removing_beside(home: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(home.parent().expect("parent"))
            .expect("read")
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(REMOVING_PREFIX)
            })
            .map(|entry| entry.path())
            .collect()
    }

    #[test]
    fn a_name_no_coordinator_would_take_is_not_sent() {
        // Nothing listens at this address, so an answer other than "unknown"
        // would have to be made up, and a connection attempt would show as a
        // delay nobody measures: `valid` false is the proof it was not asked.
        for bad in ["marie_1", "Nicolas", "", "-nope", &"a".repeat(65)] {
            let said = answer(&check_name("127.0.0.1:1", bad));
            assert_eq!(said["valid"], false, "{bad:?}: {said}");
            assert_eq!(said["taken"], "unknown", "{bad:?}: {said}");
            assert!(
                said["why"].as_str().is_some_and(|why| !why.is_empty()),
                "a refused name with no reason: {said}"
            );
            assert!(
                !said["why"].as_str().unwrap_or("").contains("refused:"),
                "the screen would show the protocol's own prefix: {said}"
            );
        }
    }

    /// Free, taken and unreachable, from the real coordinator. Sabotage: answer
    /// "no" when the coordinator cannot be asked (a person walks into a taken
    /// name with nothing said).
    #[test]
    fn red_team_a_name_is_free_or_taken_as_the_coordinator_says_and_unknown_otherwise() {
        with_directory(|address, _| {
            let dir = tempfile::tempdir().expect("temp dir");
            let (holder, _) = phone(&dir.path().join("holder"), address, "camille", None);
            coordinator::register_with(&holder, None, NOW).expect("enrol");

            let at = address.to_string();
            let said = answer(&check_name(&at, "camille"));
            assert_eq!(
                (&said["valid"], &said["taken"]),
                (&json!(true), &json!("yes"))
            );
            let said = answer(&check_name(&at, "personne"));
            assert_eq!(
                (&said["valid"], &said["taken"]),
                (&json!(true), &json!("no"))
            );
            assert!(said["why"].is_null(), "{said}");
        });
        let said = answer(&check_name("127.0.0.1:1", "camille"));
        assert_eq!(
            (&said["valid"], &said["taken"]),
            (&json!(true), &json!("unknown")),
            "a coordinator that cannot be asked must not read as a free name: {said}"
        );
    }

    /// The Restore screen: which word, 1-based; a grid taken as saved; the words
    /// checked before the name is asked; the four things a name can be. Sabotage:
    /// report `bip39`'s own index; drop `phrase_words`; compare no owner (another
    /// account's name reads as yours, and Join later fails with nothing to do).
    #[test]
    fn red_team_the_restore_screen_says_which_word_and_whose_name() {
        with_directory(|address, _| {
            let dir = tempfile::tempdir().expect("temp dir");
            let (holder, phrase) = phone(&dir.path().join("holder"), address, "camille", None);
            let (stranger, _) = phone(&dir.path().join("stranger"), address, "voisin", None);
            for node in [&holder, &stranger] {
                coordinator::register_with(node, None, NOW).expect("enrol");
            }
            let at = address.to_string();

            let mut words: Vec<&str> = phrase.split_whitespace().collect();
            words[6] = "zzzz";
            let said = answer(&check_restore(&at, "camille", &words.join(" ")));
            assert_eq!(said["words"], "bad", "{said}");
            assert_eq!(said["wordIndex"], 7, "not the number on the sheet: {said}");
            assert_eq!(said["count"], 24, "{said}");
            assert_eq!(said["name"], "unknown", "the name was asked: {said}");
            assert!(
                !said["why"].as_str().unwrap_or("").contains("zzzz"),
                "a typed word was echoed: {said}"
            );

            let words: Vec<&str> = phrase.split_whitespace().take(23).collect();
            let said = answer(&check_restore(&at, "camille", &words.join(" ")));
            assert_eq!(
                (&said["words"], &said["count"], &said["wordIndex"]),
                (&json!("bad"), &json!(23), &Value::Null),
                "{said}"
            );

            let grid = grid_of(&phrase);
            let said = answer(&check_restore(&at, "camille", &grid));
            assert_eq!(
                (&said["words"], &said["name"]),
                (&json!("ok"), &json!("yours")),
                "a numbered grid, copied as it was shown, was refused: {said}"
            );

            let said = answer(&check_restore(&at, "voisin", &phrase));
            assert_eq!(said["name"], "other", "{said}");
            let said = answer(&check_restore(&at, "personne", &phrase));
            assert_eq!(said["name"], "none", "{said}");
            let said = answer(&check_restore(&at, "marie_1", &phrase));
            assert_eq!(said["name"], "invalid", "{said}");
            assert!(said["why"].as_str().is_some(), "{said}");
            let said = answer(&check_restore("127.0.0.1:1", "camille", &phrase));
            assert_eq!(said["name"], "unknown", "{said}");
        });
    }

    /// `newPhrase` is 24 words; `words` throws for a wrong passphrase;
    /// `changePassphrase` obeys the rule and needs the current one. Sabotage:
    /// answer an empty phrase instead of throwing.
    #[test]
    fn red_team_the_words_come_back_only_for_the_passphrase() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("phone");

        let fresh = answer(&new_phrase().expect("phrase"));
        let fresh = fresh["phrase"].as_str().expect("phrase");
        assert_eq!(fresh.split_whitespace().count(), 24);

        let (node, phrase) = Node::create(&home, PASSPHRASE, "camille").expect("create");
        drop(node);
        assert_eq!(
            answer(&words(&home, PASSPHRASE).expect("words"))["phrase"],
            phrase.as_str()
        );
        assert!(
            words(&home, "not the passphrase").is_err(),
            "the words were given to a wrong passphrase"
        );

        assert!(
            change_passphrase(&home, "not the passphrase", NEW_PASSPHRASE).is_err(),
            "a wrong current passphrase re-sealed the keystore"
        );
        let said = usage_of(change_passphrase(&home, PASSPHRASE, "short"));
        assert!(said.contains("128"), "{said}");
        assert_eq!(
            answer(&change_passphrase(&home, PASSPHRASE, NEW_PASSPHRASE).expect("change"))["changed"],
            true
        );
        assert!(words(&home, NEW_PASSPHRASE).is_ok());
        assert!(words(&home, PASSPHRASE).is_err(), "the old one still opens");
    }

    fn usage_of(outcome: Answer) -> String {
        match outcome {
            Err(refused) => refused.message(),
            Ok(text) => panic!("accepted: {text}"),
        }
    }

    /// A LOGOUT FREES ITS SLOT AND LEAVES NOTHING. The device is withdrawn, the
    /// coordinator told, the home gone with no marker and no leftovers, and a
    /// login at the same path mints a different device that enrols. Sabotage: skip
    /// `withdraw_self` (the slot is held for ever, ten logouts lock the account);
    /// `mem::forget` the node instead of dropping it (the erase fails on the open
    /// store, and the phone keeps its keys).
    #[test]
    fn red_team_a_phone_that_logs_out_frees_its_slot_and_leaves_nothing_behind() {
        with_directory(|address, directory| {
            let dir = tempfile::tempdir().expect("temp dir");
            let home = dir.path().join("node");
            let (node, phrase) = phone(&home, address, "camille", None);
            coordinator::register_with(&node, None, NOW).expect("enrol");
            let device = node.store.device_id();
            assert!(live(directory, device), "the fixture is not enrolled");

            let slot = Mutex::new(Some(node));
            let said = answer(&logout_into(&slot, &home, NOW + 1).expect("logout"));
            assert_eq!(said["withdrawn"], true, "{said}");
            assert_eq!(said["departed"], true, "{said}");
            assert!(
                !live(directory, device),
                "the phone logged out and kept its slot: ten logouts and the account can add \
                 no device"
            );
            assert!(
                slot.lock().expect("lock").is_none(),
                "the node is still held"
            );
            assert!(!home.exists(), "the home survived the logout");
            assert!(
                !Node::exists(&home),
                "the keystore survived the logout: the next person on this phone is asked to \
                 unlock it"
            );
            assert_eq!(removing_beside(&home), Vec::<PathBuf>::new());
            assert!(
                !dir.path().join(WITHDRAW_AFTER_LOGIN).exists(),
                "a device that was withdrawn was left to withdraw again"
            );

            let (again, _) = phone(&home, address, "camille", Some(&phrase));
            assert_ne!(
                again.store.device_id(),
                device,
                "a login after a logout reused the withdrawn id, which can never enrol again"
            );
            coordinator::register_with(&again, None, NOW + 2)
                .expect("the account could not come back on this phone");
        });
    }

    /// Nobody is kept on a phone by a network being down. With the coordinator
    /// unreachable the node is still erased and the id is left for later; a login
    /// that cannot reach it either keeps the id, and one that can withdraws it.
    /// Sabotage: return early when the withdrawal fails; delete the marker on any
    /// outcome (the slot is lost at the first offline login).
    #[test]
    fn red_team_an_unreachable_coordinator_still_logs_out_and_the_next_login_finishes_it() {
        with_directory(|address, directory| {
            let dir = tempfile::tempdir().expect("temp dir");
            let home = dir.path().join("node");
            let (mut node, phrase) = phone(&home, address, "camille", None);
            coordinator::register_with(&node, None, NOW).expect("enrol");
            let device = node.store.device_id();
            node.config.coordinator = Some("127.0.0.1:1".to_owned());

            let slot = Mutex::new(Some(node));
            let said = answer(&logout_into(&slot, &home, NOW + 1).expect("logout"));
            assert_eq!(said["withdrawn"], false, "{said}");
            assert_eq!(said["told"], 0, "{said}");
            assert!(
                !home.exists(),
                "an unreachable coordinator kept the keys here"
            );
            let marker = dir.path().join(WITHDRAW_AFTER_LOGIN);
            assert_eq!(
                std::fs::read_to_string(&marker).expect("marker").trim(),
                device.to_hex(),
                "nothing remembers which device to withdraw"
            );

            let offline = Network {
                address: "127.0.0.1:1".to_owned(),
                device: None,
            };
            let login = Login {
                home: &home,
                username: "camille",
                phrase: &phrase,
                passphrase: NEW_PASSPHRASE,
            };
            login_into(&slot, login, roomy(), &offline, NOW + 2).expect("offline login");
            assert!(
                marker.exists(),
                "a login that could not ask forgot the id, and its slot is lost"
            );
            assert!(live(directory, device), "no coordinator was asked yet");

            // Out again, and a login that reaches the coordinator.
            let said = answer(&logout_into(&slot, &home, NOW + 3).expect("logout"));
            assert_eq!(said["withdrawn"], false, "{said}");
            login_into(&slot, login, roomy(), &network(address), NOW + 4).expect("login");
            assert!(
                !live(directory, device),
                "the next login did not withdraw the old id: the slot is held for ever"
            );
            assert!(
                !marker.exists(),
                "a withdrawal already answered stays on the list"
            );
        });
    }

    /// A login from another account does not hang on to an id its own coordinator
    /// answered for: refused there, harmlessly, and not asked again. Sabotage:
    /// keep the id unless the answer was Done.
    #[test]
    fn a_withdrawal_for_another_account_is_dropped_once_answered() {
        with_directory(|address, directory| {
            let dir = tempfile::tempdir().expect("temp dir");
            let (other, _) = phone(&dir.path().join("other"), address, "voisin", None);
            coordinator::register_with(&other, None, NOW).expect("enrol");
            let foreign = other.store.device_id();

            let home = dir.path().join("node");
            let (mine, _) = phone(&home, address, "camille", None);
            coordinator::register_with(&mine, None, NOW).expect("enrol");
            remember_to_withdraw(&home, foreign);

            withdraw_pending(&mine, NOW + 1);
            assert!(
                live(directory, foreign),
                "another account's device was withdrawn"
            );
            assert!(
                !dir.path().join(WITHDRAW_AFTER_LOGIN).exists(),
                "an id its coordinator refused is asked again at every login"
            );
        });
    }

    /// "I FORGOT MY PASSPHRASE" REFUSES BEFORE IT TOUCHES ANYTHING. Bad words, a
    /// passphrase outside the rule, and another account's words each leave the
    /// keystore byte for byte; the right words replace the node, under a new
    /// device, with the old one left to withdraw. Sabotage: erase before asking the
    /// coordinator; skip the owner check (another account's words replace this one);
    /// skip `check_new_passphrase` (a refused passphrase comes after the erase, and
    /// the account is gone from this phone).
    #[test]
    fn red_team_a_forgotten_passphrase_is_replaced_only_by_this_account_s_words() {
        with_directory(|address, directory| {
            let dir = tempfile::tempdir().expect("temp dir");
            let home = dir.path().join("node");
            let (node, phrase) = phone(&home, address, "camille", None);
            let (stranger, theirs) = phone(&dir.path().join("x"), address, "voisin", None);
            for enrolled in [&node, &stranger] {
                coordinator::register_with(enrolled, None, NOW).expect("enrol");
            }
            let (old_device, owner) = (node.store.device_id(), node.store.owner());
            // The sync service holds it, and the passphrase is lost.
            let slot = Mutex::new(Some(node));
            let keystore = std::fs::read(home.join("keystore.bin")).expect("keystore");

            let attempt = |phrase: &str, passphrase: &str| {
                let login = Login {
                    home: &home,
                    username: "",
                    phrase,
                    passphrase,
                };
                restore_over(&slot, login, roomy(), &network(address), NOW + 1)
            };
            for (what, words, passphrase) in [
                ("bad words", "not the words at all", NEW_PASSPHRASE),
                ("another account's words", theirs.as_str(), NEW_PASSPHRASE),
                ("a passphrase outside the rule", phrase.as_str(), "short"),
            ] {
                assert!(
                    attempt(words, passphrase).is_err(),
                    "{what} replaced the node"
                );
                assert_eq!(
                    std::fs::read(home.join("keystore.bin")).expect("keystore"),
                    keystore,
                    "{what} touched the keystore"
                );
                assert!(
                    slot.lock().expect("lock").is_some(),
                    "{what} closed the node the service holds"
                );
            }

            let grid = grid_of(&phrase);
            let said = answer(&attempt(&grid, NEW_PASSPHRASE).expect("the right words"));
            assert_eq!(said["username"], "camille", "{said}");
            assert_eq!(said["userId"], owner.to_string(), "{said}");
            assert_ne!(said["deviceId"], old_device.to_string(), "{said}");
            drop(slot);

            let reopened = Node::open(&home, NEW_PASSPHRASE).expect("open under the new one");
            assert_eq!(reopened.store.owner(), owner);
            assert!(
                !live(directory, old_device),
                "the replaced device kept its slot"
            );
            assert_eq!(removing_beside(&home), Vec::<PathBuf>::new());
        });
    }

    /// Offline, the words are still enough: a phone with a forgotten passphrase is
    /// not left waiting for a network, and the old id waits for a later login.
    #[test]
    fn a_forgotten_passphrase_can_be_replaced_with_no_network() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("node");
        let (node, phrase) = Node::create(&home, PASSPHRASE, "camille").expect("create");
        let (device, owner) = (node.store.device_id(), node.store.owner());
        drop(node);

        let slot = Mutex::new(None);
        let offline = Network {
            address: "127.0.0.1:1".to_owned(),
            device: None,
        };
        let login = Login {
            home: &home,
            username: "",
            phrase: phrase.as_str(),
            passphrase: NEW_PASSPHRASE,
        };
        let said = answer(&restore_over(&slot, login, roomy(), &offline, NOW).expect("restore"));
        assert_eq!(said["userId"], owner.to_string(), "{said}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(WITHDRAW_AFTER_LOGIN))
                .expect("marker")
                .trim(),
            device.to_hex()
        );
    }

    /// A login takes the grid, sweeps what an interrupted erase left, and sets the
    /// budget silently, with the pledge untouched. Sabotage: leave `keep` unset (a
    /// restore downloads until the disk is full); set the suggested pledge as well
    /// (hosting is opt-in).
    #[test]
    fn red_team_a_login_sets_a_budget_for_the_disk_and_offers_nothing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("node");
        let phrase = MasterSecret::generate()
            .and_then(|master| master.to_recovery_phrase())
            .expect("phrase");
        let leftover = dir.path().join(format!("{REMOVING_PREFIX}5"));
        std::fs::create_dir_all(leftover.join("vault")).expect("leftover");
        let grid = grid_of(&phrase);

        let slot = Mutex::new(None);
        let offline = Network {
            address: "127.0.0.1:1".to_owned(),
            device: None,
        };
        let login = Login {
            home: &home,
            username: "camille",
            phrase: &grid,
            passphrase: NEW_PASSPHRASE,
        };
        let disk = DiskSpace {
            free: Some(30 * GIB),
            total: Some(64 * GIB),
        };
        login_into(&slot, login, disk, &offline, NOW).expect("login");

        let saved = Config::load(&Node::config_path(&home)).expect("config");
        assert_eq!(
            saved.keep_bytes,
            Some(10 * GIB),
            "no budget, or not the disk's: a restore of a large account fills the phone"
        );
        assert_eq!(saved.pledge_bytes, 0, "a pledge was set with nobody asked");
        assert!(!leftover.exists(), "an interrupted erase was not swept");
    }

    /// A disk that will not say gets no made-up budget.
    #[test]
    fn a_disk_that_will_not_say_gets_no_budget() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (mut node, _) =
            Node::create(&dir.path().join("node"), PASSPHRASE, "camille").expect("create");
        set_keep_budget(&mut node, DiskSpace::default());
        assert_eq!(node.config.keep_bytes, None);
    }

    /// Deleting takes the account's own name, sends nothing otherwise, and a
    /// refused deletion leaves the phone as it was. Sabotage: skip the comparison
    /// (a mistyped name deletes the account for ever).
    #[test]
    fn red_team_deleting_the_account_takes_its_own_name_typed() {
        with_directory(|address, directory| {
            let dir = tempfile::tempdir().expect("temp dir");
            let home = dir.path().join("node");
            let (node, _) = phone(&home, address, "camille", None);
            coordinator::register_with(&node, None, NOW).expect("enrol");
            let device = node.store.device_id();

            // The coordinator refuses a deletion dated long ago.
            let now = itsanas_discover::now_unix();
            let slot = Mutex::new(Some(node));
            for wrong in ["", "Camille", "voisin"] {
                let said = usage_of(delete_account(&slot, &home, wrong, now));
                assert!(said.contains("Nothing was sent"), "{said}");
                assert!(Node::exists(&home), "{wrong:?} erased the phone");
                assert!(live(directory, device), "{wrong:?} reached the coordinator");
            }

            let said = answer(&delete_account(&slot, &home, "camille", now).expect("delete"));
            assert_eq!(said["deleted"], true, "{said}");
            assert!(!home.exists(), "the phone kept the deleted account");
            assert!(
                slot.lock().expect("lock").is_none(),
                "the node is still held"
            );
            let looked = coordinator::name_owner(&address.to_string(), "camille").expect("asked");
            assert_eq!(looked, None, "the coordinator still lists the account");
        });
    }

    /// A deletion the coordinator refuses throws its words and erases nothing: a
    /// phone that erased itself on a refusal has lost the account's keys and the
    /// account still exists. Sabotage: ignore the error and erase.
    #[test]
    fn a_refused_deletion_erases_nothing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("node");
        let (mut node, _) = Node::create(&home, PASSPHRASE, "camille").expect("create");
        node.config.coordinator = Some("127.0.0.1:1".to_owned());
        let slot = Mutex::new(Some(node));
        assert!(
            delete_account(&slot, &home, "camille", NOW).is_err(),
            "an unreachable coordinator counted as deleted"
        );
        assert!(Node::exists(&home), "a failed deletion erased the phone");
        assert!(slot.lock().expect("lock").is_some(), "and closed the node");
    }

    /// Logging out takes the node it is told about, or none: Kotlin passes the path
    /// it has, and a stale one must not erase whichever home is open.
    #[test]
    fn a_logout_of_a_home_not_open_erases_nothing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("node");
        let (node, _) = Node::create(&home, PASSPHRASE, "camille").expect("create");
        let slot = Mutex::new(Some(node));
        assert!(matches!(
            logout_into(&slot, &dir.path().join("elsewhere"), NOW),
            Err(Failure::Closed)
        ));
        assert!(Node::exists(&home));
        assert!(slot.lock().expect("lock").is_some());

        let empty = Mutex::new(None);
        assert!(matches!(
            logout_into(&empty, &home, NOW),
            Err(Failure::Closed)
        ));
        assert!(
            Node::exists(&home),
            "a closed node was erased with no check"
        );
    }

    #[test]
    fn the_words_alone_decide_the_account() {
        let master = MasterSecret::generate().expect("master");
        let phrase = master.to_recovery_phrase().expect("phrase");
        assert_eq!(
            user_of(phrase.as_str()).expect("user"),
            UserKeys::derive(&master).user_id()
        );
        assert!(user_of("not words").is_err());
    }
}
