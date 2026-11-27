//! Peers: what auditing showed of each device, who is a member of this
//! account, and whether the vault holds work not yet applied here.

use itsanas_crypto::{ChunkId, DeviceId};

use super::Store;
use crate::{error::Result, index::now_unix, reliability::Reliability, vault::Vault};

impl Store {
    // ---------------------------------------------------------- reliability

    /// What auditing has shown about `device`.
    pub fn reliability(&self, device: &DeviceId) -> Result<Reliability> {
        self.index.reliability(device)
    }

    /// Record one audit outcome, and return the updated record.
    /// Record that `device` answered something, whatever it was.
    ///
    /// Liveness is what decides whether that device's acknowledgements still
    /// count as copies, and it has to be refreshed by *any* contact -- a peer
    /// that syncs every round but happens not to be audited this round is
    /// plainly still there.
    ///
    /// # Errors
    ///
    /// If the index cannot be written.
    pub fn note_seen(&self, device: &DeviceId) -> Result<()> {
        self.note_seen_at(device, now_unix())
    }

    /// Record contact at a stated time.
    ///
    /// The clock is an argument for the same reason it is one on
    /// [`Store::coverage`] and [`Store::release`]: the interesting states are
    /// the ones a real clock cannot reach inside a test. The one this exists
    /// for is a machine that answers the phone while its disk is empty --
    /// contact recent, records about individual chunks months old -- which is
    /// precisely the case a per-machine liveness rule cannot see and the case
    /// that costs data.
    ///
    /// # Errors
    ///
    /// If the index cannot be written.
    pub fn note_seen_at(&self, device: &DeviceId, now: u64) -> Result<()> {
        self.index.note_seen(device, now)
    }

    /// When this node last had any contact with `device`, if ever.
    ///
    /// What separates a holder record that still means something from one that
    /// is only a memory. See [`CONFIRMED_FOR`](crate::holders::CONFIRMED_FOR).
    pub fn last_seen(&self, device: &DeviceId) -> Result<Option<u64>> {
        self.index.last_seen(device)
    }

    pub fn note_audit(&self, device: &DeviceId, passed: bool) -> Result<Reliability> {
        self.index.note_audit(device, passed, now_unix())
    }

    /// The live members of this account's roster. See [`Index::members`].
    pub fn members(&self) -> Result<Vec<DeviceId>> {
        self.index.members()
    }

    /// The members of the account this node has not heard from within
    /// [`LIVE_FOR`](crate::holders::LIVE_FOR) at `now`, with when it last
    /// did, the most silent first (never heard before any date).
    ///
    /// Resting, not lost: in a network of machines that are usually off, a
    /// key in a drawer and a laptop on holiday are this. Named so a person
    /// knows which machines the copy count leaves out, and why.
    pub fn dormant_members(&self, now: u64) -> Result<Vec<(DeviceId, Option<u64>)>> {
        let me = self.device_id();
        let mut dormant = Vec::new();
        for member in self.index.members()? {
            if member == me {
                continue;
            }
            let seen = self.index.last_seen(&member)?;
            if seen.is_none_or(|at| now.saturating_sub(at) > crate::holders::LIVE_FOR) {
                dormant.push((member, seen));
            }
        }
        dormant.sort_by_key(|(device, seen)| (seen.unwrap_or(0), *device));
        Ok(dormant)
    }

    /// The chunks among `chunks` that this account's files use: what a
    /// holder record may name. A device id is free to mint, and a record
    /// for a chunk nobody here uses is a row for ever.
    pub fn referenced_among(&self, chunks: &[ChunkId]) -> Result<Vec<ChunkId>> {
        let mut out = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            if self.index.reference_count(chunk)? > 0 {
                out.push(*chunk);
            }
        }
        Ok(out)
    }

    /// Whether `device` is a live member of this account's roster: admitted
    /// on a claim signed with the account's key, and not revoked. The one
    /// answer to "is this one of ours", whatever the placement mode and
    /// however long the coordinator has not seen it.
    pub fn is_member(&self, device: &DeviceId) -> Result<bool> {
        self.index.is_member(device)
    }

    /// Whether `device` was withdrawn from this account, for good.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn is_revoked(&self, device: &DeviceId) -> Result<bool> {
        self.index.is_revoked(device)
    }

    /// Whether `device` should still be offered new file content.
    ///
    /// A peer that keeps failing audits is not blocked and loses nothing of its
    /// own; it simply stops costing this node an upload per round for data it
    /// throws away. Log segments still go to it, and so does one chunk a round
    /// to answer for; each answered round pays off one failure.
    pub fn worth_sending_to(&self, device: &DeviceId) -> Result<bool> {
        // One of this account's own is never paused; one paused before it
        // was known as such is not held to it.
        Ok(self.index.is_member(device)? || self.index.reliability(device)?.worth_sending_to())
    }

    /// Peers with a failure on their record, worst first.
    pub fn unreliable_devices(&self) -> Result<Vec<(DeviceId, Reliability)>> {
        self.index.unreliable_devices()
    }

    // ---------------------------------------------------- outstanding work

    /// Whether the vault holds work this device has not managed to apply.
    ///
    /// Two lookups per device, not a walk. The answer is *conservative*: a
    /// device whose head has moved is reported as outstanding even if the new
    /// segments would apply cleanly, because finding out costs exactly the walk
    /// this is here to avoid. Being wrong in that direction means one extra
    /// replay; being wrong in the other means a file that never arrives.
    pub fn has_unapplied(&self, vault: &Vault) -> Result<bool> {
        let mine = self.device_id();
        for (device, head, _) in vault.heads_for(self.owner())? {
            if device == mine {
                continue;
            }
            if self.index.applied_head(&device)? != Some(head) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Record that everything the vault currently holds has been applied.
    ///
    /// Call only after a round that deferred nothing. A round with even one
    /// deferral must leave the markers alone, or the work it could not finish
    /// is never looked at again — which is the bug this whole mechanism exists
    /// to make impossible.
    pub fn note_all_applied(&self, vault: &Vault) -> Result<()> {
        let mine = self.device_id();
        for (device, head, _) in vault.heads_for(self.owner())? {
            if device == mine {
                continue;
            }
            self.index.set_applied_head(&device, &head)?;
        }
        Ok(())
    }
}
