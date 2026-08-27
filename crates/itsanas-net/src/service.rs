//! Answering a peer.
//!
//! Request in, response out. No sockets, no async, no I/O beyond the store and
//! the vault — which means every rule about what a peer may and may not obtain
//! can be tested directly, including the hostile cases, without standing up a
//! network.
//!
//! # The rule this module exists to enforce
//!
//! **A peer may fetch anything it asks for, because everything it can fetch is
//! either sealed or signed.** There is no access-control list, and there
//! deliberately isn't one: an access-control list is a thing that can be got
//! wrong, and the guarantee here does not need one. A chunk is ciphertext bound
//! to an address; a segment is ciphertext in a signed envelope. Serving either
//! to the wrong person reveals nothing.
//!
//! What the service *does* enforce is narrower and concerns resources rather
//! than secrecy: request limits, quota, and refusing to store things it did not
//! agree to store.

use itsanas_crypto::{DeviceId, UserId};
use itsanas_store::{Store, Vault};

use crate::{
    error::Result,
    protocol::{
        Head, MAX_SEGMENTS_PER_REQUEST, PROTOCOL_VERSION, Request, Response, challenge_proof,
    },
};

/// How much foreign data this node has agreed to hold, in bytes.
///
/// Enforced when accepting, not when serving: a node that has already taken
/// someone's data must keep serving it even if the quota is later lowered,
/// because silently refusing to serve data you accepted is how peers lose
/// files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pledge {
    pub bytes: u64,
}

impl Pledge {
    /// A node that has pledged nothing still syncs its own devices; it just
    /// hosts nothing for anyone else.
    pub const NONE: Self = Self { bytes: 0 };

    #[must_use]
    pub const fn gigabytes(count: u64) -> Self {
        Self {
            bytes: count * 1024 * 1024 * 1024,
        }
    }
}

/// Answers peer requests from a node's own store and its vault.
#[derive(Debug)]
pub struct PeerService<'a> {
    store: &'a Store,
    vault: &'a Vault,
    pledge: Pledge,
}

impl<'a> PeerService<'a> {
    #[must_use]
    pub const fn new(store: &'a Store, vault: &'a Vault, pledge: Pledge) -> Self {
        Self {
            store,
            vault,
            pledge,
        }
    }

    /// This node's device identity.
    #[must_use]
    pub fn device_id(&self) -> DeviceId {
        self.store.device_id()
    }

    /// Answer one request.
    ///
    /// Never returns `Err` for anything a peer did — a misbehaving peer gets a
    /// [`Response::Refused`], because turning a peer's bad request into a local
    /// error would let a peer decide when this node stops working. `Err` is
    /// reserved for this node's own storage failing.
    pub fn handle(&self, request: &Request) -> Result<Response> {
        if !request.is_acceptable() {
            return Ok(Response::Refused("malformed request".to_owned()));
        }

        match request {
            Request::Hello { protocol, .. } => Ok(Response::Hello {
                protocol: (*protocol).min(PROTOCOL_VERSION),
                device: self.device_id(),
            }),

            Request::Heads { owner } => self.heads(*owner),

            Request::Segments {
                owner,
                device,
                after,
                limit,
            } => {
                let limit = usize::from((*limit).min(MAX_SEGMENTS_PER_REQUEST));
                Ok(Response::Segments(
                    self.segments(*owner, *device, *after, limit)?,
                ))
            }

            Request::Chunk { owner, address } => Ok(Response::Chunk(self.chunk(*owner, address)?)),

            Request::HaveChunks { owner, addresses } => {
                let mut missing = Vec::new();
                for address in addresses {
                    if self.chunk(*owner, address)?.is_none() {
                        missing.push(*address);
                    }
                }
                Ok(Response::Missing(missing))
            }

            Request::StoreChunk {
                owner,
                address,
                sealed,
            } => {
                if self.would_exceed_pledge(sealed.len())? {
                    return Ok(Response::Refused("pledged capacity exhausted".to_owned()));
                }
                self.vault.put_chunk(*owner, address, sealed)?;
                Ok(Response::Stored { accepted: true })
            }

            Request::StoreSegment { envelope } => {
                if self.would_exceed_pledge(envelope.sealed_body.len())? {
                    return Ok(Response::Refused("pledged capacity exhausted".to_owned()));
                }
                // A rejected segment is the peer's problem, not ours: refuse it
                // and say why, rather than failing the connection.
                match self.vault.put_segment(envelope) {
                    Ok(_) => Ok(Response::Stored { accepted: true }),
                    Err(error) => Ok(Response::Refused(error.to_string())),
                }
            }

            Request::Challenge {
                owner,
                address,
                nonce,
            } => match self.chunk(*owner, address)? {
                Some(sealed) => Ok(Response::ChallengeProof(challenge_proof(nonce, &sealed))),
                None => Ok(Response::Refused("chunk not held".to_owned())),
            },
        }
    }

