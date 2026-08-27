//! Framing for the peer protocol.
//!
//! Every byte this module parses comes from a stranger. Not "an untrusted user
//! of our service" — an actual other person's computer, which we have invited
//! to send us things, and which may be running modified software specifically
//! to break us. The decoder is therefore written to be boring: fixed header,
//! explicit length, hard ceiling, no recursion, and no allocation sized by a
//! number the peer chose until that number has been checked.
//!
//! # Frame layout
//!
//! ```text
//! ┌────────┬────────────┬──────────────────┐
//! │ 1 byte │  4 bytes   │  length bytes    │
//! │ version│ length, LE │  postcard payload│
//! └────────┴────────────┴──────────────────┘
//! ```
//!
//! # The size ceiling is load-bearing
//!
//! Without it, `length = 0xFFFF_FFFF` is a four-byte message that asks us to
//! allocate four gigabytes. With a few connections that is a trivial way to
//! kill a Raspberry Pi. [`MAX_FRAME_LEN`] is checked *before* any allocation,
//! and is generous enough for the largest legitimate message — a maximum-size
//! chunk plus sealing overhead, or a log segment carrying a batch of entries.

use serde::{Deserialize, Serialize};

use crate::error::{NetError, Result};

/// Wire format version. Bumped when the framing itself changes, not when a
/// message variant is added.
pub const WIRE_VERSION: u8 = 1;

/// Bytes of header before the payload.
pub const HEADER_LEN: usize = 1 + 4;

/// Largest payload this build will accept, in bytes.
///
/// Chunks are capped at 256 KiB by the chunker, and a log segment holds a batch
/// of entries whose size is bounded by how many writes a device makes between
/// flushes. 8 MiB leaves generous headroom for both while keeping a hostile
/// peer's maximum single allocation small enough to be irrelevant on a Pi.
pub const MAX_FRAME_LEN: usize = 8 * 1024 * 1024;

