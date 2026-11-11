//! Files that need not stay on this machine (HANDOVER §8 11).
//!
//! Freeing a file of the synced folder takes its plain copy off the folder
//! ([`itsanas_folder::Folder::free`]); this machine's sealed copy goes too once
//! the account's other machines have proved they hold it ([`Store::release`]),
//! now or on a later round ([`release_all`]). Keeping it here again fetches it
//! back when nothing is left on this machine. One module for the command, the
//! page and the daemon, so the three cannot disagree on what freeing means.
//!
//! [`Store::release`]: itsanas_store::Store::release

use itsanas_node::Node;
use itsanas_store::{Release, catalogue::Presence, freed};
use serde_json::json;

use crate::{failure::Failure, format_size};

/// The most files the page lists: a long account is browsed with `itsanas ls`.
const MAX_LISTED: usize = 2_000;

fn usage(error: impl std::fmt::Display) -> Failure {
    Failure::Usage(error.to_string())
}

/// Free `path` here; says what left this machine.
///
/// # Errors
///
/// When no folder is synced here, or the folder refuses (the copy here is not
/// the account's version).
pub fn free(node: &Node, path: &str) -> Result<String, Failure> {
    let Some(root) = node.config.folder.clone() else {
        return Err(Failure::Usage(
            "no synced folder here: `itsanas folder <path>` sets one".to_owned(),
        ));
    };
    itsanas_folder::Folder::open(&root)
        .and_then(|folder| folder.free(&node.store, path))
        .map_err(usage)?;
    Ok(match node.store.release(path, itsanas_store::now_unix())? {
        Release::Gone(report) => format!(
            "{path} is freed here: it left the folder, and {} left this machine. It stays in the account.",
            format_size(report.bytes)
        ),
        Release::NotSafeYet { .. } => format!(
            "{path} left the folder. Its sealed copy stays on this machine until other machines prove they hold it; it goes then, by itself."
        ),
        Release::NotHere => format!("{path} left the folder. It stays in the account."),
    })
}

/// Keep `path` here again: the next pass writes it into the folder, fetched
/// from the account's machines first when this one let it go.
///
/// # Errors
///
/// When the record cannot be written, or the file cannot be fetched.
pub fn keep(node: &Node, path: &str) -> Result<String, Failure> {
    if !itsanas_folder::Folder::keep_here(&node.store, path).map_err(usage)? {
        return Ok(format!("{path} was not freed here."));
    }
    if node.store.stat(path)?.is_none() {
        crate::fetch_absent(node, path).map_err(usage)?;
    }
    Ok(format!(
        "{path} comes back into the folder on the next pass."
    ))
}

/// Let go of the sealed copies of freed files whose copies elsewhere are now
/// proved. Quiet when there is nothing to do; a file not yet safe waits.
pub fn release_all(node: &Node) {
    let now = itsanas_store::now_unix();
    for path in freed::list(node.store.root()) {
        if let Ok(Release::Gone(report)) = node.store.release(&path, now) {
            println!(
                "freed {path}: {} left this machine",
                format_size(report.bytes)
            );
        }
    }
}

/// The account's files for the page: whether each is here, and freed here.
///
/// # Errors
///
/// When the store cannot be read.
pub fn files(node: &Node) -> Result<String, Failure> {
    let catalogue = itsanas_store::catalogue::catalogue(&node.store, &node.vault)?;
    let freed = freed::list(node.store.root());
    let rows: Vec<_> = catalogue
        .files
        .iter()
        .take(MAX_LISTED)
        .map(|known| {
            json!({
                "path": known.path,
                "size": format_size(known.size),
                "here": known.presence == Presence::Local,
                "freed": freed.contains(&known.path),
            })
        })
        .collect();
    Ok(json!({
        "files": rows,
        "complete": catalogue.complete && catalogue.files.len() <= MAX_LISTED,
        "folder": node.config.folder.is_some(),
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With no other machine proving it holds the file, freeing it empties the
    /// folder and keeps the sealed copy: the account never rests on this disk
    /// alone and nowhere at once. Keeping it brings it back.
    #[test]
    fn a_file_freed_with_no_proved_copy_elsewhere_keeps_its_sealed_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut node, _) =
            Node::create(&dir.path().join("me"), "a passphrase", "camille").expect("node");
        let root = dir.path().join("folder");
        std::fs::create_dir_all(&root).expect("folder");
        std::fs::write(root.join("film.mov"), b"many bytes").expect("write");
        node.config.folder = Some(root.clone());
        let folder = itsanas_folder::Folder::open(&root).expect("open");
        folder.reconcile(&node.store, false).expect("import");

        let said = free(&node, "film.mov").expect("free");
        assert!(
            !root.join("film.mov").exists(),
            "the folder kept the freed file"
        );
        assert!(
            node.store.read_file("film.mov").expect("read").is_some(),
            "the only copy left this machine with nobody proved to hold another: {said}"
        );
        let listed = files(&node).expect("files");
        assert!(
            listed.contains("\"freed\":true"),
            "the page does not show it freed: {listed}"
        );

        keep(&node, "film.mov").expect("keep");
        folder.reconcile(&node.store, false).expect("export");
        assert_eq!(
            std::fs::read(root.join("film.mov"))
                .expect("back")
                .as_slice(),
            b"many bytes"
        );
    }
}
