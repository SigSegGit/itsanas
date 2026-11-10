//! What every phone shell asks the core, without the language boundary.
//!
//! Android reaches these through JNI (`itsanas-android`) and iOS through C
//! (`itsanas-ios`). The boundary crates convert arguments and errors and
//! nothing else, so a file listed, fetched or added on either phone goes
//! through the same lines, and a fix made for one is made for both.

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

/// Put a file in this phone's trash for `days`.
pub fn trash_put(node: &Node, path: &str, days: u64) -> Answer {
    trash::put(&node.home, path, unix_now(), days).map_err(|error| trash_failed(&error))?;
    Ok("{}".to_owned())
}

/// What is in the trash, after deleting what is due, and what left the folder.
pub fn trash_list(node: &Node) -> Answer {
    let trashed = purged_trash(node)?;
    let account = itsanas_store::catalogue(&node.store, &node.vault)?
        .files
        .into_iter()
        .map(|file| file.path)
        .collect();
    let rows: Vec<_> = trashed
        .iter()
        .map(|(path, due)| serde_json::json!({ "path": path, "due": due }))
        .collect();
    Ok(serde_json::json!({
        "now": unix_now(),
        "trashed": rows,
        "left": trash::left_the_folder(&node.home, &account),
    })
    .to_string())
}

/// Keep a file that left the chosen folder: its record alone, no node needed.
pub fn trash_keep(home: &Path, path: &str) -> Answer {
    trash::keep(home, path).map_err(|error| trash_failed(&error))?;
    Ok("{}".to_owned())
}

/// Take a file out of the trash: its record alone, no node needed.
pub fn trash_restore(home: &Path, path: &str) -> Answer {
    trash::take_out(home, path).map_err(|error| trash_failed(&error))?;
    Ok("{}".to_owned())
}

/// Delete a file from every machine now.
pub fn trash_delete(node: &Node, path: &str) -> Answer {
    delete_for_good(node, path)?;
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
pub fn purged_trash(node: &Node) -> Result<std::collections::BTreeMap<String, u64>, Failure> {
    for path in trash::due(&node.home, unix_now()) {
        delete_for_good(node, &path)?;
    }
    Ok(trash::list(&node.home))
}

pub fn delete_for_good(node: &Node, path: &str) -> Result<(), Failure> {
    if itsanas_store::remove_known(&node.store, &node.vault, path)? {
        node.store.flush_segment()?;
    }
    trash::take_out(&node.home, path).map_err(|error| trash_failed(&error))
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

    let totals = run_round(node, &targets, scope);
    crate::note_phone_contact(&node.home, totals.reached > 0, now);
    Ok(serde_json::json!({
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
