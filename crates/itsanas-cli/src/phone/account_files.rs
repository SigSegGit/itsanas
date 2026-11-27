//! A phone's files: fetched, added, renamed, an earlier version brought back,
//! and listed, the trash hidden.

use itsanas_node::Node;

use super::{purged_trash, trash_failed};
use crate::shell::{Answer, Failure, NO_MACHINE_YET, targets};
use crate::trash;

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

    // A machine that answered and did not serve them is not asked again at
    // its other addresses.
    let mut asked = std::collections::BTreeSet::new();
    // The account's machines, then the hosts of other accounts the
    // coordinator names, which keep its chunks too: the coordinator is asked
    // only once every machine of the account was tried.
    let hosts = std::iter::once_with(|| crate::host_targets(node)).flatten();
    for (target, pinned) in targets.into_iter().chain(hosts) {
        if pinned.is_some_and(|device| asked.contains(&device)) {
            continue;
        }
        let Ok(mut client) = crate::rounds::connect(node, &target, pinned) else {
            continue;
        };
        asked.insert(client.peer_device());
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
    // The account's trash too: a delete now lands there by default, and a file
    // added again would stay hidden and leave every machine when it came due.
    itsanas_store::deletion::restore_shared(&node.store, &node.vault, path)?;
    Ok(entry)
}

/// Give a file a new name, or a folder a new place, on every machine: what a
/// phone's Rename does. A file the phone has not downloaded moves too. A name
/// taken, a file in the trash or nothing at the old name is a sentence for the
/// person, and nothing moved.
pub fn rename(node: &Node, from: &str, to: &str) -> Answer {
    use itsanas_store::Renamed;
    match itsanas_store::rename_known(&node.store, &node.vault, from, to)? {
        Renamed::Moved(count) => Ok(serde_json::json!({ "moved": count }).to_string()),
        Renamed::NoSuchFile => Err(Failure::Usage(format!("{from} is not in the account"))),
        Renamed::Taken(name) => Err(Failure::Usage(format!(
            "{name} is already a file of the account; nothing moved"
        ))),
        Renamed::InTrash(name) => Err(Failure::Usage(format!(
            "{name} is in the trash; put it back first"
        ))),
    }
}

/// Every version of `path` the account knows, newest first, for a phone's
/// Versions list: when, how big, whether it is the file now, whether this phone
/// can bring it back, and whether this phone wrote it.
pub fn versions_of(node: &Node, path: &str) -> Answer {
    let mine = node.store.device_id();
    let listed: Vec<serde_json::Value> = itsanas_store::versions(&node.store, &node.vault, path)?
        .into_iter()
        .map(|version| {
            serde_json::json!({
                "modified": version.modified_unix,
                "size": version.size,
                "current": version.current,
                "here": version.here,
                "mine": version.author == mine,
            })
        })
        .collect();
    Ok(serde_json::json!({ "versions": listed }).to_string())
}

/// Make version `at` (0 is the newest) of `path` the file again, on every
/// machine. One not on this phone is fetched first from a machine of the
/// account that holds it; none answering is a sentence.
pub fn bring_back(node: &Node, path: &str, at: usize) -> Answer {
    let here = itsanas_store::versions(&node.store, &node.vault, path)?
        .get(at)
        .is_some_and(|version| version.here);
    if !here {
        crate::fetch_version_here(node, path, at);
    }
    if itsanas_store::bring_back_version(&node.store, &node.vault, path, at)? {
        Ok("{}".to_owned())
    } else {
        Err(Failure::Usage(
            "no machine of this account that answered holds that version; try again when the one that wrote it is up"
                .to_owned(),
        ))
    }
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
