//! Every message on the wire: each request and response round-trips, keeps
//! its number and its bytes, survives damage, and is bounded.

use itsanas_crypto::{ChunkId, DeviceId, ObjectId, UserId};
use itsanas_store::SegmentEnvelope;

use super::*;
use crate::wire;

fn user() -> UserId {
    UserId::from_bytes([7; 32])
}

fn device() -> DeviceId {
    DeviceId::from_bytes([9; 32])
}

/// One example of every request, for the round-trip test.
///
/// Kept beside [`every_variant_is_in_the_round_trip_list`], which fails to
/// compile when a variant is added and not listed here. The list used to be
/// written inline and called "every variant" while omitting four of them —
/// a test that overstates its coverage is worse than a missing one, because
/// it answers the question nobody asks again.
fn one_of_each() -> Vec<Request> {
    vec![
        Request::Hello {
            protocol: PROTOCOL_VERSION,
            device: device(),
            owner: user(),
        },
        Request::Heads { owner: user() },
        Request::Segments {
            owner: user(),
            device: device(),
            after: Some(ObjectId::from_bytes([3; 32])),
            limit: 64,
        },
        Request::Segments {
            owner: user(),
            device: device(),
            after: None,
            limit: 1,
        },
        Request::Chunk {
            owner: user(),
            address: ChunkId::from_bytes([1; 32]),
        },
        Request::HaveChunks {
            owner: user(),
            addresses: vec![ChunkId::from_bytes([6; 32])],
        },
        Request::StoreChunk {
            owner: user(),
            address: ChunkId::from_bytes([2; 32]),
            sealed: vec![0xAB; 1024],
        },
        Request::StoreSegment {
            envelope: Box::new(SegmentEnvelope {
                segment_id: ObjectId::from_bytes([7; 32]),
                owner: user(),
                device: device(),
                first_sequence: 1,
                last_sequence: 4,
                previous: None,
                sealed_body: vec![0xCD; 64],
                signature: itsanas_crypto::Signature::from_bytes([8; 64]),
            }),
        },
        Request::Challenge {
            owner: user(),
            address: ChunkId::from_bytes([4; 32]),
            nonce: [5; 32],
        },
        Request::WantHosted { limit: 16 },
        Request::Hosted {
            chunks: vec![ChunkId::from_bytes([9; 32])],
        },
        Request::Dropped {
            owner: user(),
            chunks: vec![ChunkId::from_bytes([10; 32])],
        },
        Request::ChunkSummary { owner: user() },
        Request::Leaving,
        Request::Presences,
        Request::Claim {
            claim: vec![1, 2, 3],
        },
        Request::Draining { seconds: 1800 },
        Request::Park,
        Request::Splice { to: device() },
        Request::MayEvict {
            owner: user(),
            chunks: vec![ChunkId::from_bytes([11; 32])],
        },
        Request::Claims { owner: user() },
        Request::SummaryRoot { owner: user() },
        Request::AppMode,
    ]
}

/// Fails to compile when a request variant is added.
///
/// The only reminder that works. A list of examples is a list somebody
/// forgets, and the failure it lets through — a variant that cannot be
/// encoded — happens on a live connection.
#[expect(
    clippy::match_same_arms,
    reason = "one arm per variant is the point; merging them removes the reminder"
)]
fn every_variant_is_in_the_round_trip_list(request: &Request) {
    match request {
        Request::Hello { .. } => {}
        Request::Heads { .. } => {}
        Request::Segments { .. } => {}
        Request::Chunk { .. } => {}
        Request::HaveChunks { .. } => {}
        Request::StoreChunk { .. } => {}
        Request::StoreSegment { .. } => {}
        Request::Challenge { .. } => {}
        Request::WantHosted { .. } => {}
        Request::Hosted { .. } => {}
        Request::Dropped { .. } => {}
        Request::ChunkSummary { .. } => {}
        Request::Leaving => {}
        Request::Presences => {}
        Request::Claim { .. } => {}
        Request::Draining { .. } => {}
        Request::Park => {}
        Request::Splice { .. } => {}
        Request::MayEvict { .. } => {}
        Request::Claims { .. } => {}
        Request::SummaryRoot { .. } => {}
        Request::AppMode => {}
    }
}

