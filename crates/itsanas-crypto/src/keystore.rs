//! Passphrase-protected storage for the master secret.
//!
//! The same container serves two jobs:
//!
//! * The **local keystore** on each device, so the daemon can start without the
//!   user retyping 24 words.
//! * The **escrow blob**, uploaded to the coordinator so a user can log in on a
//!   brand-new machine with a username and passphrase. The coordinator sees
//!   only opaque bytes; it has no path to the contents.
//!
//! Because the escrow blob is held by a third party, the passphrase is the only
//! thing standing between an attacker who steals the coordinator's database and
//! a user's data. That is why the KDF cost is deliberately high and why the
//! recovery phrase — not the passphrase — remains the authoritative backup.

use core::fmt;

use argon2::{Algorithm, Argon2, Params, Version};

use crate::{
    error::{CryptoError, Result},
    seal::{self, SEAL_VERSION},
    secret::{SecretBytes, SymmetricKey},
};

const SALT_LEN: usize = 16;
const KDF_ARGON2ID: u8 = 1;
const HEADER_LEN: usize = 1 + 1 + 4 + 4 + 1 + SALT_LEN;

/// Argon2id cost parameters.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KdfParams {
    /// Memory cost in kibibytes.
    pub memory_kib: u32,
    /// Number of passes.
    pub iterations: u32,
    /// Degree of parallelism.
    pub lanes: u8,
}

impl KdfParams {
    /// Production defaults: 64 MiB, 3 passes, single lane.
    ///
    /// Chosen to be comfortably above OWASP's Argon2id floor while still
    /// unlocking in well under a second on a Raspberry Pi 4, which is the
    /// weakest machine ITSaNAS targets.
    pub const RECOMMENDED: Self = Self {
        memory_kib: 65_536,
        iterations: 3,
        lanes: 1,
    };

    /// Deliberately weak parameters, so the test suite stays fast.
    ///
    /// This is ordinary public API — it has to be, because integration tests in
    /// sibling crates use it — so nothing stops production code from reaching
    /// for it by mistake. [`Self::meets_production_floor`] is the guard: any
    /// code path that locks a real user's secret must check it and refuse.
    pub const INSECURE_FOR_TESTS: Self = Self {
        memory_kib: 8,
        iterations: 1,
        lanes: 1,
    };

    /// Largest cost this build will run, in kibibytes (1 GiB).
    ///
    /// The escrow blob arrives from the coordinator, which is untrusted, and
    /// [`Keystore::unlock`] must run the KDF *before* the AEAD tag can be
    /// checked — there is no way to authenticate first. Binding the parameters
    /// into the associated data stops an attacker lowering the cost; only a
    /// hard ceiling stops them raising it. Without this, a hostile coordinator
    /// serves a blob claiming 4 TiB of Argon2 memory and every client that
    /// tries to log in dies allocating.
    pub const MAX_MEMORY_KIB: u32 = 1024 * 1024;

    /// Largest pass count this build will run.
    pub const MAX_ITERATIONS: u32 = 16;

    /// Largest degree of parallelism this build will run.
    pub const MAX_LANES: u8 = 8;

    /// Minimum cost considered acceptable for a real user's secret.
    ///
    /// 19 MiB with 2 passes is the OWASP Argon2id floor.
    pub const MIN_PRODUCTION_MEMORY_KIB: u32 = 19 * 1024;
    /// Minimum pass count considered acceptable for a real user's secret.
    pub const MIN_PRODUCTION_ITERATIONS: u32 = 2;

    /// Whether these parameters are strong enough to protect a real secret.
    ///
    /// Call this before locking anything that matters, and refuse if it is
    /// false. [`Self::INSECURE_FOR_TESTS`] deliberately fails it.
    #[must_use]
    pub const fn meets_production_floor(self) -> bool {
        self.memory_kib >= Self::MIN_PRODUCTION_MEMORY_KIB
            && self.iterations >= Self::MIN_PRODUCTION_ITERATIONS
            && self.lanes >= 1
    }

    /// Reject costs this build refuses to run at all.
    ///
    /// Applied when parsing untrusted bytes, not when a local caller chooses
    /// parameters, so tests can still use cheap settings.
    fn check_runnable(self) -> Result<()> {
        if self.memory_kib > Self::MAX_MEMORY_KIB {
            return Err(CryptoError::Kdf(
                "keystore demands more Argon2id memory than this build will allocate",
            ));
        }
        if self.iterations > Self::MAX_ITERATIONS {
            return Err(CryptoError::Kdf(
                "keystore demands more Argon2id passes than this build will run",
            ));
        }
        if self.lanes > Self::MAX_LANES {
            return Err(CryptoError::Kdf(
                "keystore demands more Argon2id lanes than this build will run",
            ));
        }
        if self.memory_kib == 0 || self.iterations == 0 || self.lanes == 0 {
            return Err(CryptoError::Kdf("keystore Argon2id cost is zero"));
        }
        Ok(())
    }

