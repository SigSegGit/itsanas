//! What every phone shell asks the core, without the language boundary.
//!
//! Android reaches these through JNI (`itsanas-android`) and iOS through C
//! (`itsanas-ios`). The boundary crates convert arguments and errors and
//! nothing else, so a file listed, fetched or added on either phone goes
//! through the same lines, and a fix made for one is made for both.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use itsanas_node::Node;

use crate::shell::{Answer, Failure, NO_MACHINE_YET, Network, describe, run_round, targets};
use crate::trash;

#[must_use]
pub fn passphrase_rule() -> String {
    serde_json::json!({
        "min": itsanas_node::node::MIN_PASSPHRASE_CHARS,
        "max": itsanas_node::node::MAX_PASSPHRASE_CHARS,
    })
    .to_string()
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

/// What a phone's `get` answers, for every phone shell.
pub fn get_file(node: &Node, path: &str, destination: &str) -> Answer {
    let content = match node.store.read_file(path)? {
        Some(content) => content,
        None => fetch(node, path)?,
    };

    std::fs::write(destination, &content)
        .map_err(|error| Failure::Usage(format!("could not write {destination}: {error}")))?;

    Ok(serde_json::json!({ "bytes": content.len() }).to_string())
}

/// Fetch a file this device knows about and has not downloaded.
///
/// The same walk `itsanas get` does: resolve the chunks from the log, ask each
/// of the account's machines in turn, stop at the first that serves them.
pub fn fetch(node: &Node, path: &str) -> Result<Vec<u8>, Failure> {
    let Some(chunks) = itsanas_store::chunks_for(&node.store, &node.vault, path)? else {
        return Err(Failure::Usage(format!("no such file: {path}")));
    };
    let wanted: std::collections::BTreeSet<_> = chunks.into_iter().collect();

    let targets = targets(node);
    if targets.is_empty() {
        return Err(Failure::Usage(format!(
            "{path} is in this account and not on this device, and {NO_MACHINE_YET}"
        )));
    }

    for (target, pinned) in &targets {
        let Ok(mut client) = itsanas_net::PeerClient::connect(
            itsanas_net::AddressList(target.as_str()),
            &node.device,
            node.store.owner(),
            *pinned,
        ) else {
            continue;
        };
        let _ = node.introduce(&mut client, itsanas_discover::now_unix());

        if itsanas_net::session::fetch_only(&node.store, &node.vault, &mut client, &wanted).is_err()
        {
            continue;
        }

        if let Some(content) = node.store.read_file(path)? {
            return Ok(content);
        }
    }

    Err(Failure::Usage(format!(
        "{path} is in this account and no machine that is up would serve it"
    )))
}

/// How old the account's other pledges may be before a write asks again. So a
/// file added offline waits on a connect timeout once an hour, not every time.
pub const PLEDGES_FRESH_FOR: std::time::Duration = std::time::Duration::from_secs(3600);

/// Bring what the account's other machines pledge up to date, if it is old:
/// without it a phone is held to the joining allowance for the whole account,
/// however much the owner's other machines lend. Written back unchanged when the
/// coordinator could not say, so the file's age is when this last asked.
pub fn refresh_old_pledges(node: &Node, now: std::time::SystemTime) {
    if Node::others_pledged_age(&node.home, now).is_some_and(|age| age < PLEDGES_FRESH_FOR) {
        return;
    }
    if !matches!(
        itsanas_node::coordinator::refresh_others_pledged(node),
        Ok(Some(_))
    ) {
        let _ = node.remember_others_pledged(Node::others_pledged(&node.home));
    }
}

/// What a phone's `put` answers, for every phone shell.
pub fn put_file(node: &Node, path: &str, source: &str) -> Answer {
    let entry = write_file(node, path, source)?;
    node.store.flush_segment()?;

    Ok(serde_json::json!({
        "bytes": entry.size,
        "chunks": entry.chunks.len(),
    })
    .to_string())
}

/// Write `source` at `path`, without sealing the segment: a caller that writes
/// many files seals once, so a first run of thousands of photos is not
/// thousands of segments (issue 93). A file written again under a path in the
/// trash leaves the trash: the person just put it back.
///
/// # Errors
///
/// When `source` cannot be read, the account has no room, or the store or the
/// trash cannot be written.
pub fn write_file(
    node: &Node,
    path: &str,
    source: &str,
) -> Result<itsanas_store::FileEntry, Failure> {
    let file = std::fs::File::open(source)
        .map_err(|error| Failure::Usage(format!("could not read {source}: {error}")))?;
    let incoming = file
        .metadata()
        .map_err(|error| Failure::Usage(format!("could not read {source}: {error}")))?
        .len();
    refresh_old_pledges(node, std::time::SystemTime::now());
    node.bound_writes()?;
    node.store.check_room(path, incoming)?;
    let entry = node.store.write_stream(path, file)?;
    trash::take_out(&node.home, path).map_err(|error| trash_failed(&error))?;
    Ok(entry)
}

/// Delete a file as a person on this machine does, by this machine's rules
/// ([`itsanas_store::deletion::rules_here`]); `_days` is what a phone sent
/// before the rules lived in the account, and is not used.
pub fn trash_put(node: &Node, path: &str, _days: u64) -> Answer {
    let became = itsanas_store::trash::delete(&node.store, &node.vault, path, unix_now())?;
    Ok(serde_json::json!({ "became": became_word(became) }).to_string())
}

/// The word a page or a phone shows for what a delete became.
#[must_use]
pub const fn became_word(became: itsanas_store::trash::Deleted) -> &'static str {
    use itsanas_store::trash::Deleted;
    match became {
        Deleted::Unknown => "unknown",
        Deleted::Trashed => "trashed",
        Deleted::Everywhere => "everywhere",
        Deleted::Waiting => "waiting",
        Deleted::HereOnly => "here-only",
    }
}

