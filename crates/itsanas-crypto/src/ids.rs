use core::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::error::CryptoError;

/// Every identifier in ITSaNAS is exactly 32 bytes.
pub const ID_LEN: usize = 32;

fn to_hex(bytes: &[u8; ID_LEN]) -> String {
    let mut out = String::with_capacity(ID_LEN * 2);
    for byte in bytes {
        use core::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn from_hex(text: &str) -> Result<[u8; ID_LEN], CryptoError> {
    if text.len() != ID_LEN * 2 {
        return Err(CryptoError::Malformed("identifier: expected 64 hex digits"));
    }
    let mut out = [0u8; ID_LEN];
    for (slot, pair) in out.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
        let hi = (pair[0] as char)
            .to_digit(16)
            .ok_or(CryptoError::Malformed("identifier: non-hex digit"))?;
        let lo = (pair[1] as char)
            .to_digit(16)
            .ok_or(CryptoError::Malformed("identifier: non-hex digit"))?;
        *slot = u8::try_from(hi * 16 + lo).expect("hex nibbles fit in a byte");
    }
    Ok(out)
}

macro_rules! define_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; ID_LEN]);

        impl $name {
            #[must_use]
            pub const fn from_bytes(bytes: [u8; ID_LEN]) -> Self {
                Self(bytes)
            }

            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; ID_LEN] {
                &self.0
            }

            #[must_use]
            pub const fn to_bytes(self) -> [u8; ID_LEN] {
                self.0
            }

            pub fn from_slice(bytes: &[u8]) -> Result<Self, CryptoError> {
                bytes
                    .try_into()
                    .map(Self)
                    .map_err(|_| CryptoError::Malformed("identifier: expected 32 bytes"))
            }

            /// Full 64-character hex rendering.
            #[must_use]
            pub fn to_hex(&self) -> String {
                to_hex(&self.0)
            }

            /// First 12 hex characters, for logs and CLI output where a full
            /// identifier would be noise.
            #[must_use]
            pub fn short(&self) -> String {
                self.to_hex()[..12].to_owned()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.to_hex())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.short())
            }
        }

        impl FromStr for $name {
            type Err = CryptoError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                from_hex(text).map(Self)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
                if ser.is_human_readable() {
                    ser.serialize_str(&self.to_hex())
                } else {
                    ser.serialize_bytes(&self.0)
                }
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
                if de.is_human_readable() {
                    let text = <&str>::deserialize(de)?;
                    text.parse().map_err(de::Error::custom)
                } else {
                    let bytes = <serde_bytes_compat::ByteArray>::deserialize(de)?;
                    Ok(Self(bytes.0))
                }
            }
        }
    };
}

/// Minimal `[u8; 32]` deserializer that accepts both `serialize_bytes` output
/// and generic sequences, so the ID types work with any binary codec.
mod serde_bytes_compat {
    use core::fmt;

    use serde::{Deserialize, Deserializer, de};

    use super::ID_LEN;

    pub struct ByteArray(pub [u8; ID_LEN]);

    impl<'de> Deserialize<'de> for ByteArray {
        fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
            struct Visitor;

            impl<'de> de::Visitor<'de> for Visitor {
                type Value = ByteArray;

                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    write!(f, "32 bytes")
                }

                fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                    value
                        .try_into()
                        .map(ByteArray)
                        .map_err(|_| E::invalid_length(value.len(), &self))
                }

                fn visit_seq<A: de::SeqAccess<'de>>(
                    self,
                    mut seq: A,
                ) -> Result<Self::Value, A::Error> {
                    let mut out = [0u8; ID_LEN];
                    for (index, slot) in out.iter_mut().enumerate() {
                        *slot = seq
                            .next_element()?
                            .ok_or_else(|| de::Error::invalid_length(index, &self))?;
                    }
                    Ok(ByteArray(out))
                }
            }

            de.deserialize_bytes(Visitor)
        }
    }
}

define_id! {
    /// A user's stable public identity: the raw bytes of their Ed25519 master
    /// verifying key. Derived from the master secret, so it survives losing
    /// every device.
    UserId
}

define_id! {
    /// One physical machine belonging to a user. Randomly generated when the
    /// device joins, certified by the user's master key, and revocable on its
    /// own so a stolen laptop never forces a master-key rotation.
    DeviceId
}

define_id! {
    /// The storage address of one encrypted chunk.
    ///
    /// Blinded with the owner's secret blinding key, so it is deterministic for
    /// the owner (which is what makes deduplication work) while telling a host
    /// nothing at all about the plaintext it names.
    ChunkId
}

define_id! {
    /// The storage address of a non-chunk object: an operation-log segment or a
    /// signed head record. Randomly generated, never derived from content.
    ObjectId
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        let id = ChunkId::from_bytes([0x0f; ID_LEN]);
        assert_eq!(id.to_hex().len(), 64);
        assert_eq!(id.to_hex().parse::<ChunkId>().unwrap(), id);
    }

    #[test]
    fn short_form_is_twelve_chars() {
        let id = UserId::from_bytes([0xde; ID_LEN]);
        assert_eq!(id.short(), "dededededede");
        assert_eq!(format!("{id:?}"), "UserId(dededededede)");
    }

    #[test]
    fn parsing_rejects_bad_input() {
        assert!("".parse::<UserId>().is_err());
        assert!("zz".repeat(32).parse::<UserId>().is_err());
        assert!("ab".repeat(31).parse::<UserId>().is_err());
        assert!("ab".repeat(33).parse::<UserId>().is_err());
        assert!("ab".repeat(32).parse::<UserId>().is_ok());
    }

    #[test]
    fn hex_rendering_is_lowercase_and_zero_padded() {
        let mut bytes = [0u8; ID_LEN];
        bytes[0] = 0x05;
        bytes[31] = 0xFF;
        let id = ObjectId::from_bytes(bytes);
        assert!(id.to_hex().starts_with("05"));
        assert!(id.to_hex().ends_with("ff"));
    }
}
