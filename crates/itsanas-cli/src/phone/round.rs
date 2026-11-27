//! A phone's round: `sync`, and what it says the round changed.

use std::collections::BTreeMap;

use itsanas_node::Node;

use crate::shell::{Answer, Failure, NO_MACHINE_YET, run_round, targets};

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

    let mut targets = targets(node);
    // The account's machines on this network, asked, ahead of the rest: a
    // phone runs no daemon to hear their beacons, and at home with no
    // coordinator to read it reached none. Not when an address was typed in:
    // the person said where.
    if node.config.peers.is_empty() {
        let mut near = crate::rounds::nearby(node);
        near.retain(|(at, _)| !targets.iter().any(|(known, _)| known == at));
        near.append(&mut targets);
        targets = near;
    }
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
