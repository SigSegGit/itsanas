//! What a sync, a restore or a listing refused or left out, in words.

use itsanas_crypto::DeviceId;

/// The line a round prints when files stayed elsewhere for want of room here.
///
/// Said apart from "deferred", which usually means a peer is asleep and fixes
/// itself: this one does not until the disk is freed or the pledge shrinks.
pub(crate) fn describe_no_room(pull: &itsanas_sync::SyncReport) -> Option<String> {
    (pull.no_room > 0).then(|| {
        format!(
            "{} file(s) left on the other machines: this disk has no room for them \
             beside what its pledge still owes other people (`itsanas space`)",
            pull.no_room
        )
    })
}

/// `, N deferred` for a round's summary line, naming the ones kept out for
/// room and any chain refused; empty when there is nothing to say.
pub(crate) fn deferred_note(pull: &itsanas_sync::SyncReport) -> String {
    let mut note = match (pull.deferred, describe_no_room(pull)) {
        (0, _) => String::new(),
        (deferred, None) => format!(", {deferred} deferred"),
        (deferred, Some(no_room)) => format!(", {deferred} deferred\n  {no_room}"),
    };
    if let Some(refused) = describe_refused_chains(pull, ChainSource::Peer) {
        note.push_str("\n  ");
        note.push_str(&refused);
    }
    note
}

/// Where a refused chain was met, said on its line.
///
/// The forgery came from whoever handed it over, and that is where the owner
/// goes looking: "from this peer" after a restore sent people hunting for a
/// misbehaving machine when the segment was on their own backup disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChainSource {
    /// Served by the peer a round pulled from.
    Peer,
    /// Pushed into this node's vault by somebody, and met at the drain.
    Vault,
    /// Read off a backup disk by `backup restore`.
    Backup,
}

impl ChainSource {
    const fn words(self) -> &'static str {
        match self {
            Self::Peer => "from this peer",
            Self::Vault => "pushed into this vault",
            Self::Backup => "on the backup",
        }
    }
}

/// ` (ab12cd, ef34gh, and N more)`: the first devices of a set, and a count of
/// the rest; empty when none is named.
///
/// Bounded, because a device key is free and a host can serve a forgery per
/// key it minted: the owner's line must not grow with them.
pub(crate) fn name_devices(named: &[DeviceId], total: usize) -> String {
    let mut devices: Vec<String> = named
        .iter()
        .take(itsanas_sync::REFUSED_DEVICES_NAMED)
        .map(DeviceId::short)
        .collect();
    let unnamed = total.saturating_sub(devices.len());
    if unnamed > 0 && !devices.is_empty() {
        devices.push(format!("and {unnamed} more"));
    }
    if devices.is_empty() {
        String::new()
    } else {
        format!(" ({})", devices.join(", "))
    }
}

/// The line printed when a device chain failed its checks. The other chains
/// were applied; this one was not, and quietly leaving it out would read as a
/// finished sync. Named, so the owner can tell a stranger's key from a
/// sibling of their own.
pub(crate) fn describe_refused_chains(
    pull: &itsanas_sync::SyncReport,
    source: ChainSource,
) -> Option<String> {
    (pull.refused_chains > 0).then(|| {
        format!(
            "{} device chain(s) {} refused{}: a segment failed its signature, chain or \
             seal check; those devices' changes wait for an honest copy",
            pull.refused_chains,
            source.words(),
            name_devices(&pull.refused_devices, pull.refused_chains)
        )
    })
}

/// The line `itsanas ls` prints when the listing left a device's chain out.
///
/// The walk leaves out another device's chain whose segment does not open, so
/// that one forgery in the vault cannot fail every listing for good. A
/// genuine sibling's segment damaged on this disk is left out the same way,
/// and its files and deletes are then missing from the list: said, or the
/// owner reads a short list as files gone.
pub(crate) fn describe_left_out(listing: &itsanas_store::catalogue::Catalogue) -> Option<String> {
    (!listing.left_out.is_empty()).then(|| {
        format!(
            "{} device chain(s) left out of this list{}: a segment does not open under this \
             account's key (a forgery, or a sibling's segment damaged on this disk), so \
             those devices' files and deletes are not shown",
            listing.left_out.len(),
            name_devices(&listing.left_out, listing.left_out.len())
        )
    })
}

/// The line a round prints when a peer refused what it was offered, if it did.
///
/// Without it a host refusing everything produced the line an idle round
/// produces, `sent 0 B`, and the owner believed nothing was pending.
pub(crate) fn describe_refusal(push: &itsanas_net::PushReport) -> Option<String> {
    let why = match push.refusal? {
        itsanas_net::Refusal::PledgeFull => {
            "its pledge is full or zero, so it hosts nothing more; on that machine, `itsanas pledge <size>`"
        }
        // Not "its log says why": a peer logs nothing about what it refuses,
        // and the first version of this line sent people to read a journal
        // that had nothing in it.
        itsanas_net::Refusal::AccountFull => {
            "this account has stored there what its pledge earns; pledge more, or let it evict spare copies"
        }
        itsanas_net::Refusal::Rejected => {
            "it rejected them: a segment that does not verify, or does not follow its chain"
        }
        itsanas_net::Refusal::Unconfirmed => {
            "it hosts another account only once its coordinator has confirmed this device; \
             both machines need `itsanas coordinator <host:port> --device <id>`, and the next \
             round tries again"
        }
        itsanas_net::Refusal::Withdrawn => {
            "its coordinator says this device was withdrawn from the account"
        }
        itsanas_net::Refusal::Contradicted => {
            "this account's devices failed its storage audits, so it stores only what they proved they hold"
        }
    };
    Some(format!("refused {} offer(s): {why}", push.refused))
}