    /// Chain tips, from both this node's own log and its vault.
    ///
    /// Both sources matter. Asking a peer "what do you have for Alice" must
    /// include Alice's own devices if this *is* one of Alice's devices, and the
    /// segments it holds for her as a host if it is not.
    fn heads(&self, owner: UserId) -> Result<Response> {
        let mut heads: Vec<Head> = self
            .vault
            .heads_for(owner)?
            .into_iter()
            .map(|(device, head, length)| Head {
                device,
                head,
                length,
            })
            .collect();

        if owner == self.store.owner()
            && let Some(head) = self.store.head_segment()?
        {
            heads.push(Head {
                device: self.store.device_id(),
                head,
                length: self.store.chain_length()?,
            });
        }

        // Deterministic order, so two identical nodes answer identically and a
        // difference in a test is a real difference.
        heads.sort_by_key(|head| head.device.to_bytes());
        Ok(Response::Heads(heads))
    }

    fn segments(
        &self,
        owner: UserId,
        device: DeviceId,
        after: Option<itsanas_crypto::ObjectId>,
        limit: usize,
    ) -> Result<Vec<itsanas_store::SegmentEnvelope>> {
        // This node's own chain, if that is what was asked for.
        if owner == self.store.owner() && device == self.store.device_id() {
            let mine = self.store.segments()?;
            let start = match after {
                None => 0,
                Some(id) => match mine.iter().position(|s| s.segment_id == id) {
                    Some(index) => index + 1,
                    // A resume point we do not recognise. Returning everything
                    // would re-send history the caller already has.
                    None => return Ok(Vec::new()),
                },
            };
            return Ok(mine.into_iter().skip(start).take(limit).collect());
        }

        Ok(self.vault.segments_for(owner, device, after, limit)?)
    }

    fn chunk(&self, owner: UserId, address: &itsanas_crypto::ChunkId) -> Result<Option<Vec<u8>>> {
        // Our own chunks first: if this is our data, the store is authoritative
        // and the vault would not hold it.
        if owner == self.store.owner()
            && let Some(sealed) = self.store.blobs().get(address)?
        {
            return Ok(Some(sealed));
        }

        Ok(self.vault.get_chunk(owner, address)?)
    }