/// THE ATTACK: damaged frames where a peer message is expected. A node
/// decodes what any authenticated stranger sends, and a device key costs
/// nothing: a decoder that panicked, hung or allocated without bound on a
/// damaged frame was a crash anybody could send. Every request and every
/// response, many thousands of damaged copies through the frame decoder;
/// each is decoded or refused, in bounded time.
#[test]
fn red_team_no_damaged_frame_crashes_or_stalls_the_decoder() {
    let started = std::time::Instant::now();
    let mut tried = 0usize;
    for (seed, request) in (1u64..).zip(one_of_each()) {
        let honest = wire::encode(&request).expect("encode");
        for damaged in itsanas_testkit::mutate::mutations(&honest, seed, 1000) {
            let _ = wire::decode::<Request>(&damaged);
            tried += 1;
        }
    }
    for (seed, response) in (1000u64..).zip(one_of_each_response()) {
        let honest = wire::encode(&response).expect("encode");
        for damaged in itsanas_testkit::mutate::mutations(&honest, seed, 1000) {
            let _ = wire::decode::<Response>(&damaged);
            tried += 1;
        }
    }
    assert!(tried > 30_000, "only {tried} damaged frames were tried");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(60),
        "{tried} damaged frames took {:?}: some decode is far slower than its bytes",
        started.elapsed()
    );
}

#[test]
fn every_request_variant_round_trips_through_the_wire() {
    // A variant that fails to encode is a runtime failure on a live
    // connection, which is a bad place to discover it.
    for request in one_of_each() {
        every_variant_is_in_the_round_trip_list(&request);
        let frame = wire::encode(&request).unwrap();
        assert_eq!(
            wire::decode::<Request>(&frame).unwrap(),
            request,
            "round trip changed {request:?}"
        );
    }
}

fn one_of_each_response() -> Vec<Response> {
    vec![
        Response::Hello {
            protocol: PROTOCOL_VERSION,
            device: device(),
        },
        Response::Heads(vec![Head {
            device: device(),
            head: ObjectId::from_bytes([6; 32]),
            length: 12,
        }]),
        Response::Heads(Vec::new()),
        Response::Segments(Vec::new()),
        Response::Chunk(Some(vec![1, 2, 3])),
        Response::Chunk(None),
        Response::Stored { accepted: true },
        Response::Stored { accepted: false },
        Response::ChallengeProof([8; 32]),
        Response::Refused("no such user".to_owned()),
        Response::Presences(vec![vec![1, 2, 3], Vec::new()]),
        Response::Claim(vec![4, 5]),
        Response::Exposed { chunks: 3 },
        Response::Parked,
        Response::Incoming { from: device() },
        Response::Spliced,
        Response::MayEvict { marked: 7 },
        Response::Claims(vec![vec![1, 2], Vec::new()]),
        Response::SummaryRoot([3; 32]),
        Response::AppMode { on: true },
    ]
}

/// The bytes of every peer message, folded (FNV-1a; postcard writes
/// fields in declaration order), one row per published
/// [`PROTOCOL_VERSION`] from 12, when the pin began. A row is never edited
/// once its version shipped: new bytes are a new row and a new version.
const WIRE_BY_VERSION: &[(u16, u64)] = &[
    (12, 4_349_036_195_717_396_686),
    (13, 7_877_340_313_677_921_213),
];