    fn derive(self, passphrase: &str, salt: &[u8]) -> Result<SymmetricKey> {
        let params = Params::new(
            self.memory_kib,
            self.iterations,
            u32::from(self.lanes),
            Some(32),
        )
        .map_err(|_| CryptoError::Kdf("argon2 parameters out of range"))?;

        let mut key = [0u8; 32];
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(passphrase.as_bytes(), salt, &mut key)
            .map_err(|_| CryptoError::Kdf("argon2 derivation failed"))?;

        Ok(SecretBytes::new(key))
    }
}

/// A passphrase-sealed payload, safe to write to disk or hand to a server.
#[derive(Clone, PartialEq, Eq)]
pub struct Keystore {
    params: KdfParams,
    salt: [u8; SALT_LEN],
    sealed: Vec<u8>,
}

impl Keystore {
    /// Seal `payload` under `passphrase`.
    ///
    /// `label` names what this container is for — for instance
    /// `"itsanas/keystore/local"` or `"itsanas/escrow/alice"`. It is bound into
    /// the associated data, so an escrow blob cannot be passed off as a local
    /// keystore, and one user's escrow blob cannot be served in place of
    /// another's.
    pub fn lock(passphrase: &str, label: &str, payload: &[u8], params: KdfParams) -> Result<Self> {
        let mut salt = [0u8; SALT_LEN];
        getrandom::fill(&mut salt).map_err(CryptoError::Entropy)?;

        let key = params.derive(passphrase, &salt)?;
        let aad = associated_data(params, &salt, label);
        let sealed = seal::seal_with_key(&key, &aad, payload)?;

        Ok(Self {
            params,
            salt,
            sealed,
        })
    }

    /// Recover the payload. Fails indistinguishably for a wrong passphrase, a
    /// wrong label, and a corrupted container — all three are
    /// [`CryptoError::Decrypt`].
    pub fn unlock(&self, passphrase: &str, label: &str) -> Result<Vec<u8>> {
        let key = self.params.derive(passphrase, &self.salt)?;
        let aad = associated_data(self.params, &self.salt, label);
        seal::open_with_key(&key, &aad, &self.sealed)
    }

    #[must_use]
    pub const fn params(&self) -> KdfParams {
        self.params
    }

    /// Serialise to the on-disk representation.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.sealed.len());
        out.push(SEAL_VERSION);
        out.push(KDF_ARGON2ID);
        out.extend_from_slice(&self.params.memory_kib.to_le_bytes());
        out.extend_from_slice(&self.params.iterations.to_le_bytes());
        out.push(self.params.lanes);
        out.extend_from_slice(&self.salt);
        out.extend_from_slice(&self.sealed);
        out
    }

    /// Parse the on-disk representation.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_LEN {
            return Err(CryptoError::Malformed("keystore is truncated"));
        }

        match bytes[0] {
            SEAL_VERSION => {}
            found => {
                return Err(CryptoError::UnsupportedVersion {
                    kind: "keystore",
                    found,
                    supported: SEAL_VERSION,
                });
            }
        }

        if bytes[1] != KDF_ARGON2ID {
            return Err(CryptoError::UnsupportedVersion {
                kind: "keystore key-derivation function",
                found: bytes[1],
                supported: KDF_ARGON2ID,
            });
        }

        let memory_kib = u32::from_le_bytes(bytes[2..6].try_into().expect("4 bytes"));
        let iterations = u32::from_le_bytes(bytes[6..10].try_into().expect("4 bytes"));
        let lanes = bytes[10];
        let salt: [u8; SALT_LEN] = bytes[11..HEADER_LEN].try_into().expect("salt length");

        let params = KdfParams {
            memory_kib,
            iterations,
            lanes,
        };
        // Rejected at parse time rather than at unlock time, so no caller can
        // hold a Keystore that will detonate the moment it is used.
        params.check_runnable()?;

        Ok(Self {
            params,
            salt,
            sealed: bytes[HEADER_LEN..].to_vec(),
        })
    }
}

impl fmt::Debug for Keystore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Keystore")
            .field("params", &self.params)
            .field("sealed_len", &self.sealed.len())
            .finish_non_exhaustive()
    }
}

