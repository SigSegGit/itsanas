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
//! It does **not** prove the owner tag it carries. Binding a device to a user
//! needs the owner-signed claim that lives in `itsanas-coord`, which a node on a
//! bare LAN has no way to obtain. The tag is a *hint* used to sort candidates —
//! try my own machines first — and never an authorisation. Acting on it as
//! though it were would be the mistake this paragraph exists to prevent: the
//! peer protocol above already treats every caller as a stranger, and everything
//! it will serve is sealed or signed.
//!
//! # The owner is not sent in the clear, and not linkable either
//!
//! Version 2 carries, in the 32 bytes where the owner goes, a fresh random
//! 16-byte nonce and a 16-byte `BLAKE3_keyed(household key, nonce ‖ device)`.
//! The household key is derived from the account's master secret
//! ([`itsanas_crypto::UserKeys::lan_tag_key`]), so only the account's own
//! machines can compute or check the tag. It is never on the wire.
//!
//! That buys three things. A user id is a public key, so broadcasting it would
//! tell every café and hotel whose machine this is. Two beacons of one account
//! -- from one machine or two -- carry unrelated tags, so a listener cannot
//! group an account's machines, which the version 1 tag (a hash of the user id,
//! the same 32 bytes from every machine for ever) let anyone do; and anyone
//! holding the user id recognised it. And the device is inside the hash, so a
//! tag lifted from a household member's beacon onto another device's beacon is
//! not recognised: a stranger cannot buy a place at the front of the dial order
//! by copying what they heard.
//!
//! **No clock is in the tag.** A Raspberry Pi 4 has no real-time clock and
//! boots believing it is 1970; a tag rotated on a clock would make its own
//! household stop recognising it exactly when it came back.
//!
//! **What it does not stop.** A whole v2 beacon replayed from another address
//! still reads as "mine": the tag checks out because the device and nonce are
//! the ones it was made for. That costs one wasted dial, and the TLS device
//! pinning one layer up refuses it, as for any replay (below). And the device id
//! is the Ed25519 verifying key and has to travel in the clear, or nobody could
//! check the signature without already knowing the device, so an observer on
//! two networks can still tell it is the same machine. They cannot tell whose,
//! and they cannot tell that two machines belong together.
//!
//! # Version 1 is still heard
//!
//! A household upgrades one machine at a time. A listener that refused version
//! 1 would make an upgraded machine and a not-yet-upgraded one blind to each
//! other on the LAN until the last one updated. So [`Announcement::parse`]
//! accepts versions 1 and 2, and a version 1 beacon comes back with its old
//! tag as [`OwnerTag::Legacy`]: its device and port are learned and dialled, and
//! it is never counted as one of this account's machines, because an unkeyed
//! tag is exactly what anybody can forge. A mixed fleet therefore loses LAN
//! *grouping* (the old machine is dialled among strangers, rationed per round
//! until it has once served this account's own work, and its silence is not
//! reported as one of yours), not LAN *discovery*; the coordinator path does
//! not read the tag at all. The other direction is not in this build's hands:
//! a version 1 build refuses version 2 (its log counts the packets as failing
//! to verify), so the old machine hears nobody, and is found because the
//! upgraded machines dial it. [`Announcement::seal`] writes version 2 only.
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

#[cfg(test)]
use itsanas_crypto::UserId;
use itsanas_crypto::{DeviceId, DeviceKeys, ID_LEN, SecretBytes, Signature, UserKeys, verify};

use crate::error::{DiscoverError, Result};

/// Domain separation for the announcement signature.
///
/// Distinct from every other signing domain in the project, so that a signature
/// made for one purpose can never be replayed as another. Shared by both
/// versions: the version byte is inside the signed bytes, so a version 1
/// signature cannot be passed off as version 2 or the other way round.
pub const BEACON_DOMAIN: &str = "itsanas v1 local discovery beacon";

/// Domain of the version 1 owner tag, `derive_key(this, user id)`.
///
/// Kept only to document what a not-yet-upgraded machine sends. Nothing in
/// this build computes it outside the tests that play such a machine.
pub const OWNER_TAG_DOMAIN: &str = "itsanas v1 local discovery owner tag";

