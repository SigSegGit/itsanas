//! The announcement a node broadcasts on its local network.
//!
//! # Why a hand-written fixed layout
//!
//! This is the only structure in the project parsed from a packet that arrived
//! unsolicited, from anybody, with no handshake in front of it. Every other
//! parser in ITSaNAS sits behind TLS and behind a peer that has already proved
//! which device it is.
//!
//! So it is deliberately the dullest parser in the codebase: a **fixed 147
//! bytes**, no length field, no variable-length member, no encoder library. A
//! packet of any other size is rejected before a single field is read, and
//! nothing here allocates. There is no size for an attacker to lie about
//! because there is no size on the wire.
//!
//! # What a signature here does and does not prove
//!
//! The announcement is signed by the device key, and [`DeviceId`] *is* the
//! Ed25519 verifying key, so a receiver checks it with no key exchange and no
//! prior contact.
//!
//! That proves exactly one thing: **the sender holds the private key for the
//! device id it claims.** Nobody can advertise somebody else's device.
//!
//! It does **not** prove the claimed `owner`. Binding a device to a user needs
//! the owner-signed claim that lives in `itsanas-coord`, which a node on a bare
//! LAN has no way to obtain. The owner field is therefore a *hint* used to sort
//! candidates — try my own machines first — and never an authorisation. Acting
//! on it as though it were would be the mistake this paragraph exists to
//! prevent: the peer protocol above already treats every caller as a stranger,
//! and everything it will serve is sealed or signed.
//!
//! # The address is not in the packet
//!
//! Only the TCP port is. The address comes from the UDP source, which means a
//! node cannot advertise a *different* machine's address — a whole class of
//! redirection attacks that a self-declared address would open up.
//!
//! A replayed announcement therefore points at whoever replayed it, and the
//! TLS device pinning one layer up refuses the connection. The cost of a replay
//! is one wasted dial, and the next honest announcement corrects the entry —
//! which is why the sender's own clock is carried for diagnostics but is never
//! used to decide anything. See `neighbours` for that argument in full.

use itsanas_crypto::{DeviceId, DeviceKeys, ID_LEN, Signature, UserId, verify};

use crate::error::{DiscoverError, Result};

/// Domain separation for the announcement signature.
///
/// Distinct from every other signing domain in the project, so that a signature
/// made for one purpose can never be replayed as another.
pub const BEACON_DOMAIN: &str = "itsanas v1 local discovery beacon";

/// The first bytes of every announcement.
///
/// Not a security measure — a signature is. It exists so that a node which
/// happens to share a port with something else discards foreign traffic in one
/// comparison instead of attempting a signature check on it.
pub const MAGIC: [u8; 8] = *b"ITSaNASd";

/// The announcement format this build speaks.
pub const BEACON_VERSION: u8 = 1;

/// Total size of an announcement, in bytes. Fixed, forever, for version 1.
pub const BEACON_LEN: usize = 147;

/// Offset at which the signature begins; everything before it is signed.
const SIGNED_LEN: usize = BEACON_LEN - 64;

const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 8;
const OFF_OWNER: usize = 9;
const OFF_DEVICE: usize = OFF_OWNER + ID_LEN;
const OFF_PORT: usize = OFF_DEVICE + ID_LEN;
const OFF_TIME: usize = OFF_PORT + 2;
const OFF_SIGNATURE: usize = OFF_TIME + 8;

const _: () = assert!(OFF_SIGNATURE == SIGNED_LEN);

/// A node saying "I am here", verified.
///
/// Only constructed by [`Announcement::parse`], which refuses anything whose
/// signature does not check out, so holding one of these means the signature
/// was valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Announcement {
    /// The user the sender claims to belong to. A hint, never an authorisation.
    pub owner: UserId,
    /// The sender's device, proved by the signature.
    pub device: DeviceId,
    /// The TCP port the sender is serving the peer protocol on.
    pub port: u16,
    /// The sender's clock when it signed, in seconds since the Unix epoch.
    pub sent_unix: u64,
}

