//! A short answer to "do we hold the same chunks?".
//!
//! # Why this exists
//!
//! A round used to ask its peer about every chunk it held, every time: thirty-two
//! bytes per chunk, per round, per peer — a two-thousandth of the account each
//! round, or a hundred and forty gigabytes a day for a terabyte. It bought an
//! exact answer to a question whose answer is almost always "nothing has
//! changed", and it paid the full price for that answer every five minutes.
//!
//! A summary costs the same whether the account is a megabyte or a terabyte:
//! one hash. If the two sides agree there is nothing more to say and the round
//! is over. If they disagree, the disagreement is *located* — the buckets whose
//! hashes differ — and only those are listed. The cost follows the difference
//! instead of the size, which is the property that makes this affordable at a
//! scale nobody has arbitrated yet.
//!
//! # Detection is separated from judgement
//!
//! A differing hash is a **question**, not a verdict. It says only "somewhere in
//! this eighth of the id space we do not agree", and what follows is the
//! ordinary have/missing exchange over that slice, which produces the same exact
//! answer it always did. Nothing is withdrawn, sanctioned or repaired on the
//! strength of a summary — an earlier design that condemned on an aggregate
//! hash would have destroyed sixty healthy holder records for every real one, on
//! nothing worse than a disk going soft.
//!
//! # The ordering is the contract
//!
//! Both sides hash their ids **in ascending order**, which they get for free:
//! the owner's index and the host's vault are both keyed by chunk id, so a range
//! scan is already sorted. Hashing in any other order would make two honest
//! machines disagree for ever, and the failure would look exactly like data
//! loss.

use itsanas_crypto::ChunkId;

/// How many buckets the chunk id space is split into.
///
/// Two hundred and fifty-six, so a bucket is the first byte of the id and the
/// arithmetic is a lookup rather than a division. A full summary is then 8 KiB,
/// which is only sent when the roots already disagree; the ordinary round sends
/// one hash and stops.
///
/// It also bounds the fallback: whatever the account's size, a disagreement
/// costs a have/missing exchange over a two-hundred-and-fifty-sixth of it.
pub const BUCKETS: usize = 256;

/// Domain string, so a summary can never be mistaken for another hash this
/// project computes over the same bytes.
const DOMAIN: &str = "itsanas v1 chunk set summary";

/// The digest of one bucket, and of the whole set.
pub type Digest = [u8; 32];

/// Summarise a set of chunk ids, **given in ascending order**.
///
/// Returns one digest per bucket. An empty bucket has a defined digest rather
/// than a special case, so two nodes that both hold nothing there agree without
/// anybody writing code about it.
#[must_use]
pub fn buckets(chunks: impl IntoIterator<Item = ChunkId>) -> Vec<Digest> {
    let mut hashers: Vec<blake3::Hasher> = (0..BUCKETS)
        .map(|bucket| {
            let mut hasher = blake3::Hasher::new();
            hasher.update(DOMAIN.as_bytes());
            hasher.update(&[u8::try_from(bucket).unwrap_or(0)]);
            hasher
        })
        .collect();

    for chunk in chunks {
        let bytes = chunk.as_bytes();
        let bucket = usize::from(bytes[0]);
        hashers[bucket].update(bytes.as_slice());
    }

    hashers
        .into_iter()
        .map(|hasher| *hasher.finalize().as_bytes())
        .collect()
}

/// One hash standing for the whole set.
///
/// What a round sends first. Thirty-two bytes, whatever the account weighs.
#[must_use]
pub fn root(buckets: &[Digest]) -> Digest {
    let mut hasher = blake3::Hasher::new();
    hasher.update(DOMAIN.as_bytes());
    hasher.update(b"root");
    for digest in buckets {
        hasher.update(digest);
    }
    *hasher.finalize().as_bytes()
}

/// Which buckets two sides disagree about.
///
/// A length mismatch means the two are not speaking the same dialect, and every
/// bucket is reported rather than silently comparing a prefix — a summary that
/// quietly compared the first half would report agreement about a half it never
/// looked at.
#[must_use]
pub fn differing(ours: &[Digest], theirs: &[Digest]) -> Vec<u8> {
    if ours.len() != theirs.len() {
        return (0..=u8::MAX).collect();
    }

    ours.iter()
        .zip(theirs)
        .enumerate()
        .filter(|(_, (ours, theirs))| ours != theirs)
        .filter_map(|(bucket, _)| u8::try_from(bucket).ok())
        .collect()
}

/// Whether a chunk falls in `bucket`.
#[must_use]
pub fn in_bucket(chunk: &ChunkId, bucket: u8) -> bool {
    chunk.as_bytes()[0] == bucket
}

#[cfg(test)]
mod tests {
    use super::{BUCKETS, buckets, differing, in_bucket, root};
    use itsanas_crypto::ChunkId;

    fn chunk(first: u8, rest: u8) -> ChunkId {
        let mut bytes = [rest; 32];
        bytes[0] = first;
        ChunkId::from_bytes(bytes)
    }

    #[test]
    fn two_machines_holding_the_same_set_agree_in_one_hash() {
        // The whole point, and the case that happens on almost every round of
        // almost every day: nothing changed, and saying so costs thirty-two
        // bytes rather than a two-thousandth of the account.
        let set: Vec<_> = (0..50).map(|index| chunk(index, index)).collect();
        assert_eq!(root(&buckets(set.clone())), root(&buckets(set)));
    }

    #[test]
    fn one_chunk_missing_is_located_rather_than_merely_noticed() {
        // A differing hash is a question, not a verdict. It has to say *where*,
        // or the only possible response is to list everything, which is the
        // cost this exists to avoid.
        let mine: Vec<_> = (0..50).map(|index| chunk(index, 7)).collect();
        let mut theirs = mine.clone();
        theirs.retain(|c| !in_bucket(c, 33));

        let (ours, theirs) = (buckets(mine), buckets(theirs));
        assert_ne!(root(&ours), root(&theirs));
        assert_eq!(differing(&ours, &theirs), vec![33]);
    }

    #[test]
    fn an_empty_set_has_a_defined_answer_on_both_sides() {
        // Two nodes that both hold nothing must agree without a special case,
        // and a node that holds nothing must not accidentally agree with one
        // that holds something.
        let empty = buckets(Vec::new());
        assert_eq!(empty.len(), BUCKETS);
        assert_eq!(root(&empty), root(&buckets(Vec::new())));
        assert_ne!(root(&empty), root(&buckets(vec![chunk(0, 0)])));
        assert!(differing(&empty, &empty).is_empty());
    }

    #[test]
    fn a_summary_of_a_different_length_is_all_disagreement() {
        // Comparing the overlap would report agreement about a part nobody
        // looked at, which is the one answer a reconciliation must never give.
        let ours = buckets(vec![chunk(1, 1)]);
        assert_eq!(differing(&ours, &ours[..10]).len(), 256);
    }

    #[test]
    fn order_within_a_bucket_is_part_of_the_contract() {
        // Both sides scan a table keyed by chunk id, so both are sorted. If one
        // ever were not, two honest machines would disagree for ever and it
        // would look exactly like data loss — so the requirement is written
        // down and demonstrated rather than assumed.
        let sorted = vec![chunk(5, 1), chunk(5, 2), chunk(5, 3)];
        let shuffled = vec![chunk(5, 3), chunk(5, 1), chunk(5, 2)];
        assert_ne!(root(&buckets(sorted)), root(&buckets(shuffled)));
    }
}
