//! Files freed on this machine: still the account's, held by its other
//! machines, and left off this disk until a person keeps them here again.
//!
//! A small text file beside the store, like the trash ([`crate::trash`]) and for
//! the same reason: it is read while a round holds the database. Losing it costs
//! a download, never a file: a freed path it no longer names is written back.

use std::collections::BTreeSet;
use std::path::Path;

/// One freed path per line.
const RECORD: &str = "freed";

/// The paths freed on this machine.
#[must_use]
pub fn list(dir: &Path) -> BTreeSet<String> {
    let text = std::fs::read(dir.join(RECORD)).unwrap_or_default();
    String::from_utf8_lossy(&text)
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Record `path` as freed here.
///
/// # Errors
///
/// When the path holds a line break, or the record cannot be written.
pub fn add(dir: &Path, path: &str) -> std::io::Result<()> {
    if path.contains(['\n', '\r']) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a path with a line break cannot be freed",
        ));
    }
    let mut freed = list(dir);
    freed.insert(path.to_owned());
    save(dir, &freed)
}

/// `path` is kept here again.
///
/// # Errors
///
/// When the record cannot be written.
pub fn take_out(dir: &Path, path: &str) -> std::io::Result<()> {
    let mut freed = list(dir);
    if freed.remove(path) {
        save(dir, &freed)
    } else {
        Ok(())
    }
}

fn save(dir: &Path, freed: &BTreeSet<String>) -> std::io::Result<()> {
    let mut text = String::new();
    for path in freed {
        text.push_str(path);
        text.push('\n');
    }
    crate::trash::write_all(dir, RECORD, &text)
}
