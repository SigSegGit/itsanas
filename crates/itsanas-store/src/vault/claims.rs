//! Claims: the device claims an account's machines presented here, kept so
//! a host can hand an account its own roster, the latest first.

use itsanas_crypto::{DeviceId, UserId};
use redb::{ReadableTable, ReadableTableMetadata};

use super::{
    CLAIM_PRESENTATIONS, CLAIM_SEEN, CLAIMS, MAX_CLAIMS_PER_ANSWER, MAX_KEPT_CLAIMS, TOTALS, Vault,
};
use crate::error::Result;

impl Vault {
    /// Keep the claim `device` presented for `owner`, replacing its last one,
    /// and note that it is now the most recently presented.
    /// Not kept when [`MAX_KEPT_CLAIMS`] devices are already kept.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn keep_claim(&self, device: DeviceId, owner: UserId, claim: &[u8]) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            let mut claims = txn.open_table(CLAIMS)?;
            let key = device.as_bytes().as_slice();
            if claims.get(key)?.is_none() && claims.len()? >= MAX_KEPT_CLAIMS {
                return Ok(());
            }
            let mut row = owner.as_bytes().to_vec();
            row.extend_from_slice(claim);
            claims.insert(key, row.as_slice())?;
            let mut totals = txn.open_table(TOTALS)?;
            let seen = totals
                .get(CLAIM_PRESENTATIONS)?
                .map_or(0, |count| count.value())
                .saturating_add(1);
            totals.insert(CLAIM_PRESENTATIONS, seen)?;
            txn.open_table(CLAIM_SEEN)?.insert(key, seen)?;
        }
        txn.commit()?;
        Ok(())
    }

    /// The claim kept for `device` and the account it was kept for, if any:
    /// read before [`Self::keep_claim`], so a claim that says nothing new
    /// costs no commit.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn kept_claim(&self, device: DeviceId) -> Result<Option<(UserId, Vec<u8>)>> {
        let txn = self.db.begin_read()?;
        let Some(row) = txn.open_table(CLAIMS)?.get(device.as_bytes().as_slice())? else {
            return Ok(None);
        };
        let bytes = row.value();
        let Some((owner, claim)) = bytes.split_first_chunk::<32>() else {
            return Ok(None);
        };
        Ok(Some((UserId::from_bytes(*owner), claim.to_vec())))
    }

    /// The claims kept for `owner`, at most [`MAX_CLAIMS_PER_ANSWER`], the
    /// most recently presented first.
    ///
    /// A withdrawn device's claim is kept like any other (this host is not
    /// told of withdrawals), and it stops being presented. Handing out the
    /// first sixteen by device id, as this did until 2026-10-23, let an
    /// account past sixteen devices ever -- ten live at the time and a few
    /// replaced --
    /// drop live ones from every answer, whichever sorted last. The ones
    /// presented last are the live ones; a claim kept before presentations
    /// were counted reads as the oldest.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn claims_for(&self, owner: UserId) -> Result<Vec<Vec<u8>>> {
        let txn = self.db.begin_read()?;
        let seen = txn.open_table(CLAIM_SEEN)?;
        let mut found = Vec::new();
        for row in txn.open_table(CLAIMS)?.iter()? {
            let (device, value) = row?;
            let bytes = value.value();
            if bytes.len() > 32 && bytes[..32] == *owner.as_bytes() {
                let when = seen.get(device.value())?.map_or(0, |n| n.value());
                found.push((when, bytes[32..].to_vec()));
            }
        }
        found.sort_by_key(|(when, _)| std::cmp::Reverse(*when));
        Ok(found
            .into_iter()
            .take(MAX_CLAIMS_PER_ANSWER)
            .map(|(_, claim)| claim)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::super::*;

    /// Red team, 2026-10-23 (found while building the fleet's CI member): a
    /// host keeps a claim per device that ever presented one, withdrawn or
    /// not, and handed out the first sixteen by device id. Twenty devices of
    /// one account, the four that sort last by id presenting last -- the
    /// live ones of an account that replaced machines -- were all missing
    /// from the answer, so a sibling learning the roster from this host
    /// never admitted them. A device that presents again moves back in.
    #[test]
    fn red_team_the_claims_presented_last_are_handed_out_past_sixteen_devices() {
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        let mut devices: Vec<DeviceId> = (10..30u8).map(|b| device(b).device_id()).collect();
        devices.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        let (withdrawn, live) = devices.split_at(16);
        let claim = |device: &DeviceId| format!("claim of {device}").into_bytes();
        for device in withdrawn.iter().chain(live) {
            vault
                .keep_claim(*device, owner, &claim(device))
                .expect("keep");
        }
        let answer = vault.claims_for(owner).expect("claims");
        assert_eq!(answer.len(), MAX_CLAIMS_PER_ANSWER);
        for device in live {
            assert!(
                answer.contains(&claim(device)),
                "a live device that presented last is missing from the answer: a sibling \
                 learning the roster from this host never admits it"
            );
        }
        // And the four presented first are the ones left out.
        for device in &withdrawn[..4] {
            assert!(
                !answer.contains(&claim(device)),
                "a claim presented before sixteen others was handed out instead of a newer one"
            );
        }
        // The oldest withdrawn device presents again (it was live after all).
        vault
            .keep_claim(withdrawn[0], owner, &claim(&withdrawn[0]))
            .expect("again");
        let answer = vault.claims_for(owner).expect("claims");
        assert!(
            answer.contains(&claim(&withdrawn[0])),
            "a device presenting again stayed out"
        );
        assert!(
            live.iter().all(|device| answer.contains(&claim(device))),
            "a live device was pushed out by one presenting again"
        );
        // Another account's claims never count against this one's sixteen.
        let other = keys(2).user_id();
        vault
            .keep_claim(device(99).device_id(), other, b"theirs")
            .expect("other");
        assert!(
            !vault
                .claims_for(owner)
                .expect("claims")
                .contains(&b"theirs".to_vec())
        );
    }
}
