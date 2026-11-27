//! A phone's trash: a delete by this machine's rules, what the trash holds,
//! a file kept, brought back or deleted for good, and what is due purged.

use std::collections::BTreeMap;
use std::path::Path;

use itsanas_node::Node;

use crate::shell::{Answer, Failure};
use crate::trash;

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
    // Before the purge: a file edited elsewhere since it was trashed is wanted.
    deletion::untrash_edited(&node.store, &node.vault)?;
    for path in deletion::due_shared(&node.store, &node.vault, now)? {
        deletion::purge(&node.store, &node.vault, &path)?;
    }
    deletion::release_protected(node.store.root(), None, now)
        .map_err(|error| trash_failed(&error))?;
    // Once a day, the traces older than the account keeps them go.
    itsanas_store::traces::sweep(&node.store, &node.vault, now)?;
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