/// Bytes of fresh randomness in a version 2 tag.
pub const NONCE_LEN: usize = 16;

/// Bytes of keyed hash in a version 2 tag.
///
/// Sixteen, not thirty-two, so that nonce and hash fit the 32-byte field
/// version 1 used and the packet keeps its size. A forger guessing a tag has a
/// 2^-128 chance per beacon, and a right guess buys one earlier dial.
pub const MAC_LEN: usize = 16;

const _: () = assert!(NONCE_LEN + MAC_LEN == ID_LEN);

/// The account's key for making and recognising its machines' tags.
///
/// Held only by machines that hold the account's master secret. Zeroized on
/// drop, and its `Debug` says nothing, like every other secret here.
#[derive(Clone, Debug)]
pub struct HouseholdKey(itsanas_crypto::SymmetricKey);

impl HouseholdKey {
    /// The key of the account these keys belong to.
    #[must_use]
    pub fn of(user: &UserKeys) -> Self {
        Self(user.lan_tag_key())
    }

    /// The keyed hash for one nonce and one device.
    ///
    /// The device is in the input so that a tag copied from one device's
    /// beacon onto another's fails: without it, anybody who heard one of your
    /// beacons could claim to be one of your machines.
    fn mac(&self, nonce: &[u8; NONCE_LEN], device: &DeviceId) -> [u8; MAC_LEN] {
        let full = blake3::Hasher::new_keyed(self.0.expose())
            .update(nonce)
            .update(device.as_bytes())
            .finalize();
        let mut out = [0u8; MAC_LEN];
        out.copy_from_slice(&full.as_bytes()[..MAC_LEN]);
        out
    }

    #[cfg(test)]
    pub(crate) fn secret_bytes(&self) -> [u8; 32] {
        *self.0.expose()
    }
}

/// What an announcement says about its owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerTag {
    /// Version 2: a fresh nonce and a keyed hash only the owner's machines can
    /// check.
    Keyed {
        /// Random per beacon, so two beacons of one account differ.
        nonce: [u8; NONCE_LEN],
        /// `BLAKE3_keyed(household key, nonce ‖ device)`, truncated.
        mac: [u8; MAC_LEN],
    },
    /// Version 1: `derive_key(OWNER_TAG_DOMAIN, user id)`, from a machine that
    /// has not been upgraded. Never read as one of this account's machines:
    /// anybody holding a user id computes it.
    Legacy([u8; ID_LEN]),
}

impl OwnerTag {
    /// Whether this tag was made by a machine holding `household`, for `device`.
    ///
    /// Unverified in the sense that matters: a replayed beacon passes. It
    /// orders candidates and authorises nothing.
    #[must_use]
    pub fn is_mine(&self, household: &HouseholdKey, device: &DeviceId) -> bool {
        match self {
            Self::Keyed { nonce, mac } => {
                let expected = household.mac(nonce, device);
                // Constant time out of habit; the tag is a hint, not a secret.
                expected
                    .iter()
                    .zip(mac.iter())
                    .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                    == 0
            }
            Self::Legacy(_) => false,
        }
    }

    /// Whether it came from a machine still speaking version 1.
    #[must_use]
    pub const fn is_legacy(&self) -> bool {
        matches!(self, Self::Legacy(_))
    }
}

/// The version 1 tag of a user id, as a not-yet-upgraded machine sends it.
#[cfg(test)]
pub(crate) fn legacy_owner_tag(owner: UserId) -> [u8; ID_LEN] {
    *blake3::Hasher::new_derive_key(OWNER_TAG_DOMAIN)
        .update(owner.as_bytes())
        .finalize()
        .as_bytes()
}

/// The first bytes of every announcement.
///
/// Not a security measure — a signature is. It exists so that a node which
/// happens to share a port with something else discards foreign traffic in one
/// comparison instead of attempting a signature check on it.
pub const MAGIC: [u8; 8] = *b"ITSaNASd";

/// The announcement format this build writes.
pub const BEACON_VERSION: u8 = 2;

/// The oldest format this build still reads. See "Version 1 is still heard".
pub const OLDEST_READ_VERSION: u8 = 1;

