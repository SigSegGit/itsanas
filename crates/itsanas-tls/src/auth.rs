//! Proving which device is on the other end.
//!
//! # Why not certificate pinning
//!
//! The obvious design is to put the device's Ed25519 key in its certificate and
//! have the peer compare it against the expected `DeviceId`. That requires
//! parsing X.509 to pull the key back out, which means either a general-purpose
//! certificate parser in the trusted path or hand-rolled ASN.1. Both are
//! avoidable attack surface for something this project does not otherwise need.
//!
//! Instead the certificates are anonymous — TLS provides confidentiality and
//! integrity and nothing else — and identity is proved one layer up, by each
//! side signing the TLS session's **exporter value** with its device key.
//!
//! # Why that is not weaker
//!
//! The exporter is derived from the session's own secrets. A man in the middle
//! who terminates TLS has *two* sessions with two different exporters, so a
//! signature made for one is worthless in the other: they cannot relay it, and
//! they cannot produce their own without the device key. Binding the
//! application identity to the channel is what closes the gap that "accept any
//! certificate" would otherwise open, and it is the same reasoning behind
//! RFC 5929 channel binding.
//!
//! The cost is one extra round trip and being explicit that the certificate
//! means nothing. Both are worth it.

use itsanas_crypto::{DeviceId, DeviceKeys, Signature, verify};
use serde::{Deserialize, Serialize};

use crate::error::{Result, TlsError};

/// Signature domain for device authentication.
pub const AUTH_DOMAIN: &str = "itsanas v1 device channel authentication";

/// Label handed to the TLS exporter.
pub const EXPORTER_LABEL: &[u8] = b"itsanas v1 device authentication";

/// Bytes of exporter material used.
pub const EXPORTER_LEN: usize = 32;

/// What each side sends to prove who it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthHello {
    /// The device claiming this end of the connection.
    pub device: DeviceId,
    /// A signature over this TLS session's exporter value.
    pub signature: Signature,
}

/// Sign this session's exporter value.
#[must_use]
pub fn prove(device: &DeviceKeys, exporter: &[u8; EXPORTER_LEN]) -> AuthHello {
    AuthHello {
        device: device.device_id(),
        signature: device.sign(AUTH_DOMAIN, exporter),
    }
}

/// Check a peer's proof against this session's exporter value.
///
/// Returns the device that proved itself. `expected` pins the answer: pass it
/// when dialling a peer whose identity is already known, and the connection is
/// refused if a different device answers.
pub fn check(
    hello: &AuthHello,
    exporter: &[u8; EXPORTER_LEN],
    expected: Option<DeviceId>,
) -> Result<DeviceId> {
    verify(
        hello.device.as_bytes(),
        AUTH_DOMAIN,
        exporter,
        hello.signature,
    )
    .map_err(|_| TlsError::AuthenticationFailed(hello.device.short()))?;

    if let Some(expected) = expected
        && expected != hello.device
    {
        return Err(TlsError::WrongPeer {
            expected: expected.short(),
            found: hello.device.short(),
        });
    }

    Ok(hello.device)
}

#[cfg(test)]
mod tests {
    use super::*;
    use itsanas_crypto::SecretBytes;

    fn device(byte: u8) -> DeviceKeys {
        DeviceKeys::from_seed(&SecretBytes::new([byte; 32]))
    }

    const SESSION: [u8; EXPORTER_LEN] = [0x5A; EXPORTER_LEN];
    const OTHER_SESSION: [u8; EXPORTER_LEN] = [0xA5; EXPORTER_LEN];

    #[test]
    fn an_honest_proof_identifies_the_device() {
        let dev = device(1);
        let hello = prove(&dev, &SESSION);

        assert_eq!(check(&hello, &SESSION, None).unwrap(), dev.device_id());
        assert_eq!(
            check(&hello, &SESSION, Some(dev.device_id())).unwrap(),
            dev.device_id()
        );
    }

    #[test]
    fn a_proof_from_one_session_is_worthless_in_another() {
        // The property the whole design rests on. A man in the middle who
        // terminates TLS has two sessions with two different exporters, so a
        // proof captured from one cannot be relayed into the other.
        let dev = device(2);
        let hello = prove(&dev, &SESSION);

        assert!(
            check(&hello, &OTHER_SESSION, None).is_err(),
            "a proof was replayed into a different session; a man in the \
             middle could relay it and impersonate the device"
        );
    }

    #[test]
    fn claiming_to_be_another_device_fails() {
        let honest = device(3);
        let attacker = device(4);

        let mut forged = prove(&attacker, &SESSION);
        forged.device = honest.device_id();

        assert!(matches!(
            check(&forged, &SESSION, None),
            Err(TlsError::AuthenticationFailed(_))
        ));
    }

    #[test]
    fn dialling_a_known_peer_refuses_a_different_answer() {
        // Without this, a coordinator that redirected an address could hand a
        // caller to a machine of its choosing and the caller would not notice.
        let expected = device(5);
        let actual = device(6);

        let hello = prove(&actual, &SESSION);

        assert!(matches!(
            check(&hello, &SESSION, Some(expected.device_id())),
            Err(TlsError::WrongPeer { .. })
        ));
    }

    #[test]
    fn a_tampered_signature_is_refused() {
        let dev = device(7);
        let hello = prove(&dev, &SESSION);

        let mut bytes = hello.signature.to_bytes();
        bytes[0] ^= 0x01;

        let tampered = AuthHello {
            device: hello.device,
            signature: Signature::from_bytes(bytes),
        };

        assert!(check(&tampered, &SESSION, None).is_err());
    }

    #[test]
    fn the_proof_round_trips_through_the_wire() {
        let dev = device(8);
        let hello = prove(&dev, &SESSION);

        let frame = itsanas_wire::encode(&hello).unwrap();
        assert_eq!(itsanas_wire::decode::<AuthHello>(&frame).unwrap(), hello);
    }
}