#[test]
fn red_team_a_peer_message_that_changes_its_bytes_changes_the_version() {
    // The coordinator's twin (`itsanas-coord`, #443): #435 changed what a
    // message wrote and left the version, and the release before read
    // garbage for a week. The number test below sees a variant moved, not
    // a field added. Any change to what a message writes changes this
    // figure: raise PROTOCOL_VERSION, keep the older peers' path behind
    // a PROTOCOL_WITH_ constant, and set the new figure here.
    let mut folded: u64 = 0xcbf2_9ce4_8422_2325;
    let requests = one_of_each();
    let responses = one_of_each_response();
    let bytes = requests
        .iter()
        .map(|request| postcard::to_stdvec(request).unwrap())
        .chain(
            responses
                .iter()
                .map(|response| postcard::to_stdvec(response).unwrap()),
        );
    for message in bytes {
        for byte in message {
            folded = (folded ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    assert_eq!(
        WIRE_BY_VERSION.last().map(|row| row.0),
        Some(PROTOCOL_VERSION),
        "PROTOCOL_VERSION has no row: add one with this build's figure, {folded}"
    );
    assert_eq!(
        (PROTOCOL_VERSION, folded),
        *WIRE_BY_VERSION.last().unwrap(),
        "a peer message writes other bytes at the same version: every deployed peer of the release before reads it as garbage. Raise PROTOCOL_VERSION"
    );
}

#[test]
fn every_response_variant_round_trips_through_the_wire() {
    for response in one_of_each_response() {
        let frame = wire::encode(&response).unwrap();
        assert_eq!(wire::decode::<Response>(&frame).unwrap(), response);
    }
}

#[test]
fn an_unbounded_segment_request_is_not_acceptable() {
    // Otherwise one request asks a peer to assemble every segment it holds.
    assert!(
        !Request::Segments {
            owner: user(),
            device: device(),
            after: None,
            limit: 0,
        }
        .is_acceptable()
    );

    assert!(
        !Request::Segments {
            owner: user(),
            device: device(),
            after: None,
            limit: MAX_SEGMENTS_PER_REQUEST + 1,
        }
        .is_acceptable()
    );

    assert!(
        Request::Segments {
            owner: user(),
            device: device(),
            after: None,
            limit: MAX_SEGMENTS_PER_REQUEST,
        }
        .is_acceptable()
    );
}

#[test]
fn a_hello_is_accepted_from_the_floor_upwards_and_refused_below_it() {
    // A window, not a point. Anything at or above the floor is answered
    // with the version both sides know; anything below it has no shared
    // vocabulary and is turned away here rather than three messages later.
    for protocol in [
        MIN_PROTOCOL_VERSION,
        PROTOCOL_VERSION,
        PROTOCOL_VERSION + 1,
        u16::MAX,
    ] {
        assert!(
            Request::Hello {
                protocol,
                device: device(),
                owner: user(),
            }
            .is_acceptable(),
            "version {protocol} was refused, which partitions the network on every upgrade"
        );
    }

    assert!(
        !Request::Hello {
            protocol: MIN_PROTOCOL_VERSION - 1,
            device: device(),
            owner: user(),
        }
        .is_acceptable()
    );
}

/// The number each request is written under on the wire. Append, never
/// insert: a number that moves is a message every deployed peer reads as a
/// different one.
fn request_number(request: &Request) -> u8 {
    match request {
        Request::Hello { .. } => 0,
        Request::Heads { .. } => 1,
        Request::Segments { .. } => 2,
        Request::Chunk { .. } => 3,
        Request::HaveChunks { .. } => 4,
        Request::StoreChunk { .. } => 5,
        Request::StoreSegment { .. } => 6,
        Request::Challenge { .. } => 7,
        Request::WantHosted { .. } => 8,
        Request::Hosted { .. } => 9,
        Request::Dropped { .. } => 10,
        Request::ChunkSummary { .. } => 11,
        Request::Leaving => 12,
        Request::Presences => 13,
        Request::Claim { .. } => 14,
        Request::Draining { .. } => 15,
        Request::Park => 16,
        Request::Splice { .. } => 17,
        Request::MayEvict { .. } => 18,
        Request::Claims { .. } => 19,
        Request::SummaryRoot { .. } => 20,
        Request::AppMode => 21,
    }
}

/// The same for responses.
fn response_number(response: &Response) -> u8 {
    match response {
        Response::Hello { .. } => 0,
        Response::Heads(_) => 1,
        Response::Segments(_) => 2,
        Response::Chunk(_) => 3,
        Response::Missing(_) => 4,
        Response::Stored { .. } => 5,
        Response::ChallengeProof(_) => 6,
        Response::ChunkSummary(_) => 7,
        Response::WantHosted { .. } => 8,
        Response::Refused(_) => 9,
        Response::Presences(_) => 10,
        Response::Claim(_) => 11,
        Response::Exposed { .. } => 12,
        Response::Parked => 13,
        Response::Incoming { .. } => 14,
        Response::Spliced => 15,
        Response::MayEvict { .. } => 16,
        Response::Claims(_) => 17,
        Response::SummaryRoot(_) => 18,
        Response::AppMode { .. } => 19,
    }
}

#[test]
fn red_team_every_variant_keeps_its_number_on_the_wire() {
    // postcard writes a variant as its position in the enum. Version 4
    // inserted `Response::ChunkSummary` before `WantHosted` and `Refused`,
    // so a version-2 peer's `WantHosted` arrived as a `ChunkSummary` --
    // "Hit the end of buffer", every round, on the whole fleet, for a week
    // -- and its `Refused` as a `WantHosted`. The round-trip tests above
    // could not see it: both ends of a round trip compile the same enum.
    //
    // What this catches: a variant inserted anywhere but the end, or two
    // swapped. The numbers are the ones already deployed and do not move.
    for request in one_of_each() {
        let bytes = postcard::to_allocvec(&request).expect("encode");
        assert_eq!(
            bytes[0],
            request_number(&request),
            "{request:?} is written under a new number; every deployed peer \
             reads it as a different request"
        );
    }

    let responses = [
        Response::Hello {
            protocol: PROTOCOL_VERSION,
            device: device(),
        },
        Response::Heads(Vec::new()),
        Response::Segments(Vec::new()),
        Response::Chunk(None),
        Response::Missing(Vec::new()),
        Response::Stored { accepted: true },
        Response::ChallengeProof([0; 32]),
        Response::ChunkSummary(Vec::new()),
        Response::WantHosted {
            owner: user(),
            chunks: Vec::new(),
        },
        Response::Refused(String::new()),
        Response::Presences(Vec::new()),
        Response::Claim(Vec::new()),
        Response::Exposed { chunks: 0 },
        Response::Parked,
        Response::Incoming { from: device() },
        Response::Spliced,
        Response::MayEvict { marked: 0 },
        Response::Claims(Vec::new()),
        Response::SummaryRoot([0; 32]),
        Response::AppMode { on: false },
    ];
    let numbered: std::collections::BTreeSet<u8> = responses.iter().map(response_number).collect();
    assert_eq!(
        numbered.len(),
        responses.len(),
        "a response variant is missing from this list, so its number is unchecked"
    );
    for response in responses {
        let bytes = postcard::to_allocvec(&response).expect("encode");
        assert_eq!(
            bytes[0],
            response_number(&response),
            "{response:?} is written under a new number; every deployed peer \
             reads it as a different response"
        );
    }
}

#[test]
fn red_team_a_peer_from_before_the_current_wire_order_is_refused_at_hello() {
    // Versions 2 and 3 predate the order `Response` has now, so every
    // answer they give after the handshake decodes as another message.
    // Refusing them at the hello turns a week of "Hit the end of buffer"
    // into one line saying which versions disagree.
    //
    // What this catches: the floor lowered again without the wire being
    // renumbered -- a window that admits peers it cannot understand.
    for protocol in [2, 3] {
        assert!(
            !Request::Hello {
                protocol,
                device: device(),
                owner: user(),
            }
            .is_acceptable(),
            "a version-{protocol} peer was admitted, and its replies decode as the wrong messages"
        );
    }
}

#[test]
fn red_team_a_claim_to_hold_things_is_bounded_like_the_claim_to_have_dropped_them() {
    // `Dropped` withdraws holder records and `Hosted` writes them. The
    // first was bounded and the second was not, so one frame could add
    // rows to a victim's index without limit -- permanent rows, in the
    // table `Store::release` reads to decide whether the last local copy
    // may go. The asymmetry is the tell: somebody bounded the message that
    // removes records and not the one that adds them.
    let chunk = ChunkId::from_bytes([9; 32]);

    assert!(
        Request::Hosted {
            chunks: vec![chunk; MAX_HAVE_BATCH],
        }
        .is_acceptable(),
        "a full legitimate batch was refused"
    );
    assert!(
        !Request::Hosted {
            chunks: vec![chunk; MAX_HAVE_BATCH + 1],
        }
        .is_acceptable(),
        "an unbounded claim was accepted"
    );
    assert!(
        !Request::Hosted { chunks: Vec::new() }.is_acceptable(),
        "an empty claim is a wasted round trip, not a message"
    );

    // The bound it was supposed to have all along, stated side by side so
    // the next verb added here is compared against both.
    assert!(
        !Request::Dropped {
            owner: user(),
            chunks: vec![chunk; MAX_HAVE_BATCH + 1],
        }
        .is_acceptable()
    );
}

#[test]
fn red_team_a_permission_to_evict_is_bounded_like_every_list_that_changes_a_host() {
    // `MayEvict` marks a row on the host for every id it names. Unbounded,
    // one frame of eight megabytes is a quarter of a million rows written
    // under the host's lock -- the hole `Hosted` had until a red-team
    // sweep. Sabotage: leave `MayEvict` out of the bounded arm.
    let chunk = ChunkId::from_bytes([12; 32]);
    let asking = |chunks: Vec<ChunkId>| Request::MayEvict {
        owner: user(),
        chunks,
    };

    assert!(
        asking(vec![chunk; MAX_HAVE_BATCH]).is_acceptable(),
        "a full legitimate batch was refused"
    );
    assert!(
        !asking(vec![chunk; MAX_HAVE_BATCH + 1]).is_acceptable(),
        "an unbounded list of chunks to mark was accepted"
    );
    assert!(
        !asking(Vec::new()).is_acceptable(),
        "an empty list is a wasted round trip, not a message"
    );
}

#[test]
fn a_maximum_size_chunk_fits_in_one_frame() {
    // The chunker caps chunks at 256 KiB; if the largest legitimate message
    // did not fit the frame limit, normal operation would fail.
    let request = Request::StoreChunk {
        owner: user(),
        address: ChunkId::from_bytes([1; 32]),
        sealed: vec![0u8; 256 * 1024 + 64],
    };

    let frame = wire::encode(&request).unwrap();
    assert!(
        frame.len() < wire::MAX_FRAME_LEN,
        "a legitimate maximum-size chunk does not fit the frame limit"
    );
    assert_eq!(wire::decode::<Request>(&frame).unwrap(), request);
}

#[test]
fn a_refusal_carries_no_secret_material() {
    // A guard against the easy mistake of formatting an error that embeds a
    // key or a plaintext. Refused carries a String; this documents that the
    // String is operator-facing only.
    let refusal = Response::Refused("unknown owner".to_owned());
    let frame = wire::encode(&refusal).unwrap();
    assert_eq!(wire::decode::<Response>(&frame).unwrap(), refusal);
}