impl Announcement {
    /// Build and sign an announcement, ready to put on the wire.
    #[must_use]
    pub fn seal(keys: &DeviceKeys, owner: UserId, port: u16, now_unix: u64) -> [u8; BEACON_LEN] {
        let mut packet = [0u8; BEACON_LEN];
        packet[OFF_MAGIC..OFF_VERSION].copy_from_slice(&MAGIC);
        packet[OFF_VERSION] = BEACON_VERSION;
        packet[OFF_OWNER..OFF_DEVICE].copy_from_slice(owner.as_bytes());
        packet[OFF_DEVICE..OFF_PORT].copy_from_slice(keys.device_id().as_bytes());
        packet[OFF_PORT..OFF_TIME].copy_from_slice(&port.to_be_bytes());
        packet[OFF_TIME..OFF_SIGNATURE].copy_from_slice(&now_unix.to_be_bytes());

        let signature = keys.sign(BEACON_DOMAIN, &packet[..SIGNED_LEN]);
        packet[OFF_SIGNATURE..].copy_from_slice(&signature.to_bytes());
        packet
    }

    /// Parse and verify a packet.
    ///
    /// Every rejection happens before any work that depends on the contents:
    /// the length is checked first, then the magic, then the version, and the
    /// signature last. A caller can therefore hand this arbitrary bytes from
    /// the network at any rate without it costing more than a memcmp.
    pub fn parse(packet: &[u8]) -> Result<Self> {
        if packet.len() != BEACON_LEN {
            return Err(DiscoverError::WrongLength {
                got: packet.len(),
                expected: BEACON_LEN,
            });
        }
        if packet[OFF_MAGIC..OFF_VERSION] != MAGIC {
            return Err(DiscoverError::NotOurs);
        }
        let version = packet[OFF_VERSION];
        if version != BEACON_VERSION {
            return Err(DiscoverError::UnknownVersion { got: version });
        }

        let mut owner = [0u8; ID_LEN];
        owner.copy_from_slice(&packet[OFF_OWNER..OFF_DEVICE]);
        let mut device = [0u8; ID_LEN];
        device.copy_from_slice(&packet[OFF_DEVICE..OFF_PORT]);

        let mut signature = [0u8; 64];
        signature.copy_from_slice(&packet[OFF_SIGNATURE..]);

        // The device id is the verifying key, so this needs no prior contact
        // and no key distribution. It proves the sender holds that device's
        // key and nothing whatsoever about the owner field above it.
        verify(
            &device,
            BEACON_DOMAIN,
            &packet[..SIGNED_LEN],
            Signature::from_bytes(signature),
        )
        .map_err(|_| DiscoverError::BadSignature)?;

        let port = u16::from_be_bytes([packet[OFF_PORT], packet[OFF_PORT + 1]]);
        if port == 0 {
            return Err(DiscoverError::NoPort);
        }

        let mut time = [0u8; 8];
        time.copy_from_slice(&packet[OFF_TIME..OFF_SIGNATURE]);

        Ok(Self {
            owner: UserId::from_bytes(owner),
            device: DeviceId::from_bytes(device),
            port,
            sent_unix: u64::from_be_bytes(time),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> DeviceKeys {
        DeviceKeys::generate().unwrap()
    }

    fn owner() -> UserId {
        UserId::from_bytes([7u8; ID_LEN])
    }

    #[test]
    fn an_announcement_round_trips() {
        let k = keys();
        let packet = Announcement::seal(&k, owner(), 9797, 1_700_000_000);
        let parsed = Announcement::parse(&packet).unwrap();

        assert_eq!(parsed.device, k.device_id());
        assert_eq!(parsed.owner, owner());
        assert_eq!(parsed.port, 9797);
        assert_eq!(parsed.sent_unix, 1_700_000_000);
    }

    #[test]
    fn the_layout_is_exactly_as_documented() {
        // The wire format is a compatibility commitment. If this fails, an
        // older build on another machine stops being able to find this one,
        // and the symptom is "discovery silently does nothing".
        let packet = Announcement::seal(&keys(), owner(), 0x1234, 0x0102_0304_0506_0708);
        assert_eq!(packet.len(), 147);
        assert_eq!(&packet[0..8], b"ITSaNASd");
        assert_eq!(packet[8], 1);
        assert_eq!(&packet[73..75], &[0x12, 0x34]);
        assert_eq!(&packet[75..83], &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn a_device_cannot_advertise_a_device_it_does_not_own() {
        // The whole point of signing a beacon. Without this, anyone on the
        // network claims to be the Raspberry Pi and every node dials them.
        let honest = keys();
        let attacker = keys();

        let mut packet = Announcement::seal(&attacker, owner(), 9797, 1_700_000_000);
        packet[OFF_DEVICE..OFF_PORT].copy_from_slice(honest.device_id().as_bytes());

        assert!(matches!(
            Announcement::parse(&packet),
            Err(DiscoverError::BadSignature)
        ));
    }

    #[test]
    fn corrupting_any_single_byte_is_refused_and_never_panics() {
        // Arrives unsolicited from anybody. It may be rejected; it may not
        // take the process down or accept a mutated field.
        let k = keys();
        let good = Announcement::seal(&k, owner(), 9797, 1_700_000_000);

        for index in 0..BEACON_LEN {
            for bit in 0..8u8 {
                let mut packet = good;
                packet[index] ^= 1 << bit;
                if packet == good {
                    continue;
                }
                assert!(
                    Announcement::parse(&packet).is_err(),
                    "byte {index} bit {bit} was accepted after corruption"
                );
            }
        }
    }

    #[test]
    fn every_truncation_and_extension_is_refused_before_anything_is_read() {
        let good = Announcement::seal(&keys(), owner(), 9797, 1);

        for len in 0..BEACON_LEN {
            assert!(matches!(
                Announcement::parse(&good[..len]),
                Err(DiscoverError::WrongLength { .. })
            ));
        }

        let mut long = good.to_vec();
        long.push(0);
        assert!(matches!(
            Announcement::parse(&long),
            Err(DiscoverError::WrongLength { .. })
        ));
    }

    #[test]
    fn arbitrary_garbage_never_panics() {
        // Anything at all may arrive on a UDP port, including another
        // protocol's traffic on a machine that reuses the number.
        for seed in 0u16..2000 {
            let mut junk = vec![0u8; usize::from(seed % 300)];
            for (index, byte) in junk.iter_mut().enumerate() {
                *byte = u8::try_from(
                    usize::from(seed)
                        .wrapping_mul(index + 1)
                        .wrapping_add(index)
                        % 256,
                )
                .unwrap_or(0);
            }
            let _ = Announcement::parse(&junk);
        }
    }

    #[test]
    fn foreign_traffic_is_discarded_on_the_magic_rather_than_the_signature() {
        let mut packet = Announcement::seal(&keys(), owner(), 9797, 1);
        packet[0] = b'X';
        assert!(matches!(
            Announcement::parse(&packet),
            Err(DiscoverError::NotOurs)
        ));
    }

    #[test]
    fn an_unknown_version_is_refused_not_guessed_at() {
        // No optimistic reinterpretation of a future format. A version 2
        // announcement may mean something entirely different at these offsets.
        let mut packet = Announcement::seal(&keys(), owner(), 9797, 1);
        packet[OFF_VERSION] = 2;
        assert!(matches!(
            Announcement::parse(&packet),
            Err(DiscoverError::UnknownVersion { got: 2 })
        ));
    }

    #[test]
    fn a_zero_port_is_refused() {
        // Nothing listens on port zero, so an announcement carrying it is
        // either a bug or bait for a connection attempt that cannot succeed.
        let k = keys();
        let packet = Announcement::seal(&k, owner(), 0, 1);
        assert!(matches!(
            Announcement::parse(&packet),
            Err(DiscoverError::NoPort)
        ));
    }

    #[test]
    fn a_signature_from_another_domain_does_not_verify_here() {
        // Domain separation, checked rather than assumed: a signature the
        // device made for the peer protocol must not be replayable as a
        // presence announcement.
        let k = keys();
        let mut packet = Announcement::seal(&k, owner(), 9797, 1);
        let elsewhere = k.sign("itsanas v1 something else entirely", &packet[..SIGNED_LEN]);
        packet[OFF_SIGNATURE..].copy_from_slice(&elsewhere.to_bytes());

        assert!(matches!(
            Announcement::parse(&packet),
            Err(DiscoverError::BadSignature)
        ));
    }

    #[test]
    fn an_ancient_clock_still_produces_a_valid_announcement() {
        // A Raspberry Pi with no RTC announces itself believing it is 1970.
        // It must still be findable, or a machine that has just come back is
        // invisible for exactly as long as it takes NTP to run.
        let k = keys();
        let packet = Announcement::seal(&k, owner(), 9797, 0);
        let parsed = Announcement::parse(&packet).unwrap();
        assert_eq!(parsed.sent_unix, 0);
        assert_eq!(parsed.device, k.device_id());
    }
}