/// Total size of an announcement, in bytes. The same 147 for versions 1 and 2.
pub const BEACON_LEN: usize = 147;

/// Offset at which the signature begins; everything before it is signed.
const SIGNED_LEN: usize = BEACON_LEN - 64;

const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 8;
const OFF_OWNER: usize = 9;
const OFF_MAC: usize = OFF_OWNER + NONCE_LEN;
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
    /// Who the sender claims to belong to. Ask [`Announcement::is_mine`].
    pub owner_tag: OwnerTag,
    /// The sender's device, proved by the signature.
    pub device: DeviceId,
    /// The TCP port the sender is serving the peer protocol on.
    pub port: u16,
    /// The sender's clock when it signed, in seconds since the Unix epoch.
    pub sent_unix: u64,
}

impl Announcement {
    /// Build and sign a version 2 announcement, ready to put on the wire.
    ///
    /// # Errors
    ///
    /// [`DiscoverError::Randomness`] if the operating system refuses a nonce.
    /// Sending a fixed one instead would make every beacon of this machine
    /// carry the same tag, which is the linkability this format exists to end.
    pub fn seal(
        keys: &DeviceKeys,
        household: &HouseholdKey,
        port: u16,
        now_unix: u64,
    ) -> Result<[u8; BEACON_LEN]> {
        let nonce = SecretBytes::<NONCE_LEN>::random()?;
        Ok(Self::seal_with_nonce(
            keys,
            household,
            nonce.expose(),
            port,
            now_unix,
        ))
    }

    fn seal_with_nonce(
        keys: &DeviceKeys,
        household: &HouseholdKey,
        nonce: &[u8; NONCE_LEN],
        port: u16,
        now_unix: u64,
    ) -> [u8; BEACON_LEN] {
        let device = keys.device_id();
        let mut owner = [0u8; ID_LEN];
        owner[..NONCE_LEN].copy_from_slice(nonce);
        owner[NONCE_LEN..].copy_from_slice(&household.mac(nonce, &device));
        Self::sign(keys, BEACON_VERSION, &owner, port, now_unix)
    }

    /// A version 1 beacon exactly as a not-yet-upgraded build sends it.
    #[cfg(test)]
    pub(crate) fn seal_v1(
        keys: &DeviceKeys,
        owner: UserId,
        port: u16,
        now_unix: u64,
    ) -> [u8; BEACON_LEN] {
        Self::sign(keys, 1, &legacy_owner_tag(owner), port, now_unix)
    }

    pub(crate) fn sign(
        keys: &DeviceKeys,
        version: u8,
        owner: &[u8; ID_LEN],
        port: u16,
        now_unix: u64,
    ) -> [u8; BEACON_LEN] {
        let mut packet = [0u8; BEACON_LEN];
        packet[OFF_MAGIC..OFF_VERSION].copy_from_slice(&MAGIC);
        packet[OFF_VERSION] = version;
        packet[OFF_OWNER..OFF_DEVICE].copy_from_slice(owner);
        packet[OFF_DEVICE..OFF_PORT].copy_from_slice(keys.device_id().as_bytes());
        packet[OFF_PORT..OFF_TIME].copy_from_slice(&port.to_be_bytes());
        packet[OFF_TIME..OFF_SIGNATURE].copy_from_slice(&now_unix.to_be_bytes());

        let signature = keys.sign(BEACON_DOMAIN, &packet[..SIGNED_LEN]);
        packet[OFF_SIGNATURE..].copy_from_slice(&signature.to_bytes());
        packet
    }

    /// Whether this announcement was made by one of `household`'s machines.
    #[must_use]
    pub fn is_mine(&self, household: &HouseholdKey) -> bool {
        self.owner_tag.is_mine(household, &self.device)
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
        if !(OLDEST_READ_VERSION..=BEACON_VERSION).contains(&version) {
            return Err(DiscoverError::UnknownVersion { got: version });
        }

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

        let owner_tag = if version == 1 {
            let mut tag = [0u8; ID_LEN];
            tag.copy_from_slice(&packet[OFF_OWNER..OFF_DEVICE]);
            OwnerTag::Legacy(tag)
        } else {
            let mut nonce = [0u8; NONCE_LEN];
            nonce.copy_from_slice(&packet[OFF_OWNER..OFF_MAC]);
            let mut mac = [0u8; MAC_LEN];
            mac.copy_from_slice(&packet[OFF_MAC..OFF_DEVICE]);
            OwnerTag::Keyed { nonce, mac }
        };

        let mut time = [0u8; 8];
        time.copy_from_slice(&packet[OFF_TIME..OFF_SIGNATURE]);

        Ok(Self {
            owner_tag,
            device: DeviceId::from_bytes(device),
            port,
            sent_unix: u64::from_be_bytes(time),
        })
    }
}

