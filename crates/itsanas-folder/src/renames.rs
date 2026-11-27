//! A deletion and an addition of the same bytes, told apart from two unrelated
//! changes.

use std::collections::BTreeMap;

/// The content each touched path had, kept for one pass so that a deletion
/// and an addition of the same bytes can be told apart from two unrelated
/// changes.
#[derive(Default)]
pub(crate) struct Contents {
    /// Removed from the store because the user deleted it here: the bytes the
    /// ledger says this device last had.
    pub(crate) gone_here: Vec<(String, [u8; 32])>,
    /// Imported from disk.
    pub(crate) came_here: Vec<(String, [u8; 32])>,
    /// Removed from disk because another device deleted it.
    pub(crate) gone_elsewhere: Vec<(String, [u8; 32])>,
    /// Written out because another device added it.
    pub(crate) came_elsewhere: Vec<(String, [u8; 32])>,
}

impl Contents {
    /// Whether anything went and anything came on the same side.
    pub(crate) fn could_pair(&self) -> bool {
        (!self.gone_here.is_empty() && !self.came_here.is_empty())
            || (!self.gone_elsewhere.is_empty() && !self.came_elsewhere.is_empty())
    }
}

/// Whether `hash` is the blake3 hash of no bytes at all.
///
/// Every empty file has it, so an empty file deleted and an unrelated empty
/// file created would otherwise read as a rename.
fn is_empty_content(hash: &[u8; 32]) -> bool {
    hash == blake3::hash(b"").as_bytes()
}

/// Pair each path that went with the path that came carrying the same bytes.
///
/// Only an unambiguous pair counts: exactly one path gone and exactly one
/// path come with those bytes, and not empty. Two identical copies deleted
/// and one added is a deletion and something else, and saying which copy
/// "became" the new one would be a guess printed as a fact. A file renamed
/// *and* edited in one pass has different bytes and stays a deletion and an
/// addition: nothing here reads names or similarity, only content.
///
/// And the new path must be the only live one with those bytes. If another
/// file still holds them, deleting one copy and making another is not a move
/// of anything -- `mv a -> b` would hide that `a`, a distinct file, was
/// deleted. `live` counts the live paths per content after the pass.
pub(crate) fn pair_renames(
    gone: &[(String, [u8; 32])],
    came: &[(String, [u8; 32])],
    live: &BTreeMap<[u8; 32], usize>,
) -> Vec<(String, String)> {
    let mut gone_by_content: BTreeMap<[u8; 32], Vec<&str>> = BTreeMap::new();
    for (path, hash) in gone {
        gone_by_content.entry(*hash).or_default().push(path);
    }
    let mut came_by_content: BTreeMap<[u8; 32], Vec<&str>> = BTreeMap::new();
    for (path, hash) in came {
        came_by_content.entry(*hash).or_default().push(path);
    }

    let mut pairs = Vec::new();
    for (hash, from) in &gone_by_content {
        if is_empty_content(hash) || live.get(hash).copied().unwrap_or(0) > 1 {
            continue;
        }
        if let ([from], Some([to])) = (
            from.as_slice(),
            came_by_content.get(hash).map(Vec::as_slice),
        ) {
            pairs.push(((*from).to_owned(), (*to).to_owned()));
        }
    }
    pairs.sort();
    pairs
}
