//! What happens when two devices edited the same file while apart.
//!
//! ITSaNAS never resolves a conflict by picking a winner and discarding the
//! loser. Two people who both worked on a document over a weekend have both
//! done real work, and no rule — not last-write-wins, not largest-file-wins,
//! and certainly not latest-timestamp-wins — can know which of them matters.
//! So both survive: one keeps the original path, the other appears beside it.
//!
//! # The part that is not arbitrary
//!
//! Which version keeps the original path *must* be decided identically on every
//! device, from information every device has. Otherwise the laptop puts version
//! A at `report.pdf` and the Pi puts version B there, and the two never
//! converge — they just keep overwriting each other forever.
//!
//! The rule is a total order on `(device_id, sequence)`: highest wins the
//! original path. Device ids are 32 random bytes, so the order is arbitrary but
//! stable, and every device computes the same answer without talking to anyone.
//!
//! # Why the sibling name has no timestamp in it
//!
//! `docs/DESIGN.md` originally specified `report.conflict-<device>-<timestamp>`.
//! A timestamp is the one thing that cannot appear here: the sibling path has to
//! be derived identically on every device, and the devices disagree about the
//! time. The sequence number is what replaced it — it is already unique per
//! device and every device sees the same value.

use itsanas_crypto::DeviceId;

/// Marker inserted into a conflicted file's name.
pub const CONFLICT_MARKER: &str = "conflict";

/// Which of two concurrent versions keeps the original path.
///
/// A total order over `(device, sequence)`, so every device agrees without
/// coordination. Returns `true` when the left side wins.
#[must_use]
pub fn wins_original_path(left: (DeviceId, u64), right: (DeviceId, u64)) -> bool {
    let (left_device, left_sequence) = left;
    let (right_device, right_sequence) = right;

    // Device first, so the answer does not depend on how many writes each
    // device happens to have made. Sequence only breaks a tie within one
    // device, which cannot actually be concurrent — but the ordering must still
    // be total for the rule to be well defined.
    match left_device.as_bytes().cmp(right_device.as_bytes()) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => left_sequence > right_sequence,
    }
}

/// The path a losing version is materialised at.
///
/// The marker goes before the extension so the file still opens in the right
/// application — a conflicted `report.pdf` becomes
/// `report.conflict-4f21c8d0a1b2-7.pdf`, not `report.pdf.conflict`, which
/// Windows would refuse to open at all.
#[must_use]
pub fn sibling_path(path: &str, device: DeviceId, sequence: u64) -> String {
    let suffix = format!("{CONFLICT_MARKER}-{}-{sequence}", device.short());

    // Split on the last dot of the final component only, so a directory called
    // `my.files` does not swallow the marker.
    let (directory, name) = match path.rfind('/') {
        Some(index) => (&path[..=index], &path[index + 1..]),
        None => ("", path),
    };

    // A leading dot is part of the name (`.bashrc`), not an extension marker.
    match name[1..].rfind('.') {
        Some(index) => {
            let split = index + 1;
            format!("{directory}{}.{suffix}{}", &name[..split], &name[split..])
        }
        None => format!("{directory}{name}.{suffix}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(byte: u8) -> DeviceId {
        DeviceId::from_bytes([byte; 32])
    }

    #[test]
    fn the_winner_is_the_same_whichever_side_asks() {
        // If this were not symmetric the two devices would each believe they
        // won, both write to the original path, and never converge.
        let a = (device(1), 5);
        let b = (device(2), 3);

        assert!(
            wins_original_path(a, b) != wins_original_path(b, a),
            "both devices reached the same verdict about who wins, so they \
             would fight over the original path forever"
        );
    }

    #[test]
    fn the_higher_device_id_wins_regardless_of_write_count() {
        // A device that has written a thousand times must not thereby win
        // against a device that has written twice — that would make the outcome
        // depend on unrelated activity elsewhere.
        assert!(wins_original_path((device(9), 1), (device(2), 9999)));
        assert!(!wins_original_path((device(2), 9999), (device(9), 1)));
    }

    #[test]
    fn the_order_is_total_so_no_pair_is_ever_undecided() {
        let candidates = [
            (device(1), 1),
            (device(1), 2),
            (device(2), 1),
            (device(200), 7),
        ];

        for (index, left) in candidates.iter().enumerate() {
            assert!(
                !wins_original_path(*left, *left),
                "a version beat itself, so the order is not strict"
            );
            for right in candidates.iter().skip(index + 1) {
                assert!(
                    wins_original_path(*left, *right) != wins_original_path(*right, *left),
                    "the order is not antisymmetric for {left:?} and {right:?}"
                );
            }
        }
    }

    #[test]
    fn the_marker_goes_before_the_extension() {
        // `report.pdf.conflict` would not open in a PDF reader, and Windows
        // would associate it with nothing at all.
        assert_eq!(
            sibling_path("report.pdf", device(0x4f), 7),
            "report.conflict-4f4f4f4f4f4f-7.pdf"
        );
    }

    #[test]
    fn a_file_with_no_extension_gets_the_marker_appended() {
        assert_eq!(
            sibling_path("README", device(0xab), 3),
            "README.conflict-abababababab-3"
        );
    }

    #[test]
    fn a_dotfile_keeps_its_leading_dot() {
        // Treating the leading dot as an extension separator would turn
        // `.bashrc` into `.conflict-....bashrc`, which is a different file and
        // no longer hidden in the way the user intended.
        assert_eq!(
            sibling_path(".bashrc", device(0x01), 2),
            ".bashrc.conflict-010101010101-2"
        );
    }

    #[test]
    fn only_the_final_component_is_examined_for_an_extension() {
        // A directory containing a dot must not swallow the marker.
        assert_eq!(
            sibling_path("my.files/report.pdf", device(0x01), 4),
            "my.files/report.conflict-010101010101-4.pdf"
        );
        assert_eq!(
            sibling_path("my.files/README", device(0x01), 4),
            "my.files/README.conflict-010101010101-4"
        );
    }

    #[test]
    fn a_multi_dot_name_splits_on_the_last_dot() {
        assert_eq!(
            sibling_path("archive.tar.gz", device(0x01), 1),
            "archive.tar.conflict-010101010101-1.gz"
        );
    }

    #[test]
    fn two_different_devices_produce_two_different_siblings() {
        // Three-way conflicts are rare but real, and two losers colliding on
        // one sibling path would destroy one of them.
        let from_one = sibling_path("report.pdf", device(1), 5);
        let from_two = sibling_path("report.pdf", device(2), 5);
        assert_ne!(from_one, from_two);
    }

    #[test]
    fn the_sibling_path_is_a_valid_logical_path() {
        // It is about to be written to a real store, which validates paths.
        for original in ["report.pdf", "a/b/c.txt", ".bashrc", "archive.tar.gz"] {
            let sibling = sibling_path(original, device(0x7f), 12);
            assert!(
                itsanas_store::path::validate(&sibling).is_ok(),
                "{sibling:?} would be rejected by the store"
            );
        }
    }
}
