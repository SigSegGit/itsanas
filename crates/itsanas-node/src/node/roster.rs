//! Which devices are this account's: the claim this device presents, the
//! siblings admitted on their account's signature, and withdrawals kept.

use super::Node;
use crate::error::Result;

impl Node {
    /// This device's claim, signed now with the account key and carrying
    /// this machine's current pledge, encoded for [`itsanas_net::PeerClient::present_claim`].
    ///
    /// Signed afresh at every connection rather than kept in a file: every
    /// node holds the account key (the claim is how a host learns whose this
    /// device is and what it offers, §8 1c), and a stored claim would carry a
    /// pledge the operator has since changed.
    #[must_use]
    pub fn claim_bytes(&self, now: u64) -> Vec<u8> {
        let claim = itsanas_coord::claim::NodeClaim {
            owner: self.store.owner(),
            device: self.store.device_id(),
            pledged_bytes: self.config.offered_bytes(),
            issued_unix: now,
            revoked: false,
        }
        .sign(&self.user);
        postcard::to_stdvec(&claim).unwrap_or_default()
    }

    /// Start a session the way every client must: present this device's
    /// claim, then admit the siblings the host knows to the roster, before
    /// any chain is applied. Failures cost the session nothing more than an
    /// older host would: a host that takes no claim stores nothing for this
    /// device, and a sibling whose claim does not arrive waits.
    ///
    /// # Errors
    ///
    /// If the roster cannot be written.
    pub fn introduce(&self, client: &mut itsanas_net::PeerClient, now: u64) -> Result<()> {
        let mut siblings = Vec::new();
        // The host's own claim, which names a sibling when the host is one.
        if let Ok(Some(theirs)) = client.present_claim(self.claim_bytes(now)) {
            siblings.push(theirs);
        }
        // The account's withdrawals this machine knows, passed on: a host of
        // another account learnt one from the coordinator alone, and took the
        // withdrawn device's old claim until then (HANDOVER §9).
        for withdrawal in self.withdrawals()? {
            let _ = client.present_claim(withdrawal);
        }
        siblings.extend(client.claims(self.store.owner()).unwrap_or_default());
        self.admit_roster(&siblings)?;
        Ok(())
    }

    /// Admit to this account's roster every device whose claim, among
    /// `claims` and the ones its siblings presented here, verifies against
    /// this account's key. Returns how many were for this account.
    ///
    /// A claim is believed for its signature alone, as a host believes it
    /// (`ClaimBook::take`): the account's key is what membership is, and a
    /// coordinator is not needed to say so. A revoked claim marks its device
    /// out for good.
    ///
    /// # Errors
    ///
    /// If the store or the vault cannot be read or written.
    pub fn admit_roster(&self, claims: &[Vec<u8>]) -> Result<usize> {
        let owner = self.store.owner();
        let kept = self.vault.claims_for(owner)?;
        let mut admitted = 0;
        for bytes in claims.iter().chain(kept.iter()) {
            let Ok(signed) = postcard::from_bytes::<itsanas_coord::claim::SignedClaim>(bytes)
            else {
                continue;
            };
            if signed.claim.owner != owner || signed.verify_origin().is_err() {
                continue;
            }
            self.store
                .admit_member(&signed.claim.device, signed.claim.revoked)?;
            admitted += 1;
        }
        Ok(admitted)
    }

    /// The withdrawals of this account kept here, as signed: the revoked
    /// claims in the vault, which [`Self::introduce`] passes on to hosts.
    ///
    /// # Errors
    ///
    /// If the vault cannot be read.
    pub fn withdrawals(&self) -> Result<Vec<Vec<u8>>> {
        let owner = self.store.owner();
        Ok(self
            .vault
            .claims_for(owner)?
            .into_iter()
            .filter(|bytes| {
                postcard::from_bytes::<itsanas_coord::claim::SignedClaim>(bytes).is_ok_and(
                    |signed| {
                        signed.claim.revoked
                            && signed.claim.owner == owner
                            && signed.verify_origin().is_ok()
                    },
                )
            })
            .collect())
    }

    /// Record here that `device` was withdrawn, on its revoked claim: out of
    /// the roster for good, and the claim kept in the vault, which is what
    /// this machine hands its siblings (`Request::Claims`). A withdrawal used
    /// to reach the coordinator alone, so a machine of the account that does
    /// not read it (no coordinator, or one that is down) went on admitting the
    /// withdrawn one (HANDOVER §9, "A withdrawn device keeps its old claim").
    ///
    /// # Errors
    ///
    /// If the store or the vault cannot be written.
    pub fn withdrawn_here(
        &self,
        device: itsanas_crypto::DeviceId,
        revoked_claim: &[u8],
    ) -> Result<()> {
        self.store.admit_member(&device, true)?;
        self.vault
            .keep_claim(device, self.store.owner(), revoked_claim)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};

    use super::*;
    use crate::node::tests::PASSPHRASE;

    #[test]
    fn red_team_an_open_node_admits_to_its_roster_only_what_its_account_signed() {
        // HANDOVER §10 7. A node arms its roster when it opens, so a chain
        // signed by a key its account never claimed is not applied; it admits
        // a sibling on its account's signature alone, and nothing a stranger
        // or another account signed.
        let dir = tempfile::tempdir().unwrap();
        let (node, _phrase) =
            Node::create(&dir.path().join("node"), PASSPHRASE, "nicolas").unwrap();
        let stranger = DeviceKeys::from_seed(&SecretBytes::new([0x51; 32])).device_id();
        let sibling = DeviceKeys::from_seed(&SecretBytes::new([0x52; 32])).device_id();
        assert!(
            node.store
                .may_apply_chain_of(&node.store.device_id())
                .unwrap()
        );
        assert!(
            !node.store.may_apply_chain_of(&stranger).unwrap(),
            "an open node applies chains from a key its account never claimed"
        );

        let claim = |device, owner, revoked, signer: &UserKeys| {
            postcard::to_stdvec(
                &itsanas_coord::claim::NodeClaim {
                    owner,
                    device,
                    pledged_bytes: 0,
                    issued_unix: 1,
                    revoked,
                }
                .sign(signer),
            )
            .unwrap()
        };
        let other = UserKeys::derive(&MasterSecret::from_bytes([0x77; 32]));
        let mine = node.store.owner();
        let forged = claim(stranger, mine, false, &other);
        let theirs = claim(stranger, other.user_id(), false, &other);
        let genuine = claim(sibling, mine, false, &node.user);
        assert_eq!(node.admit_roster(&[forged, theirs, genuine]).unwrap(), 1);
        assert!(node.store.may_apply_chain_of(&sibling).unwrap());
        assert!(
            !node.store.may_apply_chain_of(&stranger).unwrap(),
            "a claim another key signed, or for another account, admitted a stranger"
        );

        node.admit_roster(&[claim(sibling, mine, true, &node.user)])
            .unwrap();
        assert!(
            !node.store.may_apply_chain_of(&sibling).unwrap(),
            "a revocation did not stick"
        );
    }
}