/// Encode a message into a length-prefixed frame.
pub fn encode<T: Serialize>(message: &T) -> Result<Vec<u8>> {
    let payload = postcard::to_stdvec(message)?;

    if payload.len() > MAX_FRAME_LEN {
        return Err(NetError::FrameTooLarge {
            len: payload.len(),
            max: MAX_FRAME_LEN,
        });
    }

    let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
    frame.push(WIRE_VERSION);
    frame.extend_from_slice(
        &u32::try_from(payload.len())
            .expect("checked against MAX_FRAME_LEN above")
            .to_le_bytes(),
    );
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// How much payload a frame header promises, or why the header is unacceptable.
///
/// Separated from decoding so a reader can decide whether to accept a frame
/// *before* reserving space for it.
pub fn payload_len(header: &[u8]) -> Result<usize> {
    if header.len() < HEADER_LEN {
        return Err(NetError::Truncated);
    }

    match header[0] {
        WIRE_VERSION => {}
        found => {
            return Err(NetError::UnsupportedWireVersion {
                found,
                supported: WIRE_VERSION,
            });
        }
    }

    let len = u32::from_le_bytes(header[1..HEADER_LEN].try_into().expect("4 bytes")) as usize;

    if len > MAX_FRAME_LEN {
        return Err(NetError::FrameTooLarge {
            len,
            max: MAX_FRAME_LEN,
        });
    }

    Ok(len)
}

/// Decode one complete frame.
pub fn decode<T: for<'de> Deserialize<'de>>(frame: &[u8]) -> Result<T> {
    let len = payload_len(frame)?;

    let payload = frame
        .get(HEADER_LEN..HEADER_LEN + len)
        .ok_or(NetError::Truncated)?;

    Ok(postcard::from_bytes(payload)?)
}

/// Accumulates bytes from a stream and yields whole frames.
///
/// A stream gives no message boundaries, so something has to hold partial
/// frames between reads. Doing that carelessly is the usual way a protocol
/// implementation acquires an unbounded memory bug: the buffer here can never
/// exceed one maximum frame, because the length is checked as soon as the
/// header is complete and an over-long frame is an error rather than something
/// to keep buffering towards.
#[derive(Debug, Default)]
pub struct FrameReader {
    buffer: Vec<u8>,
}

impl FrameReader {
    #[must_use]
    pub const fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    /// Add freshly read bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// How many bytes are held pending a complete frame.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// Take the next complete frame's payload, if one has arrived.
    ///
    /// Returns `Ok(None)` when more bytes are needed. An error means the peer
    /// sent something unacceptable and the connection should be dropped — the
    /// reader cannot resynchronise, because there is no framing marker to
    /// resynchronise *to*, and pretending otherwise would let a peer steer the
    /// parser.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>> {
        if self.buffer.len() < HEADER_LEN {
            return Ok(None);
        }

        let len = payload_len(&self.buffer)?;
        let total = HEADER_LEN + len;

        if self.buffer.len() < total {
            return Ok(None);
        }

        let payload = self.buffer[HEADER_LEN..total].to_vec();
        self.buffer.drain(..total);
        Ok(Some(payload))
    }

    /// Take the next complete frame and decode it.
    pub fn next_message<T: for<'de> Deserialize<'de>>(&mut self) -> Result<Option<T>> {
        match self.next_frame()? {
            Some(payload) => Ok(Some(postcard::from_bytes(&payload)?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Sample {
        number: u64,
        text: String,
        blob: Vec<u8>,
    }

    fn sample() -> Sample {
        Sample {
            number: 42,
            text: "hello".to_owned(),
            blob: vec![1, 2, 3, 4, 5],
        }
    }

    #[test]
    fn a_frame_round_trips() {
        let frame = encode(&sample()).unwrap();
        assert_eq!(decode::<Sample>(&frame).unwrap(), sample());
    }

    #[test]
    fn the_header_is_exactly_as_documented() {
        let frame = encode(&sample()).unwrap();
        assert_eq!(frame[0], WIRE_VERSION);

        let declared = u32::from_le_bytes(frame[1..5].try_into().unwrap()) as usize;
        assert_eq!(
            declared,
            frame.len() - HEADER_LEN,
            "the declared length does not match the actual payload"
        );
    }

    #[test]
    fn an_unknown_wire_version_is_refused_not_guessed_at() {
        let mut frame = encode(&sample()).unwrap();
        frame[0] = 99;

        assert!(matches!(
            decode::<Sample>(&frame),
            Err(NetError::UnsupportedWireVersion { found: 99, .. })
        ));
    }

    #[test]
    fn an_oversized_length_is_rejected_before_anything_is_allocated() {
        // The attack: five bytes on the wire asking the peer to reserve four
        // gigabytes. On a Raspberry Pi a handful of these is fatal.
        let mut header = vec![WIRE_VERSION];
        header.extend_from_slice(&u32::MAX.to_le_bytes());

        match payload_len(&header) {
            Err(NetError::FrameTooLarge { len, max }) => {
                assert_eq!(len, u32::MAX as usize);
                assert_eq!(max, MAX_FRAME_LEN);
            }
            other => panic!("a 4 GiB frame was accepted: {other:?}"),
        }

        let mut reader = FrameReader::new();
        reader.push(&header);
        assert!(reader.next_frame().is_err());
    }

    #[test]
    fn a_frame_exactly_at_the_limit_is_accepted_and_one_byte_over_is_not() {
        let mut at_limit = vec![WIRE_VERSION];
        at_limit.extend_from_slice(&u32::try_from(MAX_FRAME_LEN).unwrap().to_le_bytes());
        assert_eq!(payload_len(&at_limit).unwrap(), MAX_FRAME_LEN);

        let mut over = vec![WIRE_VERSION];
        over.extend_from_slice(&u32::try_from(MAX_FRAME_LEN + 1).unwrap().to_le_bytes());
        assert!(payload_len(&over).is_err());
    }

    #[test]
    fn every_truncation_of_a_valid_frame_is_an_error_and_never_a_panic() {
        let frame = encode(&sample()).unwrap();
        for cut in 0..frame.len() {
            let result = decode::<Sample>(&frame[..cut]);
            assert!(
                result.is_err(),
                "a frame truncated to {cut} bytes decoded successfully"
            );
        }
    }

    #[test]
    fn corrupting_any_byte_never_panics() {
        // A peer controls every one of these bytes. The decoder is allowed to
        // reject them; it is not allowed to abort the process.
        let frame = encode(&sample()).unwrap();

        for index in 0..frame.len() {
            for bit in 0..8u8 {
                let mut corrupted = frame.clone();
                corrupted[index] ^= 1 << bit;
                // Must return, either way. A panic here fails the test.
                let _ = decode::<Sample>(&corrupted);
            }
        }
    }

    #[test]
    fn arbitrary_garbage_never_panics() {
        let mut reader = FrameReader::new();

        for seed in 0..64u64 {
            let noise = blake3::hash(&seed.to_le_bytes());
            let _ = decode::<Sample>(noise.as_bytes());
            reader.push(noise.as_bytes());
            // Errors are fine; panics are not. Reset after a rejection, as a
            // real connection would be dropped.
            if reader.next_frame().is_err() {
                reader = FrameReader::new();
            }
        }
    }

    #[test]
    fn a_frame_split_across_reads_is_reassembled() {
        // The normal case on a real stream: bytes arrive in whatever chunks the
        // network felt like.
        let frame = encode(&sample()).unwrap();
        let mut reader = FrameReader::new();

        for byte in &frame[..frame.len() - 1] {
            reader.push(&[*byte]);
            assert_eq!(
                reader.next_frame().unwrap(),
                None,
                "a partial frame was returned as complete"
            );
        }

        reader.push(&[frame[frame.len() - 1]]);
        let payload = reader.next_frame().unwrap().expect("the frame completed");
        assert_eq!(postcard::from_bytes::<Sample>(&payload).unwrap(), sample());
    }

    #[test]
    fn several_frames_in_one_read_are_all_returned() {
        let mut stream = Vec::new();
        for number in 0..5u64 {
            stream.extend_from_slice(
                &encode(&Sample {
                    number,
                    text: format!("message {number}"),
                    blob: vec![u8::try_from(number).unwrap(); 10],
                })
                .unwrap(),
            );
        }

        let mut reader = FrameReader::new();
        reader.push(&stream);

        for expected in 0..5u64 {
            let message: Sample = reader
                .next_message()
                .unwrap()
                .expect("five frames were written");
            assert_eq!(message.number, expected);
        }

        assert_eq!(reader.next_message::<Sample>().unwrap(), None);
        assert_eq!(reader.buffered(), 0, "bytes were left over");
    }

    #[test]
    fn the_reader_does_not_grow_without_bound_on_a_stalled_frame() {
        // A peer that sends a header and then trickles bytes forever must not be
        // able to make the buffer exceed one maximum frame.
        let mut reader = FrameReader::new();
        let mut header = vec![WIRE_VERSION];
        header.extend_from_slice(&1000u32.to_le_bytes());
        reader.push(&header);

        reader.push(&vec![0u8; 999]);
        assert_eq!(reader.next_frame().unwrap(), None);
        assert!(reader.buffered() <= HEADER_LEN + MAX_FRAME_LEN);

        reader.push(&[0u8]);
        assert!(reader.next_frame().unwrap().is_some());
        assert_eq!(reader.buffered(), 0);
    }

    #[test]
    fn an_empty_payload_is_a_valid_frame() {
        #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
        struct Empty;

        let frame = encode(&Empty).unwrap();
        assert_eq!(decode::<Empty>(&frame).unwrap(), Empty);
    }
}
