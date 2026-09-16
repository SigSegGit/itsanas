//! Validation for the logical paths a store addresses files by.
//!
//! These strings arrive from two directions: the local filesystem, and — far
//! more importantly — a peer's operation log. A path from a peer is attacker
//! controlled in the case that matters, because the sync engine will eventually
//! turn it into a real file on a real disk. Getting this wrong means a peer can
//! write `../../../.ssh/authorized_keys`.
//!
//! The rules are deliberately strict rather than clever. Anything ambiguous is
//! rejected; there is no normalisation step that an attacker could aim at.

use crate::error::{Result, StoreError};

/// Longest logical path accepted, in bytes.
///
/// Well below the point where any target filesystem complains, and short enough
/// that a peer cannot make the index enormous with one entry.
pub const MAX_PATH_LEN: usize = 1024;

/// Longest single component.
pub const MAX_COMPONENT_LEN: usize = 255;

/// Device names Windows resolves specially regardless of directory or
/// extension. Creating `com1.txt` on the Raspberry Pi and syncing it to the
/// laptop must not open a serial port.
const WINDOWS_RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Check that `path` is a safe, canonical, relative logical path.
///
/// Accepted: `notes.txt`, `work/reports/q3.pdf`, `Ünïcode ok.txt`.
/// Rejected: absolute paths, `..`, backslashes, empty components, trailing
/// slashes, control characters, and Windows device names.
pub fn validate(path: &str) -> Result<()> {
    let reject = |reason: &'static str| Err(StoreError::InvalidPath(path.to_owned(), reason));

    if path.is_empty() {
        return reject("empty");
    }
    if path.len() > MAX_PATH_LEN {
        return reject("longer than MAX_PATH_LEN");
    }
    if path.contains('\\') {
        return reject("backslash; logical paths use '/' only");
    }
    if path.starts_with('/') {
        return reject("absolute");
    }
    if path.ends_with('/') {
        return reject("trailing slash");
    }
    if path.chars().any(char::is_control) {
        return reject("control character");
    }
    // `C:` and friends are absolute on Windows even without a leading slash.
    if path.as_bytes().get(1) == Some(&b':') {
        return reject("drive-letter prefix");
    }

    for component in path.split('/') {
        if component.is_empty() {
            return reject("empty component");
        }
        if component.len() > MAX_COMPONENT_LEN {
            return reject("component longer than MAX_COMPONENT_LEN");
        }
        if component == "." || component == ".." {
            return reject("relative component");
        }
        // Windows silently strips these, so `evil.txt ` and `evil.txt` would
        // collide on one device and not another.
        if component.ends_with(' ') || component.ends_with('.') {
            return reject("component ends with a space or dot");
        }

        let stem = component
            .split('.')
            .next()
            .unwrap_or(component)
            .to_ascii_lowercase();
        if WINDOWS_RESERVED.contains(&stem.as_str()) {
            return reject("Windows reserved device name");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn accepted(path: &str) {
        assert!(validate(path).is_ok(), "{path:?} should have been accepted");
    }

    /// Two spellings of one filename are two files, and that is not decided.
    ///
    /// # What this pins
    ///
    /// macOS hands back **decomposed** names from the filesystem (NFD: `e`
    /// followed by U+0301) where Linux and Windows use **composed** (NFC:
    /// U+00E9). Nothing in this workspace normalises a filename, so the same
    /// `Café.txt` is two different logical paths depending on which machine
    /// wrote it.
    ///
    /// Reproduced on Windows on 2026-09-16 without a Mac, by storing both
    /// forms directly: two `put`s produced two stored files, `scan` wrote both
    /// to NTFS, and the round printed `out  Unicode/Café.txt` **twice** with
    /// `0 conflicts` — identical to the eye, unrelated to the system. Nothing
    /// is lost; what the person gets is a duplicate they cannot tell apart,
    /// and a Mac joining the account would re-upload every accented file it
    /// received from Linux.
    ///
    /// This test does not say that is right. It says it is **current**, so
    /// that adding normalisation is a decision somebody takes on purpose —
    /// canonicalising to NFC on ingest while keeping the on-disk form per
    /// platform is where Syncthing landed — rather than a silent change that
    /// makes every existing accented path unreachable.
    #[test]
    fn the_two_unicode_spellings_of_one_name_are_two_paths_today() {
        let composed = "Caf\u{e9}.txt";
        let decomposed = "Cafe\u{301}.txt";

        assert_ne!(
            composed, decomposed,
            "the fixture is wrong: these must be different byte strings"
        );
        accepted(composed);
        accepted(decomposed);
        assert_ne!(
            composed.len(),
            decomposed.len(),
            "composed is 3 bytes for the accent, decomposed 3 for e + combining mark"
        );
    }

    #[track_caller]
    fn rejected(path: &str) {
        assert!(
            validate(path).is_err(),
            "{path:?} was accepted; a peer could use it to escape the sync root"
        );
    }

    #[test]
    fn ordinary_paths_are_accepted() {
        accepted("notes.txt");
        accepted("work/reports/q3.pdf");
        accepted("a/b/c/d/e/f/g.bin");
        accepted("Ünïcode filename.txt");
        accepted("file.with.many.dots.txt");
        accepted("-leading-dash.txt");
        accepted(".hidden");
    }

    #[test]
    fn traversal_is_rejected_in_every_position() {
        rejected("..");
        rejected("../secrets");
        rejected("a/../../b");
        rejected("a/..");
        rejected("./a");
        rejected("a/./b");
    }

    #[test]
    fn absolute_paths_are_rejected() {
        rejected("/etc/passwd");
        rejected("/");
        rejected("C:/Windows/System32/drivers/etc/hosts");
        rejected("c:relative");
    }

    #[test]
    fn backslashes_are_rejected_rather_than_translated() {
        // Translating would mean `a\b` and `a/b` name the same file on Windows
        // and different files on Linux — the two devices would diverge.
        rejected("a\\b");
        rejected("..\\..\\secrets");
        rejected("C:\\Windows");
    }

    #[test]
    fn windows_device_names_are_rejected() {
        // The Raspberry Pi will happily create these; the laptop must never try.
        rejected("con");
        rejected("CON");
        rejected("nul.txt");
        rejected("com1");
        rejected("LPT9.log");
        rejected("dir/aux/file.txt");

        // Not reserved, and must stay usable.
        accepted("console.txt");
        accepted("common.txt");
        accepted("com0");
        accepted("com10");
    }

    #[test]
    fn trailing_spaces_and_dots_are_rejected() {
        rejected("file.txt ");
        rejected("file.txt.");
        rejected("dir /file.txt");
        rejected("dir./file.txt");
    }

    #[test]
    fn malformed_separators_are_rejected() {
        rejected("");
        rejected("a//b");
        rejected("a/");
        rejected("//a");
    }

    #[test]
    fn control_characters_are_rejected() {
        rejected("a\0b");
        rejected("a\nb");
        rejected("a\rb");
        rejected("a\tb");
    }

    #[test]
    fn oversized_paths_and_components_are_rejected() {
        rejected(&"a".repeat(MAX_PATH_LEN + 1));
        rejected(&format!("dir/{}", "a".repeat(MAX_COMPONENT_LEN + 1)));
        accepted(&"a".repeat(MAX_COMPONENT_LEN));
    }
}