/// What is in the trash, after deleting what is due, and what left the folder.
pub fn trash_list(node: &Node) -> Answer {
    let trashed = purged_trash(node)?;
    let account = itsanas_store::catalogue(&node.store, &node.vault)?
        .files
        .into_iter()
        .map(|file| file.path)
        .collect();
    let mut answer = trash_rows(node, &trashed)?;
    answer["left"] = serde_json::json!(trash::left_the_folder(&node.home, &account));
    Ok(answer.to_string())
}

/// The trash as a page or a phone shows it: each file with when it goes and
/// what kind of entry it is (`shared`, the account's trash; `waiting`, a
/// definitive delete to confirm here; `protected`, deleted elsewhere and kept
/// here; `here`, this machine's own record), and this machine's rules.
///
/// # Errors
///
/// When the store cannot be read.
pub fn trash_rows(
    node: &Node,
    hidden: &BTreeMap<String, u64>,
) -> Result<serde_json::Value, Failure> {
    use itsanas_store::deletion;
    let shared = deletion::shared(&node.store, &node.vault)?;
    let mut rows: Vec<_> = hidden
        .iter()
        .map(|(path, due)| {
            let kind = if shared.contains_key(path) {
                "shared"
            } else if *due >= itsanas_store::trash::MAX_DAYS * itsanas_store::trash::DAY {
                "waiting"
            } else {
                "here"
            };
            let by = shared.get(path).map(|trashed| trashed.by.clone());
            serde_json::json!({ "path": path, "due": due, "kind": kind, "by": by })
        })
        .collect();
    for (path, (due, _)) in deletion::protected(node.store.root()) {
        rows.push(serde_json::json!({ "path": path, "due": due, "kind": "protected" }));
    }
    let rules = deletion::rules_here(&node.store, &node.vault)?;
    Ok(serde_json::json!({
        "now": unix_now(),
        "trashed": rows,
        "ask_auth": rules.ask_auth,
        "confirm": rules.confirm,
        "mode": rules.mode.word(),
        "days": rules.days,
    }))
}

/// Keep a file that left the chosen folder: its record alone, no node needed.
pub fn trash_keep(home: &Path, path: &str) -> Answer {
    trash::keep(home, path).map_err(|error| trash_failed(&error))?;
    Ok("{}".to_owned())
}

/// The trash from its records alone, no node: this machine's record, the
/// account's trash as last noted, and what is protected here. A round holds
/// the node for as long as it takes, and the screen said the trash was empty
/// meanwhile.
pub fn trash_record(home: &Path) -> Answer {
    use itsanas_store::{deletion, trash::DAY, trash::MAX_DAYS};
    let dir = trash::record_dir(home);
    let shared = deletion::noted_shared(&dir);
    let mut rows: Vec<_> = trash::list(home)
        .into_iter()
        .filter(|(path, _)| !shared.contains_key(path))
        .map(|(path, due)| {
            let kind = if due >= MAX_DAYS * DAY {
                "waiting"
            } else {
                "here"
            };
            serde_json::json!({ "path": path, "due": due, "kind": kind })
        })
        .collect();
    for (path, due) in shared {
        rows.push(serde_json::json!({ "path": path, "due": due, "kind": "shared" }));
    }
    for (path, (due, _)) in deletion::protected(&dir) {
        rows.push(serde_json::json!({ "path": path, "due": due, "kind": "protected" }));
    }
    Ok(serde_json::json!({ "now": unix_now(), "trashed": rows, "left": [] }).to_string())
}

