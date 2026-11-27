//! Claims an account presents to a host, and the presences a host relays.

use itsanas_crypto::{DeviceId, UserId};

use crate::{error::Result, protocol::Response};

use super::{NO_BOOK, NOT_YOURS, PeerService};

impl PeerService<'_> {
    /// Take the claim `caller` presented, if this host bounds accounts.
    pub(super) fn claim(&self, caller: DeviceId, claim: &[u8]) -> Response {
        match self.owners {
            // A host that bounds nobody has no use for it, and says so the
            // same way it says it took it: nothing for a peer to act on.
            None => Response::Stored { accepted: false },
            Some(owners) => match owners.present(caller, claim, self.vault) {
                Ok(()) => {
                    // Kept as presented, so the account's other machines can
                    // be handed it (`Request::Claims`). A failed write costs
                    // a sibling one round of waiting, never this session.
                    if let Some(owner) = owners.presented_for(caller) {
                        // Read first: every connection re-signs the claim,
                        // and rewriting one that says nothing new was a
                        // commit per peer per round (the idle bench).
                        let unchanged = self.vault.kept_claim(caller).ok().flatten().is_some_and(
                            |(kept_for, kept)| {
                                kept_for == owner && owners.adds_nothing(&kept, claim)
                            },
                        );
                        // A machine of this account withdrawn here keeps its
                        // revoked claim: the old live one it still presents
                        // would replace it, and the siblings asking would be
                        // handed the claim that admits it.
                        let withdrawn = owner == self.store.owner()
                            && self.store.is_revoked(&caller).unwrap_or(false);
                        if !unchanged && !withdrawn {
                            let _ = self.vault.keep_claim(caller, owner, claim);
                        }
                        // A sibling: `present` verified this account's
                        // signature and refused a revocation, which is what
                        // admission to the roster is. A machine that is only
                        // ever pushed to learns its siblings here.
                        if owner == self.store.owner() {
                            let _ = self.store.admit_member(&caller, false);
                            if let Some(seen) = self.sibling_seen
                                && self.store.is_member(&caller).unwrap_or(false)
                            {
                                seen.fetch_max(
                                    itsanas_store::now_unix(),
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                            }
                        }
                    }
                    self.claim_answer()
                }
                Err(why) => Response::Refused(why),
            },
        }
    }

    /// A claim taken: this host's own in return, so the caller can bound
    /// what it pulls from it, or a plain acknowledgement.
    pub(super) fn claim_answer(&self) -> Response {
        match &self.own_claim {
            Some(own) => Response::Claim(own.clone()),
            None => Response::Stored { accepted: true },
        }
    }

    /// The claims this host keeps for `owner`, for a caller that presented a
    /// claim for `owner` itself. Anyone else learns nothing, not even that
    /// the host knows the account.
    pub(super) fn claims(&self, caller: DeviceId, owner: UserId) -> Result<Response> {
        let Some(owners) = self.owners else {
            return Ok(Response::Claims(Vec::new()));
        };
        if owners.presented_for(caller) != Some(owner) {
            return Ok(Response::Claims(Vec::new()));
        }
        Ok(Response::Claims(
            self.vault
                .claims_for(owner)?
                .into_iter()
                .filter(|claim| claim.len() <= crate::protocol::MAX_CLAIM_BYTES)
                .collect(),
        ))
    }

    /// The account's machines, for a caller the book counts as one of them.
    ///
    /// Trimmed to the wire's bounds here as well as by the book, so a book
    /// that grew past them costs the caller rows and never the connection.
    pub(super) fn presences(&self, caller: DeviceId) -> Response {
        let Some(relay) = self.relay else {
            return Response::Refused(NO_BOOK.to_owned());
        };
        match relay.presences_for(caller) {
            Some(rows) => Response::Presences(
                rows.into_iter()
                    .filter(|row| row.len() <= crate::protocol::MAX_RELAYED_ROW_BYTES)
                    .take(crate::protocol::MAX_RELAYED_ROWS)
                    .collect(),
            ),
            None => Response::Refused(NOT_YOURS.to_owned()),
        }
    }
}
