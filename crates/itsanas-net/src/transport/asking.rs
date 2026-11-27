//! One method per request a client puts to a peer, each answering in the
//! caller's terms and degrading for a peer too old to know the verb.

use itsanas_crypto::{ChunkId, DeviceId, ObjectId, UserId};
use itsanas_store::SegmentEnvelope;

use crate::{
    error::{NetError, Result},
    protocol::{
        Head, PROTOCOL_WITH_APP_MODE, PROTOCOL_WITH_CHUNK_SUMMARY, PROTOCOL_WITH_CLAIMS,
        PROTOCOL_WITH_DRAINING, PROTOCOL_WITH_DROP_NOTICES, PROTOCOL_WITH_EVICTION,
        PROTOCOL_WITH_LEAVING, PROTOCOL_WITH_PRESENCES, PROTOCOL_WITH_SUMMARY_ROOT, Request,
        Response,
    },
};

use super::client::{Offer, PeerClient};

impl PeerClient {
    /// Send a request and wait for its response.
    pub fn request(&mut self, request: &Request) -> Result<Response> {
        Ok(self.connection.exchange(request)?)
    }

    /// Ask what chains the peer holds for `owner`.
    pub fn heads(&mut self, owner: UserId) -> Result<Vec<Head>> {
        match self.request(&Request::Heads { owner })? {
            Response::Heads(heads) => Ok(heads),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse { expected: "heads" }),
        }
    }

    /// Fetch a run of segments from one device's chain.
    pub fn segments(
        &mut self,
        owner: UserId,
        device: DeviceId,
        after: Option<ObjectId>,
        limit: u16,
    ) -> Result<Vec<SegmentEnvelope>> {
        match self.request(&Request::Segments {
            owner,
            device,
            after,
            limit,
        })? {
            Response::Segments(segments) => Ok(segments),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse {
                expected: "segments",
            }),
        }
    }

    /// Fetch one sealed chunk.
    pub fn chunk(&mut self, owner: UserId, address: ChunkId) -> Result<Option<Vec<u8>>> {
        match self.request(&Request::Chunk { owner, address })? {
            Response::Chunk(chunk) => Ok(chunk),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse { expected: "chunk" }),
        }
    }

    /// Ask the peer whether it has anything it would like this node to hold.
    ///
    /// The question that makes hosting mutual over a single outbound
    /// connection. Everything else here runs one way -- this node offers its
    /// work and the peer stores it -- which made hosting something only the
    /// dialled side could do, and so shut out every member behind a router they
    /// do not control.
    ///
    /// Returns whose data it is along with the list, because the opening
    /// exchange carries this node's owner to the peer and not the other way
    /// round.
    ///
    /// # Errors
    ///
    /// If the peer refuses, or answers something else.
    pub fn want_hosted(&mut self, limit: u32) -> Result<(UserId, Vec<ChunkId>)> {
        match self.request(&Request::WantHosted { limit })? {
            // Held to the limit here, where the peer's list arrives: the
            // host looks up or fetches every id it names.
            Response::WantHosted { owner, chunks }
                if u32::try_from(chunks.len()).is_ok_and(|len| len <= limit) =>
            {
                Ok((owner, chunks))
            }
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse {
                expected: "chunks wanted",
            }),
        }
    }

    /// Tell the peer which of its chunks this node has taken.
    ///
    /// So the peer can record who holds them. It is the owner's ledger, and
    /// this is a claim rather than a proof -- the owner's storage challenges
    /// are what make it evidence.
    ///
    /// # Errors
    ///
    /// If the peer refuses, or answers something else.
    pub fn hosted(&mut self, chunks: Vec<ChunkId>) -> Result<bool> {
        match self.request(&Request::Hosted { chunks })? {
            Response::Stored { accepted } => Ok(accepted),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse { expected: "stored" }),
        }
    }

    /// Ask whether this peer holds the same chunks for `owner`.
    ///
    /// `None` when the peer is too old to be asked, which is not a failure:
    /// the caller then does what every round did before, and lists everything.
    pub fn chunk_summary(&mut self, owner: UserId) -> Result<Option<Vec<[u8; 32]>>> {
        if self.spoken < PROTOCOL_WITH_CHUNK_SUMMARY {
            return Ok(None);
        }
        match self.request(&Request::ChunkSummary { owner })? {
            // A summary of the wrong length is not a peer that holds different
            // data; it is a peer that is not answering the question. Treating
            // it as "everything differs" would let eight bytes of nonsense buy
            // a full listing of the account, every round, for ever -- the best
            // amplification ratio available in this protocol. It is a refusal.
            Response::ChunkSummary(digests) if digests.len() == itsanas_store::summary::BUCKETS => {
                Ok(Some(digests))
            }
            Response::ChunkSummary(_) => Err(NetError::UnexpectedResponse {
                expected: "a chunk summary with one digest per bucket",
            }),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse {
                expected: "chunk summary",
            }),
        }
    }

    /// The root of this peer's chunk summary for `owner`: `None` when the
    /// peer is too old to be asked, and the caller asks the whole summary.
    pub fn summary_root(&mut self, owner: UserId) -> Result<Option<[u8; 32]>> {
        if self.spoken < PROTOCOL_WITH_SUMMARY_ROOT {
            return Ok(None);
        }
        match self.request(&Request::SummaryRoot { owner })? {
            Response::SummaryRoot(root) => Ok(Some(root)),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse {
                expected: "a summary root",
            }),
        }
    }

    /// Whether this peer runs in application mode: `None` when it is too old
    /// to be asked (before [`PROTOCOL_WITH_APP_MODE`]), which the caller
    /// takes as not.
    pub fn app_mode(&mut self) -> Result<Option<bool>> {
        if self.spoken < PROTOCOL_WITH_APP_MODE {
            return Ok(None);
        }
        match self.request(&Request::AppMode)? {
            Response::AppMode { on } => Ok(Some(on)),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse {
                expected: "whether the peer runs in application mode",
            }),
        }
    }

    /// Tell the peer this device no longer holds these chunks.
    ///
    /// Answers `false`, having sent nothing, when the peer is too old to know
    /// the verb. That is not a failure: the audit still catches the same
    /// staleness, more slowly, which is what happened before this existed.
    ///
    /// Batched by the caller: the request carries at most `MAX_HAVE_BATCH`
    /// addresses, the same ceiling the have/missing exchange uses.
    pub fn dropped(&mut self, owner: UserId, chunks: Vec<ChunkId>) -> Result<bool> {
        if self.spoken < PROTOCOL_WITH_DROP_NOTICES || chunks.is_empty() {
            return Ok(false);
        }
        match self.request(&Request::Dropped { owner, chunks })? {
            Response::Stored { accepted } => Ok(accepted),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse { expected: "stored" }),
        }
    }

    /// Tell the peer this device is going and should no longer be counted.
    ///
    /// Answers `false`, having sent nothing, when the peer is too old to know
    /// the verb: its records age out of `LIVE_FOR` instead, which is what
    /// happened before this existed.
    pub fn leaving(&mut self) -> Result<bool> {
        if self.spoken < PROTOCOL_WITH_LEAVING {
            return Ok(false);
        }
        match self.request(&Request::Leaving)? {
            Response::Stored { accepted } => Ok(accepted),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse { expected: "stored" }),
        }
    }

    /// Ask the peer to stop counting this device for `seconds`, and how many
    /// of its chunks rest on this device alone.
    ///
    /// `None`, having sent nothing, when the peer is too old to know the verb:
    /// the caller counts it as an owner it could not ask, never as 0.
    pub fn draining(&mut self, seconds: u64) -> Result<Option<u64>> {
        if self.spoken < PROTOCOL_WITH_DRAINING {
            return Ok(None);
        }
        match self.request(&Request::Draining { seconds })? {
            Response::Exposed { chunks } => Ok(Some(chunks)),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse {
                expected: "exposed",
            }),
        }
    }

    /// Tell the peer which of the chunks it holds for `owner` it may evict to
    /// make room for `owner` ([`Request::MayEvict`]), and learn how many of
    /// them it now holds marked.
    ///
    /// `None`, having sent nothing, when the peer is too old to know the verb
    /// ([`PROTOCOL_WITH_EVICTION`]): an older peer closes the connection on a
    /// request it cannot decode, and the round still needed it. The caller
    /// then keeps counting those copies, which is what happened before this
    /// existed. `Some(0)`, having sent nothing, for an empty list.
    ///
    /// Batched by the caller: the request carries at most `MAX_HAVE_BATCH`
    /// chunks, and a longer one is refused as malformed.
    ///
    /// # Errors
    ///
    /// If the peer refuses -- it does not count this device as one that
    /// speaks for `owner`, or bounds nobody -- or answers something else.
    pub fn may_evict(&mut self, owner: UserId, chunks: Vec<ChunkId>) -> Result<Option<u32>> {
        if self.spoken < PROTOCOL_WITH_EVICTION {
            return Ok(None);
        }
        if chunks.is_empty() {
            return Ok(Some(0));
        }
        match self.request(&Request::MayEvict { owner, chunks })? {
            Response::MayEvict { marked } => Ok(Some(marked)),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse {
                expected: "chunks marked evictable",
            }),
        }
    }

    /// Ask the peer where the account's machines are, as it knows them.
    ///
    /// `None` when the peer cannot tell: it is older than
    /// [`PROTOCOL_WITH_PRESENCES`] (and is not asked), or it refused -- it
    /// keeps no book, or does not count this device as one of its account.
    /// Neither is an error: the coordinator remains the other source.
    ///
    /// The rows are the peer's word until checked, and the caller checks
    /// every one (`itsanas_node::contact::Contact::relayed`). What is bounded
    /// here is only their number and size, past which the answer is refused
    /// whole rather than trimmed: a peer that pads is not one to believe.
    ///
    /// # Errors
    ///
    /// If the connection fails or the answer is not a list of presences.
    pub fn presences(&mut self) -> Result<Option<Vec<Vec<u8>>>> {
        if self.spoken < PROTOCOL_WITH_PRESENCES {
            return Ok(None);
        }
        match self.request(&Request::Presences)? {
            Response::Presences(rows)
                if rows.len() <= crate::protocol::MAX_RELAYED_ROWS
                    && rows
                        .iter()
                        .all(|row| row.len() <= crate::protocol::MAX_RELAYED_ROW_BYTES) =>
            {
                Ok(Some(rows))
            }
            Response::Presences(_) => Err(NetError::UnexpectedResponse {
                expected: "a bounded list of presences",
            }),
            Response::Refused(_) => Ok(None),
            _ => Err(NetError::UnexpectedResponse {
                expected: "presences",
            }),
        }
    }

    /// Present this device's account claim, so a host that bounds what it
    /// stores per account (§8 1c) will store for it.
    ///
    /// `Ok(Some(claim))`: the host took it and answered with its own claim,
    /// which the caller holds the host to when pulling its chunks
    /// ([`crate::session::host_for_bounded`]). `Ok(None)`: the host is older
    /// than [`PROTOCOL_WITH_CLAIMS`] (not asked), bounds nobody, or took it
    /// without answering with its own. A refusal is an error, because the
    /// stores that follow will be refused too and the operator should read
    /// why once, not once per chunk.
    ///
    /// # Errors
    ///
    /// If the host refused the claim, or the connection failed.
    pub fn present_claim(&mut self, claim: Vec<u8>) -> Result<Option<Vec<u8>>> {
        if self.spoken < PROTOCOL_WITH_CLAIMS {
            return Ok(None);
        }
        match self.request(&Request::Claim { claim })? {
            Response::Claim(theirs) if theirs.len() <= crate::protocol::MAX_CLAIM_BYTES => {
                Ok(Some(theirs))
            }
            Response::Stored { .. } => Ok(None),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse { expected: "stored" }),
        }
    }

    /// The claims devices of `owner` presented to this host, unverified:
    /// the caller checks every signature before believing one. Empty from a
    /// host older than [`crate::protocol::PROTOCOL_WITH_ROSTER`] (not asked),
    /// one that keeps none, or one this device has not presented a claim to
    /// for `owner` in this session.
    ///
    /// # Errors
    ///
    /// If the connection failed or the answer was out of bounds.
    pub fn claims(&mut self, owner: UserId) -> Result<Vec<Vec<u8>>> {
        if self.spoken < crate::protocol::PROTOCOL_WITH_ROSTER {
            return Ok(Vec::new());
        }
        match self.request(&Request::Claims { owner })? {
            Response::Claims(claims)
                if claims.len() <= itsanas_store::vault::MAX_CLAIMS_PER_ANSWER
                    && claims
                        .iter()
                        .all(|claim| claim.len() <= crate::protocol::MAX_CLAIM_BYTES) =>
            {
                Ok(claims)
            }
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse { expected: "claims" }),
        }
    }

    /// Ask which of `addresses` the peer lacks.
    ///
    /// The answer is a subset of the question, once each, or it is refused:
    /// every caller uploads or re-records what comes back, so an id the peer
    /// was not asked about, or one repeated, would be sent again and credited
    /// to it outside placement.
    pub fn missing_chunks(
        &mut self,
        owner: UserId,
        addresses: Vec<ChunkId>,
    ) -> Result<Vec<ChunkId>> {
        let asked_len = addresses.len();
        let asked: std::collections::BTreeSet<ChunkId> = addresses.iter().copied().collect();
        match self.request(&Request::HaveChunks { owner, addresses })? {
            Response::Missing(missing)
                if missing.len() <= asked_len && missing.iter().all(|a| asked.contains(a)) =>
            {
                let unique: std::collections::BTreeSet<ChunkId> = missing.into_iter().collect();
                Ok(unique.into_iter().collect())
            }
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse {
                expected: "missing chunks",
            }),
        }
    }

    /// Offer a sealed chunk for storage, and say what the peer did with it.
    pub fn store_chunk(
        &mut self,
        owner: UserId,
        address: ChunkId,
        sealed: Vec<u8>,
    ) -> Result<Offer> {
        let answer = self.request(&Request::StoreChunk {
            owner,
            address,
            sealed,
        })?;
        Offer::from_response(answer)
    }

    /// Offer a segment for storage, and say what the peer did with it.
    pub fn store_segment(&mut self, envelope: &SegmentEnvelope) -> Result<Offer> {
        let answer = self.request(&Request::StoreSegment {
            envelope: Box::new(envelope.clone()),
        })?;
        Offer::from_response(answer)
    }

    /// Challenge the peer to prove it still holds a chunk.
    ///
    /// `expected` is the sealed bytes the verifier already knows — an owner can
    /// re-derive them, because chunk sealing is deterministic, without keeping
    /// a second copy.
    pub fn challenge(
        &mut self,
        owner: UserId,
        address: ChunkId,
        nonce: [u8; 32],
        expected: &[u8],
    ) -> Result<bool> {
        match self.request(&Request::Challenge {
            owner,
            address,
            nonce,
        })? {
            Response::ChallengeProof(proof) => {
                Ok(crate::protocol::challenge_holds(&nonce, expected, &proof))
            }
            // Away is not lost: the round ends without an audit verdict.
            Response::Refused(why) if why == crate::service::HOSTING_AWAY => {
                Err(NetError::Refused(why))
            }
            Response::Refused(_) => Ok(false),
            _ => Err(NetError::UnexpectedResponse {
                expected: "challenge proof",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use itsanas_crypto::DeviceKeys;
    use itsanas_tls::{Authenticated, Identity};

    use super::*;
    use crate::protocol::PROTOCOL_VERSION;

    /// A peer that answers the opening exchange honestly and then `answer`,
    /// whatever was asked: a host that lies about one verb.
    fn scripted_peer(answer: Response) -> (SocketAddr, std::thread::JoinHandle<()>) {
        use itsanas_crypto::SecretBytes;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let config = Identity::generate().unwrap().server_config().unwrap();
            let device = DeviceKeys::from_seed(&SecretBytes::new([41; 32]));
            let Authenticated { mut connection, .. } =
                itsanas_tls::accept(&config, &device, stream).unwrap();
            while let Ok(Some(request)) = connection.receive::<Request>() {
                let reply = match request {
                    Request::Hello { .. } => Response::Hello {
                        protocol: PROTOCOL_VERSION,
                        device: device.device_id(),
                    },
                    _ => answer.clone(),
                };
                if connection.send(&reply).is_err() {
                    return;
                }
            }
        });
        (address, handle)
    }

    fn dial(address: SocketAddr) -> PeerClient {
        use itsanas_crypto::SecretBytes;
        let device = DeviceKeys::from_seed(&SecretBytes::new([42; 32]));
        PeerClient::connect(address, &device, UserId::from_bytes([7; 32]), None).unwrap()
    }

    #[test]
    fn red_team_a_missing_answer_naming_chunks_not_asked_for_is_refused() {
        // THE ATTACK, by a host offered one chunk by placement: it answers
        // that it lacks that one, another the owner holds, and one id
        // repeated. The owner uploaded every id it named, credited the host
        // with copies the roster gave to others, and sent the same 256 KiB
        // again for the whole session. Sabotage: drop the subset check in
        // `missing_chunks`.
        let (a, b, c) = (
            ChunkId::from_bytes([1; 32]),
            ChunkId::from_bytes([2; 32]),
            ChunkId::from_bytes([3; 32]),
        );
        let (address, peer) = scripted_peer(Response::Missing(vec![a, b, b, b, c]));
        let mut client = dial(address);
        let answer = client.missing_chunks(UserId::from_bytes([7; 32]), vec![a]);
        assert!(
            matches!(answer, Err(NetError::UnexpectedResponse { .. })),
            "a host's answer named chunks it was not asked about and the owner took it: {answer:?}"
        );
        drop(client);
        peer.join().unwrap();
    }

    #[test]
    fn a_missing_answer_comes_back_without_repeats() {
        // An honest host answers a batch that names a chunk twice with it
        // twice; the owner uploads it once.
        let a = ChunkId::from_bytes([1; 32]);
        let (address, peer) = scripted_peer(Response::Missing(vec![a, a]));
        let mut client = dial(address);
        let answer = client
            .missing_chunks(UserId::from_bytes([7; 32]), vec![a, a])
            .unwrap();
        assert_eq!(answer, vec![a]);
        drop(client);
        peer.join().unwrap();
    }

    #[test]
    fn red_team_a_want_hosted_answer_longer_than_asked_is_refused() {
        // THE ATTACK, by a member this node hosts for: asked for 32 chunks,
        // it names 33 (or 262 144, a full frame). The host looked up or
        // fetched every one, then sent a `Hosted` over the protocol's batch
        // cap, which lost the round's genuine holder records too. Sabotage:
        // drop the length guard in `want_hosted`.
        let chunks = (0..33u8).map(|i| ChunkId::from_bytes([i; 32])).collect();
        let (address, peer) = scripted_peer(Response::WantHosted {
            owner: UserId::from_bytes([9; 32]),
            chunks,
        });
        let mut client = dial(address);
        let answer = client.want_hosted(32);
        assert!(
            matches!(answer, Err(NetError::UnexpectedResponse { .. })),
            "an answer longer than the limit asked was taken: {:?}",
            answer.map(|(_, chunks)| chunks.len())
        );
        drop(client);
        peer.join().unwrap();
    }
}