/// Take a file out of this machine's own record: no node needed. A file of the
/// account's trash, or one protected here, needs [`trash_restore_in`].
pub fn trash_restore(home: &Path, path: &str) -> Answer {
    trash::take_out(home, path).map_err(|error| trash_failed(&error))?;
    Ok("{}".to_owned())
}

/// Bring a file back, wherever it waits: the account's trash (back on every
/// machine), protected here (published again from here), or this machine's
/// own record.
pub fn trash_restore_in(node: &Node, path: &str) -> Answer {
    use itsanas_store::deletion;
    if !deletion::restore_shared(&node.store, &node.vault, path)?
        && !deletion::restore_protected(&node.store, path)?
    {
        trash::take_out(&node.home, path).map_err(|error| trash_failed(&error))?;
    }
    Ok("{}".to_owned())
}

/// Delete a file of the trash for good: out of the account's trash and from
/// every machine; a protected one is let go here; one waiting here is deleted
/// from every machine.
pub fn trash_delete(node: &Node, path: &str) -> Answer {
    use itsanas_store::deletion;
    let named = [path.to_owned()];
    if deletion::shared(&node.store, &node.vault)?.contains_key(path) {
        deletion::purge(&node.store, &node.vault, path)?;
    } else if deletion::release_protected(node.store.root(), Some(&named), unix_now())
        .map_err(|error| trash_failed(&error))?
        == 0
    {
        delete_for_good(node, path)?;
    }
    Ok("{}".to_owned())
}

#[must_use]
pub fn trash_failed(error: &std::io::Error) -> Failure {
    Failure::Usage(format!("the trash could not be written: {error}"))
}

#[must_use]
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Really delete what is due, then answer what stays in the trash. The record
/// goes last, so a crash between the two only repeats a delete that finds nothing.
///
/// The account's trash is emptied of what is due by whichever machine sees it
/// first (both deleting it is one delete), and protected files whose days are
/// over are let go. What is hidden here is this machine's record and the
/// account's trash, which is also noted next to the store for a screen that
/// cannot wait for the node ([`itsanas_store::deletion::noted_shared`]).
pub fn purged_trash(node: &Node) -> Result<BTreeMap<String, u64>, Failure> {
    use itsanas_store::deletion;
    let now = unix_now();
    for path in trash::due(&node.home, now) {
        delete_for_good(node, &path)?;
    }
    for path in deletion::due_shared(&node.store, &node.vault, now)? {
        deletion::purge(&node.store, &node.vault, &path)?;
    }
    deletion::release_protected(node.store.root(), None, now)
        .map_err(|error| trash_failed(&error))?;
    let mut hidden = trash::list(&node.home);
    let shared: BTreeMap<String, u64> = deletion::shared(&node.store, &node.vault)?
        .into_values()
        .map(|trashed| (trashed.path, trashed.due))
        .collect();
    deletion::note_shared(node.store.root(), &shared).map_err(|error| trash_failed(&error))?;
    hidden.extend(shared);
    Ok(hidden)
}

pub fn delete_for_good(node: &Node, path: &str) -> Result<(), Failure> {
    if itsanas_store::remove_known(&node.store, &node.vault, path)? {
        node.store.flush_segment()?;
    }
    trash::take_out(&node.home, path).map_err(|error| trash_failed(&error))
}

/// Each file's size and date as the listing knows them: two of these, taken
/// around a round, say what the round changed.
pub type Versions = BTreeMap<String, (u64, u64)>;

/// How many names of each kind a round's report gives; the rest are counted.
const NAMED: usize = 5;

fn versions(node: &Node) -> Versions {
    itsanas_store::catalogue(&node.store, &node.vault)
        .map(|listing| {
            listing
                .files
                .into_iter()
                .map(|file| (file.path, (file.size, file.modified_unix)))
                .collect()
        })
        .unwrap_or_default()
}

/// What a round changed, as a person reads it: the files that arrived, changed
/// and left, a few named and all counted. A phone's message used to say only
/// that a round ran, so a file arriving from a computer went unseen.
#[must_use]
pub fn changes(before: &Versions, after: &Versions) -> serde_json::Value {
    let came: Vec<&String> = after
        .keys()
        .filter(|path| !before.contains_key(*path))
        .collect();
    let edited: Vec<&String> = after
        .iter()
        .filter(|(path, version)| before.get(*path).is_some_and(|was| was != *version))
        .map(|(path, _)| path)
        .collect();
    let gone: Vec<&String> = before
        .keys()
        .filter(|path| !after.contains_key(*path))
        .collect();
    let part = |paths: &[&String]| {
        serde_json::json!({
            "count": paths.len(),
            "names": paths.iter().take(NAMED).collect::<Vec<_>>(),
        })
    };
    serde_json::json!({ "new": part(&came), "changed": part(&edited), "gone": part(&gone) })
}

