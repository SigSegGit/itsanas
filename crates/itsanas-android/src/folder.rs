//! The phone's chosen folder: which of its files the application should copy,
//! and a batch of copies put into the account.

use itsanas_cli::phone::{trash_failed, write_file};
use itsanas_cli::phone_folder::{self, FolderFile};
use itsanas_cli::shell::{Answer, Failure};
use itsanas_cli::trash;
use itsanas_node::Node;

/// One file of a listing, as the application sends it.
fn folder_file(value: &serde_json::Value) -> Result<FolderFile, Failure> {
    let number = |key: &str| value[key].as_u64();
    match (value["path"].as_str(), number("size"), number("modified")) {
        (Some(path), Some(size), Some(modified)) => Ok(FolderFile {
            path: path.to_owned(),
            size,
            modified,
        }),
        _ => Err(Failure::Usage(format!("not a file in a folder: {value}"))),
    }
}

fn folder_json(text: &str) -> Result<serde_json::Value, Failure> {
    serde_json::from_str(text)
        .map_err(|error| Failure::Usage(format!("not a folder listing: {error}")))
}

/// [`Java_fr_ngas_itsanas_Native_folderWanted`] without the JNI.
pub(super) fn folder_wanted(node: &Node, listing: &str) -> Answer {
    let listed = folder_json(listing)?
        .as_array()
        .ok_or_else(|| Failure::Usage("a folder listing is a list".to_owned()))?
        .iter()
        .map(folder_file)
        .collect::<Result<Vec<_>, _>>()?;
    trash::note_listing(&node.home, &listed).map_err(|error| trash_failed(&error))?;
    let wanted: Vec<_> = phone_folder::wanted(&node.home, listed)
        .into_iter()
        .map(|file| {
            serde_json::json!({
                "path": file.path,
                "size": file.size,
                "modified": file.modified,
            })
        })
        .collect();
    Ok(serde_json::json!({ "wanted": wanted }).to_string())
}

/// [`Java_fr_ngas_itsanas_Native_folderImport`] without the JNI: a list of
/// `{"file": {...}, "source": "..."}`.
///
/// One segment for the whole batch, not one per photo (issue 93): a first run
/// of thousands of files was thousands of signed segments, each one a commit on
/// the phone and a log entry every machine of the account fetches. A file is
/// recorded only once the store has it and the segment is sealed: a file
/// refused for room is asked for again next round. The others of the batch go
/// in all the same, and the first refusal is the answer, so a person sees it.
pub(super) fn folder_import(node: &Node, batch: &str) -> Answer {
    let rows = folder_json(batch)?;
    let rows = rows
        .as_array()
        .ok_or_else(|| Failure::Usage("a batch is a list".to_owned()))?;
    let mut taken = Vec::with_capacity(rows.len());
    let mut refused = Vec::new();
    for row in rows {
        let file = folder_file(&row["file"])?;
        let source = row["source"]
            .as_str()
            .ok_or_else(|| Failure::Usage(format!("no source for {}", file.path)))?;
        match write_file(node, &file.in_account(), source) {
            Ok(_) => taken.push(file),
            Err(error) => refused.push(format!("{}: {}", file.path, error.message())),
        }
    }
    node.store.flush_segment()?;
    for file in &taken {
        phone_folder::imported(&node.home, file)
            .map_err(|error| Failure::Usage(format!("could not record {}: {error}", file.path)))?;
    }
    match refused.first() {
        None => Ok(serde_json::json!({ "imported": taken.len() }).to_string()),
        Some(first) => Err(Failure::Usage(format!(
            "{} of {} files did not reach the account; the first: {first}",
            refused.len(),
            rows.len(),
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole route of a photo. Sabotage: record before the store has it.
    #[test]
    fn red_team_a_folder_file_reaches_the_account_and_only_then_counts_as_seen() {
        let dir = tempfile::tempdir().unwrap();
        let (node, _) = Node::create(&dir.path().join("me"), "a passphrase", "camille").unwrap();
        let listing = r#"[{"path":"Camera/a.jpg","size":5,"modified":100}]"#;
        let one = |source: &std::path::Path| {
            serde_json::json!([{
                "file": {"path": "Camera/a.jpg", "size": 5, "modified": 100},
                "source": source,
            }])
            .to_string()
        };
        let wanted = |node: &Node| -> serde_json::Value {
            serde_json::from_str(&folder_wanted(node, listing).unwrap()).unwrap()
        };
        assert_eq!(wanted(&node)["wanted"].as_array().unwrap().len(), 1);

        let missing = dir.path().join("gone.jpg");
        assert!(folder_import(&node, &one(&missing)).is_err());
        assert_eq!(
            wanted(&node)["wanted"].as_array().unwrap().len(),
            1,
            "a copy the store never took counts as seen: this photo never reaches the account"
        );

        let source = dir.path().join("a.jpg");
        std::fs::write(&source, b"photo").unwrap();
        folder_import(&node, &one(&source)).unwrap();
        assert_eq!(
            node.store.read_file("Phone/Camera/a.jpg").unwrap(),
            Some(b"photo".to_vec()),
            "the photo is not in the account where the phone said it would be"
        );
        assert!(
            wanted(&node)["wanted"].as_array().unwrap().is_empty(),
            "an imported photo was asked for again: every round copies the whole folder"
        );
    }

    /// Issue 93: a first run of thousands of photos was one signed segment per
    /// photo. Sabotage: seal the segment in `write_file`.
    #[test]
    fn red_team_a_batch_of_photos_is_one_segment_not_one_each() {
        let dir = tempfile::tempdir().unwrap();
        let (node, _) = Node::create(&dir.path().join("me"), "a passphrase", "camille").unwrap();
        let rows: Vec<_> = (0..5)
            .map(|index| {
                let source = dir.path().join(format!("{index}.jpg"));
                std::fs::write(&source, format!("photo {index}")).unwrap();
                serde_json::json!({
                    "file": {"path": format!("Camera/{index}.jpg"), "size": 7, "modified": 1},
                    "source": source,
                })
            })
            .collect();
        let before = node.store.chain_length().unwrap();
        folder_import(&node, &serde_json::Value::from(rows).to_string()).unwrap();
        assert_eq!(
            node.store.chain_length().unwrap() - before,
            1,
            "five photos made several segments: a first run of thousands is thousands of commits and log entries"
        );
        assert_eq!(
            node.store.read_file("Phone/Camera/4.jpg").unwrap(),
            Some(b"photo 4".to_vec()),
            "the last photo of the batch is not in the account"
        );
    }

    /// The application is the only caller, and still: a bad listing is an error,
    /// not an empty folder that reads as nothing to do. Sabotage: skip bad rows.
    #[test]
    fn red_team_a_folder_listing_that_makes_no_sense_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (node, _) = Node::create(&dir.path().join("me"), "a passphrase", "camille").unwrap();
        for listing in ["not json", "{}", r#"[{"path":"a","size":-1,"modified":1}]"#] {
            assert!(
                folder_wanted(&node, listing).is_err(),
                "{listing} read as a folder with nothing new: its photos would stop syncing \
                 and nothing would say so"
            );
        }
    }
}