#[cfg(test)]
mod tests {
    use itsanas_crypto::MasterSecret;

    use super::*;

    fn keys() -> DeviceKeys {
        DeviceKeys::generate().unwrap()
    }

    fn account(seed: u8) -> UserKeys {
        UserKeys::derive(&MasterSecret::from_bytes([seed; 32]))
    }

    fn household() -> HouseholdKey {
        HouseholdKey::of(&account(7))
    }

    fn seal(keys: &DeviceKeys, household: &HouseholdKey, port: u16, now: u64) -> [u8; BEACON_LEN] {
        Announcement::seal(keys, household, port, now).unwrap()
    }

    #[test]
    fn an_announcement_round_trips() {
        let k = keys();
        let packet = seal(&k, &household(), 9797, 1_700_000_000);
        let parsed = Announcement::parse(&packet).unwrap();

        assert_eq!(parsed.device, k.device_id());
        assert!(parsed.is_mine(&household()));
        assert_eq!(parsed.port, 9797);
        assert_eq!(parsed.sent_unix, 1_700_000_000);
    }

    #[test]
    fn the_layout_is_exactly_as_documented() {
        // The wire format is a compatibility commitment. If this fails, an
        // older build on another machine stops being able to find this one,
        // and the symptom is "discovery silently does nothing". Version 2
        // changed byte 8 and the meaning of bytes 9..41 on purpose, and kept
        // the size and every other offset, so a version 1 parser rejects it on
        // the version byte and nowhere else.
        let k = keys();
        let packet = seal(&k, &household(), 0x1234, 0x0102_0304_0506_0708);
        assert_eq!(packet.len(), 147);
        assert_eq!(&packet[0..8], b"ITSaNASd");
        assert_eq!(packet[8], 2);
        assert_eq!(&packet[41..73], k.device_id().as_bytes());
        assert_eq!(&packet[73..75], &[0x12, 0x34]);
        assert_eq!(&packet[75..83], &[1, 2, 3, 4, 5, 6, 7, 8]);

        let v1 = Announcement::seal_v1(&k, account(7).user_id(), 0x1234, 1);
        assert_eq!(v1.len(), packet.len());
        assert_eq!(v1[8], 1);
    }

