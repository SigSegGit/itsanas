//! What a pass did, said: a summary for a log and one line per file.

use std::{collections::BTreeSet, fmt::Write as _};

/// What one reconciliation pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Files taken from disk into the store.
    pub imported: Vec<String>,
    /// Files written out of the store onto disk.
    pub exported: Vec<String>,
    /// Files the user deleted: in this machine's trash, or deleted in the store
    /// at once when the trash is off or the file was moved.
    pub removed_from_store: Vec<String>,
    /// Files a peer deleted, now removed from disk.
    pub deleted_from_disk: Vec<String>,
    /// Paths where both sides had changed differently. The local version was
    /// moved aside; the value is where it went.
    pub kept_both: Vec<(String, String)>,
    /// Paths whose bookkeeping was corrected without moving any data.
    pub recorded: usize,
    /// Paths that could not be handled, with why. One bad file must not stop
    /// the rest of the folder from syncing.
    pub failed: Vec<(String, String)>,
    /// Deletions this pass found and did **not** write, because there were
    /// enough of them to look like an accident rather than a decision.
    ///
    /// Nothing was lost: the files are still in the account, and the next pass
    /// will find them missing again. `itsanas folder --confirm` is what says
    /// they really are meant to go.
    pub held_deletions: usize,
    /// Files moved or renamed in this folder, as `(from, to)`.
    ///
    /// Recognised, not performed: the store still saw a deletion of `from`
    /// and an import of `to`, and both stay listed in `removed_from_store`
    /// and `imported` -- that is what replicates. This only says that the
    /// two are one gesture, so that a person reading the report sees
    /// "renamed" instead of a file lost and an unknown one appearing. Only
    /// an unambiguous pair counts: one path gone and one come with the same
    /// non-empty bytes, and no other live path holding them.
    pub renamed_here: Vec<(String, String)>,
    /// Files another device moved or renamed, as `(from, to)`, out of
    /// `deleted_from_disk` and `exported` in the same way.
    pub renamed_elsewhere: Vec<(String, String)>,
    /// Files that changed while they were being read: something was still
    /// writing them. Nothing of them was taken; the next pass reads them again.
    pub still_writing: Vec<String>,
}

/// How many file lines [`ReconcileReport::lines`] gives before it stops
/// naming files, when the caller asks for a bound.
///
/// The daemon and `itsanas sync` print the report into a log on every pass;
/// the first pass over an existing photo library would otherwise write ten
/// thousand lines into it. `itsanas scan` asks for every line.
pub const LINES_IN_A_LOG: usize = 20;

impl ReconcileReport {
    /// Whether anything actually moved.
    #[must_use]
    pub fn changed_anything(&self) -> bool {
        !self.imported.is_empty()
            || !self.exported.is_empty()
            || !self.removed_from_store.is_empty()
            || !self.deleted_from_disk.is_empty()
            || !self.kept_both.is_empty()
    }

    /// A one-line summary for a log.
    ///
    /// A recognised rename counts once, as a rename, and not also as one file
    /// in and one deleted: "1 in, 1 deleted locally" for a renamed file reads
    /// as a loss to anyone who does not know the two are the same bytes.
    #[must_use]
    pub fn summary(&self) -> String {
        let here = self.renamed_here.len();
        let elsewhere = self.renamed_elsewhere.len();
        let mut line = format!(
            "{} in, {} out, {} deleted locally, {} deleted remotely, {} conflicts",
            self.imported.len().saturating_sub(here),
            self.exported.len().saturating_sub(elsewhere),
            self.removed_from_store.len().saturating_sub(here),
            self.deleted_from_disk.len().saturating_sub(elsewhere),
            self.kept_both.len()
        );
        if here + elsewhere > 0 {
            let _ = write!(line, ", {} renamed", here + elsewhere);
        }
        // Said, or a file a writer is still filling waits silently, and its
        // owner sees "0 in" with no idea why it is not on the other machines.
        if !self.still_writing.is_empty() {
            let _ = write!(
                line,
                ", {} still being written (read again next pass)",
                self.still_writing.len()
            );
        }
        line
    }

