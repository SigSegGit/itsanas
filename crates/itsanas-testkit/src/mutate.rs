//! Damaged copies of a message, for decoders that read bytes from strangers.
//!
//! Every peer and coordinator message reaches a decoder before anything
//! checks who sent it, so a decoder that panics, hangs or allocates without
//! bound on hostile bytes is a remote crash. A fuzzer finds that by trying
//! damaged inputs; [`mutations`] is the deterministic part of one: the same
//! seeds give the same damage on every run and platform, so it can stand in
//! the ordinary test suite, where a nightly fuzzer cannot.

/// `count` damaged copies of `message`, from `seed`.
///
/// Each copy has one to four of: a byte changed, a byte inserted, a byte
/// removed, the end cut off, a run of bytes repeated, or a run set to `0xFF`
/// (in postcard, the start of the largest length a varint can say).
#[must_use]
pub fn mutations(message: &[u8], seed: u64, count: usize) -> Vec<Vec<u8>> {
    let mut state = seed | 1;
    let mut next = move |bound: usize| -> usize {
        // xorshift64*: enough to spread the damage, and no crate for it.
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let drawn = state.wrapping_mul(0x2545_f491_4f6c_dd1d);
        usize::try_from(drawn % u64::try_from(bound.max(1)).unwrap_or(1)).unwrap_or(0)
    };
    (0..count)
        .map(|_| {
            let mut copy = message.to_vec();
            for _ in 0..=next(4) {
                damage(&mut copy, &mut next);
            }
            copy
        })
        .collect()
}

/// Apply one kind of damage to `bytes`.
fn damage(bytes: &mut Vec<u8>, next: &mut impl FnMut(usize) -> usize) {
    let at = next(bytes.len() + 1);
    match next(6) {
        0 if at < bytes.len() => {
            bytes[at] = u8::try_from(next(256)).unwrap_or(0);
        }
        1 => bytes.insert(at, u8::try_from(next(256)).unwrap_or(0)),
        2 if at < bytes.len() => {
            bytes.remove(at);
        }
        3 => bytes.truncate(at),
        4 if at < bytes.len() => {
            let end = (at + 1 + next(16)).min(bytes.len());
            let run = bytes[at..end].to_vec();
            for _ in 0..next(4) {
                bytes.splice(at..at, run.iter().copied());
            }
        }
        _ => {
            let end = (at + 1 + next(10)).min(bytes.len());
            for byte in bytes.iter_mut().take(end).skip(at) {
                *byte = 0xFF;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_damages_the_same_way_and_a_message_is_damaged() {
        let message = b"a message of a few dozen bytes, to be damaged".to_vec();
        let first = mutations(&message, 7, 200);
        assert_eq!(first, mutations(&message, 7, 200), "a seed gave two runs");
        assert!(
            first.iter().filter(|copy| **copy != message).count() > 150,
            "most copies came out undamaged: the fuzz would test the honest message"
        );
        assert!(
            first.iter().any(|copy| copy.len() < message.len())
                && first.iter().any(|copy| copy.len() > message.len()),
            "copies are never shorter or never longer: one kind of damage is missing"
        );
    }
}