/// What a phone's `sync` answers, for every phone shell.
///
/// Enrols first, best effort: a person has no step for it, and a phone that
/// never enrolled holds no slot, no name, and cannot read what its machines
/// pledge. Then what they pledge, which is the bound `bound_writes` sets.
pub fn sync_node(node: &Node, metadata_only: bool, now: u64) -> Answer {
    let scope = if metadata_only {
        itsanas_net::session::Scope::Metadata
    } else {
        itsanas_net::session::Scope::Everything
    };

    let _ = itsanas_node::coordinator::ensure_enrolled(node, now);
    let _ = itsanas_node::coordinator::refresh_others_pledged(node);

    let targets = targets(node);
    if targets.is_empty() {
        // The wait starts here: a phone with no machine yet is reaching nobody.
        crate::note_phone_contact(&node.home, false, now);
        return Err(Failure::Usage(NO_MACHINE_YET.to_owned()));
    }

    // What comes in is held to the disk bound (8.1b); without this the
    // ceiling is the one opening the node set, which is none.
    node.bound_writes()?;

    let before = versions(node);
    let totals = run_round(node, &targets, scope);
    crate::note_phone_contact(&node.home, totals.reached > 0, now);
    let changed = changes(&before, &versions(node));
    Ok(serde_json::json!({
        "new": changed["new"],
        "changed": changed["changed"],
        "gone": changed["gone"],
        "reached": totals.reached,
        "adopted": totals.adopted,
        "sent": totals.sent,
        "released": totals.released,
        "freed": totals.freed,
        "unreachable": totals.unreachable,
    })
    .to_string())
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

/// Every file of the account, the trash hidden: what a phone's `list` answers.
pub fn list_all(node: &Node) -> Answer {
    let hidden = purged_trash(node)?;
    let listing = itsanas_store::catalogue(&node.store, &node.vault)?;
    let files: Vec<_> = listing
        .files
        .iter()
        .filter(|file| !hidden.contains_key(&file.path))
        .map(|file| {
            serde_json::json!({
                "path": file.path,
                "size": file.size,
                "modified": file.modified_unix,
                "here": file.presence == itsanas_store::Presence::Local,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "files": files,
        "complete": listing.complete,
    })
    .to_string())
}

/// One folder of the account, the trash hidden: what a phone's `listAt` answers.
pub fn list_at(node: &Node, folder: &str) -> Answer {
    let hidden = purged_trash(node)?;
    let listing = itsanas_store::catalogue(&node.store, &node.vault)?;
    let entries: Vec<_> = listing
        .files
        .iter()
        .filter(|file| !hidden.contains_key(&file.path))
        .map(|file| crate::file_tree::Entry {
            path: file.path.clone(),
            size: file.size,
            modified_unix: file.modified_unix,
            here: file.presence == itsanas_store::Presence::Local,
        })
        .collect();
    let level = crate::file_tree::level(&entries, folder);
    Ok(serde_json::json!({
        "at": level.at,
        "folders": level.folders.iter().map(|folder| serde_json::json!({
            "name": folder.name,
            "path": folder.path,
            "files": folder.files,
            "size": folder.size,
            "notHere": folder.not_here,
        })).collect::<Vec<_>>(),
        "files": level.files.iter().map(|file| serde_json::json!({
            "path": file.path,
            "size": file.size,
            "modified": file.modified_unix,
            "here": file.here,
        })).collect::<Vec<_>>(),
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A round's report names what arrived, what changed and what left, and
    /// counts past the few it names. Sabotage: compare paths only, so an edit
    /// made on a computer goes unreported.
    #[test]
    fn red_team_a_round_names_the_files_it_brought_changed_and_removed() {
        let mut before = Versions::new();
        before.insert("kept.txt".to_owned(), (3, 10));
        before.insert("edited.txt".to_owned(), (3, 10));
        before.insert("gone.txt".to_owned(), (3, 10));
        let mut after = before.clone();
        after.remove("gone.txt");
        after.insert("edited.txt".to_owned(), (4, 20));
        for n in 0..7 {
            after.insert(format!("photos/{n}.jpg"), (1, 30));
        }
        let said = changes(&before, &after);
        assert_eq!(said["new"]["count"], 7, "{said}");
        assert_eq!(
            said["new"]["names"].as_array().map(Vec::len),
            Some(NAMED),
            "{said}"
        );
        assert_eq!(
            said["changed"]["names"][0], "edited.txt",
            "an edit from a computer went unreported: {said}"
        );
        assert_eq!(said["gone"]["names"][0], "gone.txt", "{said}");
        assert_eq!(
            said["changed"]["count"], 1,
            "a file nobody touched was reported changed: {said}"
        );
    }
}