/// Canonical associated data: every KDF parameter plus the container's label.
///
/// Binding the parameters is what stops an attacker from handing back the same
/// ciphertext with `memory_kib` rewritten to 8, turning a 64 MiB Argon2id
/// container into one that can be brute-forced on a laptop.
fn associated_data(params: KdfParams, salt: &[u8; SALT_LEN], label: &str) -> Vec<u8> {
    let label = label.as_bytes();
    let mut aad = Vec::with_capacity(HEADER_LEN + 4 + label.len());
    aad.push(SEAL_VERSION);
    aad.push(KDF_ARGON2ID);
    aad.extend_from_slice(&params.memory_kib.to_le_bytes());
    aad.extend_from_slice(&params.iterations.to_le_bytes());
    aad.push(params.lanes);
    aad.extend_from_slice(salt);
    aad.extend_from_slice(&u32::try_from(label.len()).unwrap_or(u32::MAX).to_le_bytes());
    aad.extend_from_slice(label);
    aad
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::MasterSecret;

    const TEST: KdfParams = KdfParams::INSECURE_FOR_TESTS;
    const LABEL: &str = "itsanas/keystore/local";

    #[test]
    fn round_trips_a_master_secret() {
        let master = MasterSecret::generate().unwrap();
        let store =
            Keystore::lock("correct horse battery staple", LABEL, master.expose(), TEST).unwrap();

        let recovered = store.unlock("correct horse battery staple", LABEL).unwrap();
        assert_eq!(recovered, master.expose());
    }

    #[test]
    fn survives_serialisation() {
        let store = Keystore::lock("pass", LABEL, b"payload", TEST).unwrap();
        let reloaded = Keystore::from_bytes(&store.to_bytes()).unwrap();

        assert_eq!(reloaded, store);
        assert_eq!(reloaded.unlock("pass", LABEL).unwrap(), b"payload");
    }

    #[test]
    fn a_wrong_passphrase_fails() {
        let store = Keystore::lock("right", LABEL, b"payload", TEST).unwrap();

        assert!(matches!(
            store.unlock("wrong", LABEL),
            Err(CryptoError::Decrypt)
        ));
        assert!(store.unlock("", LABEL).is_err());
        assert!(store.unlock("right ", LABEL).is_err());
    }

    #[test]
    fn an_escrow_blob_cannot_be_passed_off_as_a_local_keystore() {
        let store = Keystore::lock("pass", "itsanas/escrow/alice", b"payload", TEST).unwrap();

        assert!(
            store.unlock("pass", "itsanas/keystore/local").is_err(),
            "container labels are not bound, so a stolen escrow blob could be \
             dropped in as a device keystore"
        );
    }

    #[test]
    fn one_users_escrow_blob_cannot_be_served_for_another() {
        let store = Keystore::lock("shared-pass", "itsanas/escrow/alice", b"alice", TEST).unwrap();

        assert!(
            store.unlock("shared-pass", "itsanas/escrow/bob").is_err(),
            "a malicious coordinator could hand Bob's client Alice's blob"
        );
    }

    #[test]
    fn downgrading_the_kdf_cost_is_detected() {
        let store = Keystore::lock(
            "pass",
            LABEL,
            b"payload",
            KdfParams {
                memory_kib: 32,
                iterations: 2,
                lanes: 1,
            },
        )
        .unwrap();

        let mut bytes = store.to_bytes();
        // Rewrite memory_kib from 32 to 8, the cheapest Argon2id allows.
        bytes[2..6].copy_from_slice(&8u32.to_le_bytes());

        let tampered = Keystore::from_bytes(&bytes).unwrap();
        assert_eq!(tampered.params().memory_kib, 8);
        assert!(
            tampered.unlock("pass", LABEL).is_err(),
            "an attacker rewrote the Argon2id cost downward and the container \
             still opened, making offline cracking cheap"
        );
    }

    #[test]
    fn an_absurd_kdf_cost_is_refused_at_parse_time() {
        // The escrow blob comes from the untrusted coordinator, and unlock has
        // to run Argon2id before any tag can be checked. A hostile coordinator
        // that could name the cost could kill every client trying to log in.
        let store = Keystore::lock("pass", LABEL, b"payload", TEST).unwrap();

        for (offset, value) in [
            (2, u32::MAX),                      // memory_kib
            (2, KdfParams::MAX_MEMORY_KIB + 1), // just over the ceiling
            (6, u32::MAX),                      // iterations
            (6, KdfParams::MAX_ITERATIONS + 1), // just over the ceiling
        ] {
            let mut bytes = store.to_bytes();
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());

            assert!(
                Keystore::from_bytes(&bytes).is_err(),
                "a keystore claiming {value} at offset {offset} parsed \
                 successfully; unlocking it would exhaust the machine"
            );
        }

        let mut too_many_lanes = store.to_bytes();
        too_many_lanes[10] = KdfParams::MAX_LANES + 1;
        assert!(Keystore::from_bytes(&too_many_lanes).is_err());

        let mut zero_cost = store.to_bytes();
        zero_cost[6..10].copy_from_slice(&0u32.to_le_bytes());
        assert!(Keystore::from_bytes(&zero_cost).is_err());
    }

    #[test]
    fn the_production_floor_rejects_the_parameters_the_tests_use() {
        assert!(
            !KdfParams::INSECURE_FOR_TESTS.meets_production_floor(),
            "the deliberately weak test parameters passed the production floor, \
             so the guard protects nothing"
        );
        assert!(
            KdfParams::RECOMMENDED.meets_production_floor(),
            "the recommended parameters fail their own floor"
        );

        // The floor is the OWASP Argon2id minimum, so anything at or above it
        // passes and anything below it does not.
        assert!(
            KdfParams {
                memory_kib: KdfParams::MIN_PRODUCTION_MEMORY_KIB,
                iterations: KdfParams::MIN_PRODUCTION_ITERATIONS,
                lanes: 1,
            }
            .meets_production_floor()
        );
        assert!(
            !KdfParams {
                memory_kib: KdfParams::MIN_PRODUCTION_MEMORY_KIB - 1,
                iterations: KdfParams::MIN_PRODUCTION_ITERATIONS,
                lanes: 1,
            }
            .meets_production_floor()
        );
    }

    #[test]
    fn tampering_with_the_salt_is_detected() {
        let store = Keystore::lock("pass", LABEL, b"payload", TEST).unwrap();
        let mut bytes = store.to_bytes();
        bytes[11] ^= 0xFF;

        assert!(
            Keystore::from_bytes(&bytes)
                .unwrap()
                .unlock("pass", LABEL)
                .is_err()
        );
    }

    #[test]
    fn tampering_with_the_ciphertext_is_detected() {
        let store = Keystore::lock("pass", LABEL, b"payload", TEST).unwrap();
        let mut bytes = store.to_bytes();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;

        assert!(
            Keystore::from_bytes(&bytes)
                .unwrap()
                .unlock("pass", LABEL)
                .is_err()
        );
    }

    #[test]
    fn each_lock_uses_a_fresh_salt() {
        let a = Keystore::lock("pass", LABEL, b"payload", TEST).unwrap();
        let b = Keystore::lock("pass", LABEL, b"payload", TEST).unwrap();

        assert_ne!(a.salt, b.salt, "salt reuse across keystores");
        assert_ne!(a.to_bytes(), b.to_bytes());
    }

    #[test]
    fn malformed_input_is_rejected_without_panicking() {
        assert!(Keystore::from_bytes(&[]).is_err());
        for len in 0..HEADER_LEN {
            assert!(Keystore::from_bytes(&vec![SEAL_VERSION; len]).is_err());
        }

        let mut wrong_version = vec![0u8; HEADER_LEN + 40];
        wrong_version[0] = 200;
        assert!(matches!(
            Keystore::from_bytes(&wrong_version),
            Err(CryptoError::UnsupportedVersion { found: 200, .. })
        ));

        let mut wrong_kdf = vec![0u8; HEADER_LEN + 40];
        wrong_kdf[0] = SEAL_VERSION;
        wrong_kdf[1] = 7;
        assert!(matches!(
            Keystore::from_bytes(&wrong_kdf),
            Err(CryptoError::UnsupportedVersion { found: 7, .. })
        ));
    }

    #[test]
    #[ignore = "runs the real 64 MiB Argon2id cost; enable with --ignored"]
    fn recommended_parameters_actually_work() {
        let master = MasterSecret::generate().unwrap();
        let store = Keystore::lock(
            "a genuinely long passphrase for the escrow blob",
            "itsanas/escrow/nicolas",
            master.expose(),
            KdfParams::RECOMMENDED,
        )
        .unwrap();

        assert_eq!(
            store
                .unlock(
                    "a genuinely long passphrase for the escrow blob",
                    "itsanas/escrow/nicolas"
                )
                .unwrap(),
            master.expose()
        );
    }
}