    /// One line per file this pass touched, naming it and what happened to
    /// it, renames as one line rather than a deletion and an addition.
    ///
    /// `limit` bounds the number of routine lines (renames, imports,
    /// exports); the ones past it are counted in a last line rather than
    /// dropped silently. Deletions and conflicts come first and are never
    /// bounded. Failures are not included:
    /// they go to standard error, where a caller already sends them.
    #[must_use]
    pub fn lines(&self, limit: Option<usize>) -> Vec<String> {
        let moved_from_here: BTreeSet<&str> = self
            .renamed_here
            .iter()
            .map(|(from, _)| from.as_str())
            .collect();
        let moved_to_here: BTreeSet<&str> = self
            .renamed_here
            .iter()
            .map(|(_, to)| to.as_str())
            .collect();
        let moved_from_elsewhere: BTreeSet<&str> = self
            .renamed_elsewhere
            .iter()
            .map(|(from, _)| from.as_str())
            .collect();
        let moved_to_elsewhere: BTreeSet<&str> = self
            .renamed_elsewhere
            .iter()
            .map(|(_, to)| to.as_str())
            .collect();

        // Deletions and conflicts first, and never cut: a log that named
        // twenty imports and counted a deletion among "7 more" would hide the
        // one line somebody needs to read. The bound is for the routine ones.
        let mut urgent = Vec::new();
        for (original, sibling) in &self.kept_both {
            let (original, sibling) = (shown(original), shown(sibling));
            urgent.push(format!(
                "  !!   {original} conflicted -- your version kept as {sibling}"
            ));
        }
        for path in &self.removed_from_store {
            if !moved_from_here.contains(path.as_str()) {
                let path = shown(path);
                urgent.push(format!(
                    "  del  {path} (deleted here, everywhere once it leaves the trash)"
                ));
            }
        }
        for path in &self.deleted_from_disk {
            if !moved_from_elsewhere.contains(path.as_str()) {
                let path = shown(path);
                urgent.push(format!(
                    "  rm   {path} (deleted elsewhere, removed from this folder)"
                ));
            }
        }

        let mut routine = Vec::new();
        for (from, to) in &self.renamed_here {
            let (from, to) = (shown(from), shown(to));
            routine.push(format!(
                "  mv   {from} -> {to} (renamed here, renamed everywhere)"
            ));
        }
        for (from, to) in &self.renamed_elsewhere {
            let (from, to) = (shown(from), shown(to));
            routine.push(format!(
                "  mv   {from} -> {to} (renamed elsewhere, renamed here)"
            ));
        }
        for path in &self.imported {
            if !moved_to_here.contains(path.as_str()) {
                routine.push(format!("  in   {}", shown(path)));
            }
        }
        for path in &self.exported {
            if !moved_to_elsewhere.contains(path.as_str()) {
                routine.push(format!("  out  {}", shown(path)));
            }
        }

        if let Some(limit) = limit
            && routine.len() > limit
        {
            let more = routine.len() - limit;
            routine.truncate(limit);
            routine.push(format!(
                "  ... and {more} more (`itsanas scan` lists them all)"
            ));
        }
        urgent.extend(routine);
        urgent
    }

    /// Whether this pass refused to write deletions it found.
    #[must_use]
    pub const fn held_anything(&self) -> bool {
        self.held_deletions > 0
    }
}

/// A path as a report prints it: anything that would change how the line
/// reads, rather than what it says, escaped.
///
/// The store already refuses control characters in a path, so a newline cannot
/// start a forged line; they are escaped here too because this function is
/// what the report relies on, not a check two crates away. Bidirectional
/// overrides and isolates are not control characters to `char::is_control`
/// and the store accepts them, and printed raw they reorder the line:
/// `invoice<U+202E>fdp.exe` reads as `invoiceexe.pdf`. Every other
/// character, accents and scripts included, prints as it is.
fn shown(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}') {
            out.extend(c.escape_unicode());
        } else {
            out.push(c);
        }
    }
    out
}