    fn would_exceed_pledge(&self, incoming: usize) -> Result<bool> {
        let held = self.vault.stats()?.bytes;
        Ok(held.saturating_add(incoming as u64) > self.pledge.bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use itsanas_crypto::{ChunkId, DeviceKeys, MasterSecret, SecretBytes, UserKeys};
    use itsanas_store::ChunkerConfig;

    use crate::protocol::challenge_holds;

    struct Node {
        _dir: tempfile::TempDir,
        store: Store,
        vault: Vault,
    }

    fn node(master: &MasterSecret, device_seed: u8) -> Node {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Store::open_for_testing(
            dir.path().join("store"),
            UserKeys::derive(master),
            DeviceKeys::from_seed(&SecretBytes::new([device_seed; 32])),
            ChunkerConfig::default(),
        )
        .expect("store");
        let vault = Vault::open(dir.path().join("vault")).expect("vault");

        Node {
            _dir: dir,
            store,
            vault,
        }
    }

    fn service(node: &Node) -> PeerService<'_> {
        PeerService::new(&node.store, &node.vault, Pledge::gigabytes(1))
    }

    fn alice() -> MasterSecret {
        MasterSecret::from_bytes([0xA1; 32])
    }

    #[test]
    fn hello_reports_this_nodes_device_and_agrees_on_a_version() {
        let node = node(&alice(), 1);
        let service = service(&node);

        let response = service
            .handle(&Request::Hello {
                protocol: PROTOCOL_VERSION,
                device: DeviceId::from_bytes([9; 32]),
                owner: node.store.owner(),
            })
            .unwrap();

        match response {
            Response::Hello { protocol, device } => {
                assert_eq!(protocol, PROTOCOL_VERSION);
                assert_eq!(device, node.store.device_id());
            }
            other => panic!("expected a hello, got {other:?}"),
        }
    }

    #[test]
    fn a_hello_from_a_future_protocol_version_is_refused_not_guessed_at() {
        let node = node(&alice(), 2);
        let response = service(&node)
            .handle(&Request::Hello {
                protocol: PROTOCOL_VERSION + 5,
                device: DeviceId::from_bytes([9; 32]),
                owner: node.store.owner(),
            })
            .unwrap();

        assert!(matches!(response, Response::Refused(_)));
    }

    #[test]
    fn a_peer_can_fetch_this_nodes_own_segments_and_chunks() {
        let node = node(&alice(), 3);
        node.store.write_file("notes.txt", b"content").unwrap();
        let entry = node.store.stat("notes.txt").unwrap().unwrap();
        node.store.flush_segment().unwrap();

        let service = service(&node);

        let heads = match service
            .handle(&Request::Heads {
                owner: node.store.owner(),
            })
            .unwrap()
        {
            Response::Heads(heads) => heads,
            other => panic!("expected heads, got {other:?}"),
        };
        assert_eq!(heads.len(), 1);
        assert_eq!(heads[0].device, node.store.device_id());
        assert_eq!(heads[0].length, 1);

        let segments = match service
            .handle(&Request::Segments {
                owner: node.store.owner(),
                device: node.store.device_id(),
                after: None,
                limit: 10,
            })
            .unwrap()
        {
            Response::Segments(segments) => segments,
            other => panic!("expected segments, got {other:?}"),
        };
        assert_eq!(segments.len(), 1);
        segments[0].verify_signature().unwrap();

        let chunk = match service
            .handle(&Request::Chunk {
                owner: node.store.owner(),
                address: entry.chunks[0],
            })
            .unwrap()
        {
            Response::Chunk(chunk) => chunk,
            other => panic!("expected a chunk, got {other:?}"),
        };
        assert!(chunk.is_some());
    }

    #[test]
    fn what_a_peer_fetches_is_useless_without_the_key() {
        // The reason there is no access-control list: serving this to the wrong
        // person reveals nothing.
        let node = node(&alice(), 4);
        node.store
            .write_file("secret.txt", b"ITSANAS-SERVICE-CANARY-8f2a")
            .unwrap();
        let entry = node.store.stat("secret.txt").unwrap().unwrap();

        let chunk = match service(&node)
            .handle(&Request::Chunk {
                owner: node.store.owner(),
                address: entry.chunks[0],
            })
            .unwrap()
        {
            Response::Chunk(Some(bytes)) => bytes,
            other => panic!("expected a chunk, got {other:?}"),
        };

        let needle = b"ITSANAS-SERVICE-CANARY-8f2a";
        assert!(
            !chunk.windows(needle.len()).any(|w| w == needle),
            "the bytes served to a peer contain the plaintext"
        );

        // And a different user genuinely cannot open them.
        let stranger = UserKeys::derive(&MasterSecret::from_bytes([0xBB; 32]));
        assert!(
            stranger.open_chunk(&entry.chunks[0], &chunk).is_err(),
            "a stranger opened a chunk served to them"
        );
    }

    #[test]
    fn an_unknown_chunk_is_none_rather_than_an_error() {
        // "I do not have it" is ordinary. Turning it into an error would let a
        // peer's request decide when this node reports a fault.
        let node = node(&alice(), 5);
        let response = service(&node)
            .handle(&Request::Chunk {
                owner: node.store.owner(),
                address: ChunkId::from_bytes([0xEE; 32]),
            })
            .unwrap();

        assert_eq!(response, Response::Chunk(None));
    }

    #[test]
    fn a_node_stores_and_serves_a_strangers_chunk_without_reading_it() {
        // The whole mutual-storage bargain, in one test.
        let host = node(&alice(), 6);
        let guest_keys = UserKeys::derive(&MasterSecret::from_bytes([0xC0; 32]));
        let (address, sealed) = guest_keys.seal_chunk(b"the guest's private data").unwrap();

        let service = service(&host);

        assert_eq!(
            service
                .handle(&Request::StoreChunk {
                    owner: guest_keys.user_id(),
                    address,
                    sealed: sealed.clone(),
                })
                .unwrap(),
            Response::Stored { accepted: true }
        );

        let served = match service
            .handle(&Request::Chunk {
                owner: guest_keys.user_id(),
                address,
            })
            .unwrap()
        {
            Response::Chunk(Some(bytes)) => bytes,
            other => panic!("the host did not serve back what it stored: {other:?}"),
        };
        assert_eq!(served, sealed);

        // The host's own keys are useless against it.
        assert!(
            UserKeys::derive(&alice())
                .open_chunk(&address, &served)
                .is_err(),
            "the host could read the data it is storing for someone else"
        );
        // The guest's keys work.
        assert_eq!(
            guest_keys.open_chunk(&address, &served).unwrap(),
            b"the guest's private data"
        );
    }

    #[test]
    fn a_storage_challenge_passes_when_held_and_fails_when_not() {
        let host = node(&alice(), 7);
        let guest = UserKeys::derive(&MasterSecret::from_bytes([0xC1; 32]));
        let (address, sealed) = guest.seal_chunk(b"prove you have this").unwrap();

        let service = service(&host);
        service
            .handle(&Request::StoreChunk {
                owner: guest.user_id(),
                address,
                sealed: sealed.clone(),
            })
            .unwrap();

        let nonce = [0x5A; 32];
        let proof = match service
            .handle(&Request::Challenge {
                owner: guest.user_id(),
                address,
                nonce,
            })
            .unwrap()
        {
            Response::ChallengeProof(proof) => proof,
            other => panic!("expected a proof, got {other:?}"),
        };

        assert!(
            challenge_holds(&nonce, &sealed, &proof),
            "a host holding the chunk failed its own challenge"
        );

        // A chunk it never had.
        assert!(matches!(
            service
                .handle(&Request::Challenge {
                    owner: guest.user_id(),
                    address: ChunkId::from_bytes([0xDD; 32]),
                    nonce,
                })
                .unwrap(),
            Response::Refused(_)
        ));
    }

    #[test]
    fn a_host_that_discarded_a_chunk_cannot_fake_the_proof() {
        let host = node(&alice(), 8);
        let guest = UserKeys::derive(&MasterSecret::from_bytes([0xC2; 32]));
        let (address, sealed) = guest.seal_chunk(b"data the host will drop").unwrap();

        let service = service(&host);
        service
            .handle(&Request::StoreChunk {
                owner: guest.user_id(),
                address,
                sealed: sealed.clone(),
            })
            .unwrap();

        // The host quietly deletes it to save space.
        host.vault.remove_chunk(guest.user_id(), &address).unwrap();

        assert!(
            matches!(
                service
                    .handle(&Request::Challenge {
                        owner: guest.user_id(),
                        address,
                        nonce: [1; 32],
                    })
                    .unwrap(),
                Response::Refused(_)
            ),
            "a host that discarded a chunk still passed its challenge"
        );
    }

    #[test]
    fn storing_beyond_the_pledge_is_refused() {
        // A node must be able to bound what it takes on, or "pledge 10 GB" is
        // meaningless and the disk fills.
        let host = node(&alice(), 9);
        let guest = UserKeys::derive(&MasterSecret::from_bytes([0xC3; 32]));
        let service = PeerService::new(&host.store, &host.vault, Pledge { bytes: 512 });

        let (first, first_sealed) = guest.seal_chunk(&vec![1u8; 400]).unwrap();
        assert_eq!(
            service
                .handle(&Request::StoreChunk {
                    owner: guest.user_id(),
                    address: first,
                    sealed: first_sealed,
                })
                .unwrap(),
            Response::Stored { accepted: true }
        );

        let (second, second_sealed) = guest.seal_chunk(&vec![2u8; 400]).unwrap();
        assert!(
            matches!(
                service
                    .handle(&Request::StoreChunk {
                        owner: guest.user_id(),
                        address: second,
                        sealed: second_sealed,
                    })
                    .unwrap(),
                Response::Refused(_)
            ),
            "the node accepted more than it pledged and will fill its disk"
        );
    }

    #[test]
    fn a_node_that_pledged_nothing_still_serves_its_own_data() {
        // Hosting nothing must not break syncing your own devices.
        let node = node(&alice(), 10);
        node.store.write_file("mine.txt", b"my own file").unwrap();
        node.store.flush_segment().unwrap();

        let service = PeerService::new(&node.store, &node.vault, Pledge::NONE);

        match service
            .handle(&Request::Heads {
                owner: node.store.owner(),
            })
            .unwrap()
        {
            Response::Heads(heads) => assert_eq!(heads.len(), 1),
            other => panic!("expected heads, got {other:?}"),
        }
    }

    #[test]
    fn a_forged_segment_is_refused_rather_than_stored() {
        let host = node(&alice(), 11);
        let guest_master = MasterSecret::from_bytes([0xC4; 32]);
        let guest = node(&guest_master, 12);

        guest.store.write_file("theirs.txt", b"content").unwrap();
        let mut envelope = guest.store.flush_segment().unwrap().unwrap();
        envelope.sealed_body[0] ^= 0xFF;

        let response = service(&host)
            .handle(&Request::StoreSegment {
                envelope: Box::new(envelope),
            })
            .unwrap();

        assert!(
            matches!(response, Response::Refused(_)),
            "a host accepted a segment with a broken signature"
        );
    }

    #[test]
    fn a_bad_request_never_becomes_a_local_error() {
        // A peer must not be able to decide when this node reports a fault.
        let node = node(&alice(), 13);
        let service = service(&node);

        for request in [
            Request::Segments {
                owner: node.store.owner(),
                device: node.store.device_id(),
                after: None,
                limit: 0,
            },
            Request::Hello {
                protocol: 9999,
                device: DeviceId::from_bytes([1; 32]),
                owner: node.store.owner(),
            },
        ] {
            let response = service
                .handle(&request)
                .expect("a peer's bad request must not be a local error");
            assert!(matches!(response, Response::Refused(_)));
        }
    }

    #[test]
    fn heads_for_an_unknown_owner_are_empty_rather_than_an_error() {
        let node = node(&alice(), 14);
        let response = service(&node)
            .handle(&Request::Heads {
                owner: UserKeys::derive(&MasterSecret::from_bytes([0xEE; 32])).user_id(),
            })
            .unwrap();

        assert_eq!(response, Response::Heads(Vec::new()));
    }

    #[test]
    fn the_segment_limit_is_clamped_to_the_protocol_maximum() {
        let node = node(&alice(), 15);
        for round in 0..5 {
            node.store
                .write_file(&format!("f{round}.txt"), b"x")
                .unwrap();
            node.store.flush_segment().unwrap();
        }

        match service(&node)
            .handle(&Request::Segments {
                owner: node.store.owner(),
                device: node.store.device_id(),
                after: None,
                limit: 2,
            })
            .unwrap()
        {
            Response::Segments(segments) => assert_eq!(segments.len(), 2),
            other => panic!("expected segments, got {other:?}"),
        }
    }
}