#[cfg(test)]
mod tests {
    use itsanas_crypto::DeviceId;

    /// `itsanas sync` names a refused chain on its summary line, and the
    /// device it came from, and says nothing extra when there is none.
    /// Sabotage: drop the push of `describe_refused_chains` in `deferred_note`
    /// -- the round then reads as a finished sync with one device's changes
    /// silently left out; leave the device names off -- the owner cannot tell
    /// a forgery from a stale sibling of their own.
    #[test]
    fn a_refused_chain_is_said_on_the_sync_line() {
        let quiet = itsanas_sync::SyncReport::default();
        assert_eq!(super::deferred_note(&quiet), "");

        let device = DeviceId::from_bytes([0x3F; 32]);
        let refused = itsanas_sync::SyncReport {
            refused_chains: 1,
            refused_devices: vec![device],
            ..itsanas_sync::SyncReport::default()
        };
        let line = super::deferred_note(&refused);
        assert!(
            line.contains("1 device chain(s) from this peer refused"),
            "a refused chain left no trace on the sync line: {line:?}"
        );
        assert!(
            line.contains(&device.short()),
            "the sync line did not name the refused device: {line:?}"
        );

        // A peer serving many forgeries gets a bounded line, not one name
        // per key it minted.
        let named: Vec<DeviceId> = (1..=itsanas_sync::REFUSED_DEVICES_NAMED)
            .map(|seed| DeviceId::from_bytes([u8::try_from(seed).expect("small"); 32]))
            .collect();
        let many = itsanas_sync::SyncReport {
            refused_chains: named.len() + 2,
            refused_devices: named.clone(),
            ..itsanas_sync::SyncReport::default()
        };
        let line = super::deferred_note(&many);
        assert!(
            named.iter().all(|device| line.contains(&device.short()))
                && line.contains("and 2 more"),
            "the sync line did not name the first refused devices and count the rest: {line:?}"
        );
    }

    /// `backup restore` says the chains it refused, the same whoever runs it.
    ///
    /// With a daemon running, a restore is a disk job, and the job's line
    /// once said only the counts: a restore from a disk carrying a forged
    /// chain read as done, that device's changes left out without a word.
    /// And it says where the forgery was -- on the backup -- not "from this
    /// peer", which sent the owner hunting for a misbehaving machine.
    /// Sabotage: drop the refused line from `backup::restore_summary`; or
    /// word it with `ChainSource::Peer`.
    #[test]
    fn a_restore_says_the_chains_it_refused_on_the_backup() {
        let moved = crate::backup::Moved {
            segments: 4,
            chunks: 9,
            bytes: 1024,
            ..crate::backup::Moved::default()
        };
        let clean = itsanas_sync::SyncReport {
            adopted: 2,
            ..itsanas_sync::SyncReport::default()
        };
        assert_eq!(
            crate::backup::restore_summary(&moved, &clean, "E:\\backup"),
            "read 4 segments and 9 chunks (1.0 KiB) from E:\\backup; 2 file(s) adopted"
        );

        let device = DeviceId::from_bytes([0x4E; 32]);
        let forged = itsanas_sync::SyncReport {
            adopted: 2,
            refused_chains: 1,
            refused_devices: vec![device],
            ..itsanas_sync::SyncReport::default()
        };
        let said = crate::backup::restore_summary(&moved, &forged, "E:\\backup");
        assert!(
            said.contains("1 device chain(s) on the backup refused")
                && said.contains(&device.short()),
            "a restore that refused a forged chain read as done, or did not name it: {said:?}"
        );
        assert!(
            !said.contains("from this peer"),
            "a restore blamed a peer for a segment on the owner's own backup disk: {said:?}"
        );
    }

    /// `itsanas ls` says a device chain it left out, and names the device.
    ///
    /// The listing leaves out a chain whose segment does not open, so one
    /// forgery cannot fail every listing; a sibling's damaged segment is left
    /// out the same way, and a list that said nothing showed its deleted
    /// files back and its new ones gone. Sabotage: have `describe_left_out`
    /// answer `None`, or leave the device names off.
    #[test]
    fn a_listing_that_left_a_chain_out_says_so() {
        let whole = itsanas_store::catalogue::Catalogue {
            files: Vec::new(),
            complete: true,
            left_out: Vec::new(),
            behind: Vec::new(),
        };
        assert_eq!(super::describe_left_out(&whole), None);

        let device = DeviceId::from_bytes([0x6C; 32]);
        let short = itsanas_store::catalogue::Catalogue {
            files: Vec::new(),
            complete: false,
            left_out: vec![device],
            behind: Vec::new(),
        };
        let line = super::describe_left_out(&short).unwrap_or_default();
        assert!(
            line.contains("1 device chain(s) left out") && line.contains(&device.short()),
            "a listing that left a device's chain out said nothing, or did not name it: \
             {line:?}; the owner reads its files as gone"
        );
    }

    #[test]
    fn a_refusal_is_reported_once_and_then_only_after_a_quiet_period() {
        // A pledge-0 host refuses every round. The line that fixed a silent
        // round must not turn into one line every five minutes per peer.
        let now = std::time::Instant::now();
        assert!(
            crate::daemon::refusal_due(None, now),
            "the first refusal from a peer was not reported"
        );
        assert!(
            !crate::daemon::refusal_due(Some(now), now + std::time::Duration::from_secs(300)),
            "a refusal was repeated at the next round"
        );
        assert!(
            crate::daemon::refusal_due(Some(now), now + crate::daemon::OUTAGE_QUIET_FOR_TESTS),
            "a refusal that is still going on was never reported again"
        );
    }
}
