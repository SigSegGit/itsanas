//! The book as the peer service asks it (`itsanas_net::Owners`).

use itsanas_coord::claim::SignedClaim;
use itsanas_crypto::{DeviceId, UserId};
use itsanas_store::{Store, Vault};

use super::{ClaimBook, KEEP_CLAIM_STEP, STANDING_FOR, Standing};

impl itsanas_net::Owners for ClaimBook {
    fn present(&self, caller: DeviceId, claim: &[u8], vault: &Vault) -> Result<(), String> {
        self.take(caller, claim, vault)
    }

    fn admits(
        &self,
        caller: DeviceId,
        owner: UserId,
        incoming: u64,
        store: &Store,
        vault: &Vault,
        host_pledge: u64,
    ) -> Result<(), String> {
        ClaimBook::admits(self, caller, owner, incoming, store, vault, host_pledge)
    }

    fn adds_nothing(&self, kept: &[u8], presented: &[u8]) -> bool {
        let (Ok(kept), Ok(presented)) = (
            postcard::from_bytes::<SignedClaim>(kept),
            postcard::from_bytes::<SignedClaim>(presented),
        ) else {
            return false;
        };
        let (kept, presented) = (&kept.claim, &presented.claim);
        // An older claim never replaces a newer one, whatever it says: a
        // device's claims are ordered by their date (`supersedes`).
        let older = presented.issued_unix <= kept.issued_unix;
        let redated = kept.pledged_bytes == presented.pledged_bytes
            && kept.revoked == presented.revoked
            && presented.issued_unix < kept.issued_unix.saturating_add(KEEP_CLAIM_STEP);
        kept.owner == presented.owner && kept.device == presented.device && (older || redated)
    }

    fn presented_for(&self, caller: DeviceId) -> Option<UserId> {
        // `take` verified the signature and refused a revoked claim; what is
        // left out here is a device the coordinator since said is withdrawn.
        self.lock()
            .get(&caller)
            .filter(|held| held.standing != Standing::Withdrawn)
            .map(|held| held.owner)
    }

    fn confirmed_account(&self, caller: DeviceId) -> Option<UserId> {
        self.lock()
            .get(&caller)
            .and_then(|held| match held.standing {
                Standing::Live(at) if at.elapsed() < STANDING_FOR => Some(held.owner),
                _ => None,
            })
    }

    /// The test `admits` puts a store to, before any question of room: a
    /// claim for `owner` presented by `caller`, confirmed lately and not
    /// withdrawn. A device whose standing lapsed may no longer store for the
    /// account, so it may no longer mark the account's chunks either.
    fn speaks_for(&self, caller: DeviceId, owner: UserId) -> bool {
        self.confirmed_account(caller) == Some(owner)
    }
}

#[cfg(test)]
mod tests {
    use itsanas_coord::claim::NodeClaim;
    use itsanas_crypto::{ChunkId, UserKeys};

    use super::*;
    use crate::owners::Verdict;
    use crate::owners::tests::{NOW, account, claim, device, host, present, presents};