    #[test]
    fn a_device_cannot_advertise_a_device_it_does_not_own() {
        // The whole point of signing a beacon. Without this, anyone on the
        // network claims to be the Raspberry Pi and every node dials them.
        let honest = keys();
        let attacker = keys();

        let mut packet = seal(&attacker, &household(), 9797, 1_700_000_000);
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
        for good in [
            seal(&k, &household(), 9797, 1_700_000_000),
            Announcement::seal_v1(&k, account(7).user_id(), 9797, 1_700_000_000),
        ] {
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
    }

    #[test]
    fn every_truncation_and_extension_is_refused_before_anything_is_read() {
        let good = seal(&keys(), &household(), 9797, 1);

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
        let mut packet = seal(&keys(), &household(), 9797, 1);
        packet[0] = b'X';
        assert!(matches!(
            Announcement::parse(&packet),
            Err(DiscoverError::NotOurs)
        ));
    }

    #[test]
    fn an_unknown_version_is_refused_not_guessed_at() {
        // No optimistic reinterpretation of a future format. A version 3
        // announcement may mean something entirely different at these offsets,
        // and version 0 was never written by anything.
        for version in [0u8, 3, 255] {
            let mut packet = seal(&keys(), &household(), 9797, 1);
            packet[OFF_VERSION] = version;
            assert!(matches!(
                Announcement::parse(&packet),
                Err(DiscoverError::UnknownVersion { got }) if got == version
            ));
        }
    }

    #[test]
    fn red_team_a_version_1_beacon_is_still_heard_and_never_counted_as_mine() {
        // THE FAILURE: a household upgrades the laptop first. If the laptop
        // refused version 1, it and the not-yet-upgraded Pi would be blind to
        // each other on the LAN until the Pi updated -- discovery silently
        // gone, exactly during the upgrade.
        //
        // THE ATTACK on the other side: a version 1 tag is a hash of the user
        // id, which anybody holding the id computes. Reading it as "mine"
        // would hand any such stranger a place at the front of the dial order
        // by the simple act of speaking the old format.
        //
        // If this fails, either an upgrade splits the household, or the old
        // format is a downgrade path back to the forgeable tag.
        let pi = keys();
        let me = account(7);
        let heard = Announcement::parse(&Announcement::seal_v1(&pi, me.user_id(), 9797, 0))
            .expect("a version 1 beacon must still be read");

        assert_eq!(heard.device, pi.device_id());
        assert_eq!(heard.port, 9797);
        assert!(heard.owner_tag.is_legacy());
        assert!(
            !heard.is_mine(&HouseholdKey::of(&me)),
            "an unkeyed version 1 tag was read as one of this account's machines"
        );
    }

    #[test]
    fn a_zero_port_is_refused() {
        // Nothing listens on port zero, so an announcement carrying it is
        // either a bug or bait for a connection attempt that cannot succeed.
        let k = keys();
        let packet = seal(&k, &household(), 0, 1);
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
        let mut packet = seal(&k, &household(), 9797, 1);
        let elsewhere = k.sign("itsanas v1 something else entirely", &packet[..SIGNED_LEN]);
        packet[OFF_SIGNATURE..].copy_from_slice(&elsewhere.to_bytes());

        assert!(matches!(
            Announcement::parse(&packet),
            Err(DiscoverError::BadSignature)
        ));
    }

    #[test]
    fn red_team_the_user_id_never_appears_on_the_wire() {
        // THE ATTACK: sit on a café or hotel network and listen. If the user id
        // travelled in the clear, every announcement would say whose machine
        // this is — a stable public key, broadcast every thirty seconds, on
        // whatever network the laptop happens to be attached to. Neither may
        // the household key, which would let the room recognise every tag.
        //
        // If this test fails, running the daemon in public tells the room who
        // you are.
        let me = account(0x5A);
        let real_owner = me.user_id();
        let secret = HouseholdKey::of(&me).secret_bytes();
        let packet = seal(&keys(), &HouseholdKey::of(&me), 9797, 1_700_000_000);

        assert!(
            !packet
                .windows(ID_LEN)
                .any(|window| window == real_owner.as_bytes()),
            "the user id was broadcast in the clear"
        );
        assert!(
            !packet
                .windows(MAC_LEN)
                .any(|window| window == &secret[..MAC_LEN] || window == &secret[MAC_LEN..]),
            "the household key was broadcast"
        );
        assert!(
            !packet
                .windows(ID_LEN)
                .any(|window| window == legacy_owner_tag(real_owner)),
            "the version 1 tag, which anybody holding the user id computes, was sent"
        );
    }

    #[test]
    fn red_team_two_beacons_of_one_account_carry_unlinkable_tags() {
        // THE ATTACK: listen on a shared network and group what you hear. With
        // one tag per account for ever -- version 1 -- every beacon from the
        // laptop, the phone and the Pi said "same owner", to anyone.
        //
        // Two beacons of one device, and beacons of two devices of one
        // account, must carry unrelated tag fields, while the household still
        // recognises every one of them. If this fails, a listener learns which
        // machines belong together, or the household stops finding itself.
        let mine = household();
        let laptop = keys();
        let pi = keys();

        let first = seal(&laptop, &mine, 9797, 1);
        let second = seal(&laptop, &mine, 9797, 1);
        let other = seal(&pi, &mine, 9797, 1);

        let field = |packet: &[u8; BEACON_LEN]| packet[OFF_OWNER..OFF_DEVICE].to_vec();
        assert_ne!(
            field(&first),
            field(&second),
            "two beacons of one machine carried the same tag"
        );
        assert_ne!(field(&first), field(&other));
        // Not merely different: no half of the field repeats either, which a
        // fixed nonce or a nonce-free hash would leave behind.
        assert_ne!(first[OFF_OWNER..OFF_MAC], second[OFF_OWNER..OFF_MAC]);
        assert_ne!(first[OFF_MAC..OFF_DEVICE], second[OFF_MAC..OFF_DEVICE]);

        for packet in [first, second, other] {
            assert!(
                Announcement::parse(&packet).unwrap().is_mine(&mine),
                "the household did not recognise its own machine"
            );
        }
    }

    #[test]
    fn red_team_a_stranger_holding_the_user_id_cannot_recognise_the_tag() {
        // THE ATTACK: a user id is a public key -- the coordinator hands it
        // out, a shared folder shows it. With version 1 that was all it took to
        // pick a person's machines out of a room. Now recognising a tag needs
        // the account's master secret.
        //
        // If this fails, the tag is a hash of something public again, and
        // anybody who knows who you are can find your machines.
        let me = account(1);
        let stranger = account(2);
        let heard = Announcement::parse(&seal(&keys(), &HouseholdKey::of(&me), 9797, 1)).unwrap();

        assert!(heard.is_mine(&HouseholdKey::of(&me)));
        assert!(
            !heard.is_mine(&HouseholdKey::of(&stranger)),
            "another account recognised this account's beacon"
        );
        let OwnerTag::Keyed { nonce, mac } = heard.owner_tag else {
            panic!("this build must write version 2");
        };
        let public = me.user_id();
        let unkeyed = blake3::Hasher::new()
            .update(&nonce)
            .update(heard.device.as_bytes())
            .finalize();
        let with_id = blake3::Hasher::new_keyed(public.as_bytes())
            .update(&nonce)
            .update(heard.device.as_bytes())
            .finalize();
        assert_ne!(&unkeyed.as_bytes()[..MAC_LEN], &mac);
        assert_ne!(
            &with_id.as_bytes()[..MAC_LEN],
            &mac,
            "the tag is keyed on the public user id, which anybody holds"
        );
    }

    #[test]
    fn red_team_a_tag_lifted_onto_another_device_is_not_recognised() {
        // THE ATTACK: hear one of the household's beacons, copy its tag into a
        // beacon signed by your own freshly minted device, and sort to the
        // front of every member's dial order. The device is inside the keyed
        // hash, so the copy fails. If this fails, anybody who can hear you can
        // pose as one of your machines.
        let mine = household();
        let honest = seal(&keys(), &mine, 9797, 1);
        let attacker = keys();
        let mut owner = [0u8; ID_LEN];
        owner.copy_from_slice(&honest[OFF_OWNER..OFF_DEVICE]);
        let forged = Announcement::sign(&attacker, BEACON_VERSION, &owner, 9797, 1);

        let heard = Announcement::parse(&forged).unwrap();
        assert!(
            !heard.is_mine(&mine),
            "a copied tag made a stranger's device read as one of ours"
        );
    }

    #[test]
    fn the_household_recognises_itself_whatever_its_clock_says() {
        // Deliberately no clock in the tag. A Raspberry Pi 4 has no real-time
        // clock and boots believing it is 1970; a tag that rotated on time
        // would make its own household treat it as a stranger exactly when it
        // came back from a power cut.
        let mine = household();
        for clock in [0, 1_700_000_000, u64::MAX] {
            let heard = Announcement::parse(&seal(&keys(), &mine, 9797, clock)).unwrap();
            assert!(
                heard.is_mine(&mine),
                "not recognised with the clock at {clock}"
            );
        }
    }

    #[test]
    fn an_ancient_clock_still_produces_a_valid_announcement() {
        // A Raspberry Pi with no RTC announces itself believing it is 1970.
        // It must still be findable, or a machine that has just come back is
        // invisible for exactly as long as it takes NTP to run.
        let k = keys();
        let packet = seal(&k, &household(), 9797, 0);
        let parsed = Announcement::parse(&packet).unwrap();
        assert_eq!(parsed.sent_unix, 0);
        assert_eq!(parsed.device, k.device_id());
    }
}