    /// The idle bench of 2026-10-27, 84 KB a round with nothing to do: every
    /// connection re-signs the device's claim (`Node::claim_bytes` dates it
    /// now), and the host rewrote the copy it keeps for the account's other
    /// machines -- one commit per peer per round for a claim that said
    /// nothing new. Kept again only when something but its date changed, or
    /// its date moved [`KEEP_CLAIM_STEP`]; never replaced by an older one.
    #[test]
    fn red_team_a_claim_presented_again_unchanged_is_not_written_again() {
        use itsanas_net::{PeerService, Pledge, Request};
        let host = host();
        let service = PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1))
            .with_owners(&host.book);
        let alice = account(0x0A);
        let dated = |pledged: u64, issued: u64| {
            let signed = NodeClaim {
                owner: alice.user_id(),
                device: device(0x21),
                pledged_bytes: pledged,
                issued_unix: issued,
                revoked: false,
            }
            .sign(&alice);
            postcard::to_stdvec(&signed).unwrap()
        };
        let present = |bytes: &[u8]| {
            service
                .handle(
                    &Request::Claim {
                        claim: bytes.to_vec(),
                    },
                    device(0x21),
                )
                .unwrap()
        };
        let kept = || host.vault.claims_for(alice.user_id()).unwrap();

        let first = dated(100, NOW);
        present(&first);
        assert_eq!(kept(), vec![first.clone()], "the premise: a claim is kept");

        present(&dated(100, NOW + 300));
        assert_eq!(
            kept(),
            vec![first.clone()],
            "the same claim re-signed five minutes later was written again: a commit per \
             peer per round on a node with nothing to do"
        );

        let pledged = dated(200, NOW + 360);
        present(&pledged);
        assert_eq!(
            kept(),
            vec![pledged.clone()],
            "a new pledge was not kept, and siblings are handed the old one"
        );

        let later = dated(200, NOW + 360 + KEEP_CLAIM_STEP);
        present(&later);
        assert_eq!(
            kept(),
            vec![later.clone()],
            "a claim a whole step newer was not kept: siblings would be handed one ever older"
        );

        present(&first);
        assert_eq!(
            kept(),
            vec![later],
            "an older claim presented again replaced the newer one kept"
        );
    }

    #[test]
    fn red_team_a_host_hands_an_account_its_roster_and_nobody_else() {
        // HANDOVER §10 7. A host keeps the claims presented to it and hands
        // an account's claims to a device that presented one for that same
        // account: its siblings learn each other through hosts. A device of
        // another account, or one that presented nothing, learns nothing --
        // not even that the host knows the account.
        use itsanas_net::{PeerService, Pledge, Request, Response};
        let host = host();
        let service = PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1))
            .with_owners(&host.book);
        let (alice, mallory) = (account(0x0A), account(0x0B));
        let present = |seed, account: &UserKeys| {
            service
                .handle(
                    &Request::Claim {
                        claim: claim(account, seed, 100, false),
                    },
                    device(seed),
                )
                .unwrap()
        };
        assert!(!matches!(present(0x21, &alice), Response::Refused(_)));
        let ask = |seed| {
            service.handle(
                &Request::Claims {
                    owner: alice.user_id(),
                },
                device(seed),
            )
        };

        assert_eq!(
            ask(0x23).unwrap(),
            Response::Refused(itsanas_net::NOT_YOUR_ACCOUNT.to_owned()),
            "a device that presented nothing was told"
        );
        assert!(!matches!(present(0x24, &mallory), Response::Refused(_)));
        assert_eq!(
            ask(0x24).unwrap(),
            Response::Refused(itsanas_net::NOT_YOUR_ACCOUNT.to_owned()),
            "a device of another account was handed this account's devices"
        );

        assert!(!matches!(present(0x22, &alice), Response::Refused(_)));
        let Response::Claims(claims) = ask(0x22).unwrap() else {
            panic!("not claims")
        };
        assert_eq!(claims.len(), 2, "both of alice's devices, and only hers");
        assert!(claims.contains(&claim(&alice, 0x21, 100, false)));
    }

    #[test]
    fn red_team_a_sibling_presenting_here_joins_the_roster_and_a_stranger_does_not() {
        // A machine only ever pushed to never dials anyone, so it learns its
        // siblings from the claims they present. Only its own account's.
        use itsanas_net::{PeerService, Pledge, Request};
        let host = host();
        let service = PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1))
            .with_owners(&host.book);
        host.store
            .admit_member(&host.store.device_id(), false)
            .unwrap();
        let mine = account(0x01);
        assert_eq!(
            mine.user_id(),
            host.store.owner(),
            "the fixture's account changed"
        );
        for (seed, account) in [(0x31, &mine), (0x32, &account(0x0C))] {
            service
                .handle(
                    &Request::Claim {
                        claim: claim(account, seed, 0, false),
                    },
                    device(seed),
                )
                .unwrap();
        }
        assert!(host.store.may_apply_chain_of(&device(0x31)).unwrap());
        assert!(
            !host.store.may_apply_chain_of(&device(0x32)).unwrap(),
            "a device of another account joined this account's roster by presenting"
        );
    }

    /// A public machine the others only dial in to read `isolated` though
    /// they reached it at every round: only a contact it dialled counted.
    /// A sibling's claim presented here counts now, a stranger's does not.
    /// Sabotage: note nothing.
    #[test]
    fn red_team_a_sibling_presenting_here_is_a_contact_and_a_stranger_is_not() {
        use itsanas_net::{PeerService, Pledge, Request};
        let host = host();
        let seen = std::sync::atomic::AtomicU64::new(0);
        let service = PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1))
            .with_owners(&host.book)
            .noting_siblings(&seen);
        let present = |seed: u8, account: &UserKeys| {
            service
                .handle(
                    &Request::Claim {
                        claim: claim(account, seed, 0, false),
                    },
                    device(seed),
                )
                .unwrap();
        };
        present(0x32, &account(0x0C));
        assert_eq!(
            seen.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "a device of another account presenting here counted as this account reaching the node"
        );
        present(0x31, &account(0x01));
        assert!(
            seen.load(std::sync::atomic::Ordering::Relaxed) > 0,
            "a machine of this account dialled in and the node still reads as reaching nobody"
        );
    }

    #[test]
    fn red_team_nobody_but_the_account_reads_what_a_host_holds_for_it() {
        // ROADMAP, "reads are not scoped to the account": anyone who knew a
        // user id -- every host of it does -- read its device list, every
        // envelope and the sealed chunks from any host. Now every read names
        // its account and is answered only to a device whose presented claim
        // does: not a stranger, not a device of another account (a fellow
        // host), not this host's own peers of other accounts.
        use itsanas_net::{NOT_YOUR_ACCOUNT, PeerService, Pledge, Request, Response};
        let host = host();
        let service = PeerService::new(&host.store, &host.vault, Pledge::gigabytes(1))
            .with_owners(&host.book);
        let (alice, mallory) = (account(0x0A), account(0x0B));
        let owner = alice.user_id();
        let address = ChunkId::from_bytes([3; 32]);
        let reads = [
            Request::Heads { owner },
            Request::Segments {
                owner,
                device: device(0x41),
                after: None,
                limit: 1,
            },
            Request::Chunk { owner, address },
            Request::HaveChunks {
                owner,
                addresses: vec![address],
            },
            Request::ChunkSummary { owner },
            Request::Challenge {
                owner,
                address,
                nonce: [1; 32],
            },
            Request::Claims { owner },
        ];
        service
            .handle(
                &Request::Claim {
                    claim: claim(&mallory, 0x42, 100, false),
                },
                device(0x42),
            )
            .unwrap();
        for request in &reads {
            for caller in [device(0x40), device(0x42)] {
                assert_eq!(
                    service.handle(request, caller).unwrap(),
                    Response::Refused(NOT_YOUR_ACCOUNT.to_owned()),
                    "{request:?} was answered to a device that is not alice's"
                );
            }
        }
        service
            .handle(
                &Request::Claim {
                    claim: claim(&alice, 0x41, 100, false),
                },
                device(0x41),
            )
            .unwrap();
        for request in &reads {
            assert_ne!(
                service.handle(request, device(0x41)).unwrap(),
                Response::Refused(NOT_YOUR_ACCOUNT.to_owned()),
                "{request:?} was refused to alice's own device"
            );
        }
    }

    #[test]
    fn red_team_only_a_confirmed_device_of_the_account_speaks_for_it() {
        // What lets a device mark an account's chunks evictable on this
        // host. A device of another account, one never confirmed, and one
        // whose claim was withdrawn must not; the account's live device must.
        // Sabotages: answer `confirmed_account(..).is_some()`; ignore the
        // standing.
        use itsanas_net::Owners as _;
        let host = host();
        let (alice, bob) = (account(0xA1), account(0xB0));
        present(&host, &alice, 0x11, 700);
        present(&host, &bob, 0x12, 700);
        presents(&host, &alice, 0x13, 700);
        present(&host, &alice, 0x14, 700);
        host.book.note(device(0x14), Verdict::Withdrawn);

        assert!(host.book.speaks_for(device(0x11), alice.user_id()));
        assert!(!host.book.speaks_for(device(0x12), alice.user_id()));
        assert!(!host.book.speaks_for(device(0x13), alice.user_id()));
        assert!(!host.book.speaks_for(device(0x14), alice.user_id()));
        assert!(!host.book.speaks_for(device(0x99), alice.user_id()));
    }
}
