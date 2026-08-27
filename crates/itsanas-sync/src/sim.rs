//! A deterministic multi-device swarm, for proving convergence.
//!
//! Convergence is not the kind of property you establish by reading the merge
//! rules carefully and concluding they look right. The failure mode — two
//! devices settling on different content and never noticing — is silent, and
//! it only shows up in the orderings nobody thought about. So the rules are
//! subjected here to scenarios designed to break them: partitions, devices that
//! never come back, deletes racing edits, three-way conflicts, and operations
//! arriving in every order.
//!
//! # What is real and what is simulated
//!
//! **Real:** the stores, the chunking, the sealing, the signatures, the version
//! vectors, the merge rules, the conflict resolution. Every device is an actual
//! [`Store`] on an actual temporary directory doing actual cryptography.
//!
//! **Simulated:** the network. [`Cloud`] stands in for the set of blind hosts
//! that a real deployment would replicate to. It holds sealed chunks and signed
//! segments and — exactly like a real host — cannot read any of them.
//!
//! Nothing here uses randomness or wall-clock time, so a failing scenario fails
//! identically on every machine and every run.
//!
//! # Example
//!
//! The scenario the whole architecture exists for: a device writes, publishes,
//! and is switched off permanently, and the others still converge on its work.
//!
//! ```
//! use itsanas_sync::sim::Swarm;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut swarm = Swarm::new(3)?;
//!
//! // The Pi writes while the laptop is asleep, then goes offline for good.
//! swarm.device(1).write("notes.txt", b"written on the Pi")?;
//! swarm.device(1).publish()?;
//! swarm.set_online(1, false);
//!
//! // The laptop wakes and catches up from a host that cannot read the data.
//! swarm.settle()?;
//!
//! assert_eq!(swarm.device(0).read("notes.txt")?.unwrap(), b"written on the Pi");
//! # Ok(())
//! # }
//! ```

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use itsanas_crypto::{ChunkId, DeviceKeys, MasterSecret, SecretBytes, UserId, UserKeys};
use itsanas_store::{SegmentEnvelope, Store};

use crate::{
    engine::{self, Divergence, SyncReport},
    error::{Result, SyncError},
    source::ChunkSource,
};

/// The stand-in for every blind host in the network.
///
/// Holds sealed chunks and signed segment envelopes. It has no keys, and the
/// only reason it can serve a segment to the right device is that envelopes are
/// plaintext by design — the bodies are not.
#[derive(Debug, Default)]
pub struct Cloud {
    chunks: BTreeMap<(UserId, ChunkId), Vec<u8>>,
    segments: Vec<SegmentEnvelope>,
}

impl Cloud {
    /// How many distinct sealed chunks are held.
    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// How many segments are held.
    #[must_use]
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// Total bytes held, as a host would account for them.
    #[must_use]
    pub fn total_bytes(&self) -> usize {
        self.chunks.values().map(Vec::len).sum()
    }

    /// The segments held, so a test can check what a host can verify.
    #[must_use]
    pub fn segments(&self) -> &[SegmentEnvelope] {
        &self.segments
    }

    /// Drop every chunk, keeping the segments.
    ///
    /// Models the real and common situation where a segment reaches a device
    /// before the chunks it names do — the only host holding them went offline
    /// in between.
    pub fn forget_all_chunks(&mut self) {
        self.chunks.clear();
    }

    /// Every byte the cloud holds, for plaintext-leak scans.
    #[must_use]
    pub fn all_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for bytes in self.chunks.values() {
            out.extend_from_slice(bytes);
        }
        for segment in &self.segments {
            if let Ok(encoded) = segment.encode() {
                out.extend_from_slice(&encoded);
            }
        }
        out
    }
}

/// A shared handle to the cloud, usable as a [`ChunkSource`].
#[derive(Clone, Debug, Default)]
pub struct CloudHandle(Arc<Mutex<Cloud>>);

impl CloudHandle {
    /// Run `f` against the cloud.
    ///
    /// # Panics
    ///
    /// If a previous caller panicked while holding the lock. In a test harness
    /// that is the correct response: the run is already invalid.
    pub fn with<T>(&self, f: impl FnOnce(&mut Cloud) -> T) -> T {
        f(&mut self
            .0
            .lock()
            .expect("the simulated cloud lock was poisoned"))
    }
}

impl ChunkSource for CloudHandle {
    fn fetch(&self, owner: UserId, address: &ChunkId) -> Result<Option<Vec<u8>>> {
        Ok(self.with(|cloud| cloud.chunks.get(&(owner, *address)).cloned()))
    }
}

/// One device in the swarm.
#[derive(Debug)]
pub struct SimDevice {
    store: Store,
    cloud: CloudHandle,
    online: bool,
}

impl SimDevice {
    /// The underlying store, for assertions the harness does not wrap.
    #[must_use]
    pub const fn store(&self) -> &Store {
        &self.store
    }

    #[must_use]
    pub const fn is_online(&self) -> bool {
        self.online
    }

    /// Write a file locally. Works offline — that is the entire point.
    pub fn write(&self, path: &str, content: &[u8]) -> Result<()> {
        self.store.write_file(path, content)?;
        Ok(())
    }

    /// Delete a file locally.
    pub fn remove(&self, path: &str) -> Result<bool> {
        Ok(self.store.remove_file(path)?)
    }

    /// Read a file back.
    pub fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.store.read_file(path)?)
    }

    /// Every path this device currently believes in.
    pub fn list(&self) -> Result<Vec<String>> {
        Ok(self.store.list()?)
    }

    /// Seal pending writes into a segment and push it, with every chunk this
    /// device holds, to the cloud.
    ///
    /// A no-op while offline: an unreachable device cannot publish, which is
    /// exactly the situation the sync design exists to survive.
    pub fn publish(&self) -> Result<bool> {
        if !self.online {
            return Ok(false);
        }

        let segment = self.store.flush_segment()?;
        let owner = self.store.owner();

        // Upload every chunk this device holds. A real node replicates only to
        // its placement targets; here every host holds everything, which is the
        // strictly harder case for the merge logic because nothing is ever
        // unavailable for an accidental reason.
        for address in self.store.blobs().addresses()? {
            if let Some(bytes) = self.store.blobs().get(&address)? {
                self.cloud
                    .with(|cloud| cloud.chunks.insert((owner, address), bytes));
            }
        }

        if let Some(segment) = segment {
            self.cloud.with(|cloud| cloud.segments.push(segment));
            return Ok(true);
        }

        Ok(false)
    }

    /// Pull every segment the cloud holds and merge it into local state.
    ///
    /// A no-op while offline.
    pub fn sync(&self) -> Result<SyncReport> {
        if !self.online {
            return Ok(SyncReport::default());
        }

        let owner = self.store.owner();
        let segments = self.cloud.with(|cloud| {
            cloud
                .segments
                .iter()
                .filter(|segment| segment.owner == owner)
                .cloned()
                .collect::<Vec<_>>()
        });

        let (report, _) = engine::apply_segments(&self.store, &segments, &self.cloud)?;
        Ok(report)
    }
}

/// A set of devices belonging to one user, plus the cloud between them.
#[derive(Debug)]
pub struct Swarm {
    devices: Vec<SimDevice>,
    cloud: CloudHandle,
    // Held so the temporary directories outlive the stores that use them.
    _directories: Vec<tempfile::TempDir>,
}

impl Swarm {
    /// Build a swarm of `count` devices for one freshly derived user.
    pub fn new(count: usize) -> Result<Self> {
        Self::with_master(count, &MasterSecret::from_bytes([0x5c; 32]))
    }

    /// Build a swarm for a specific user.
    ///
    /// Device keys are derived from a fixed seed rather than generated, so a
    /// failing scenario reproduces exactly — including which device wins a
    /// conflict tie-break, which depends on the device ids.
    pub fn with_master(count: usize, master: &MasterSecret) -> Result<Self> {
        let cloud = CloudHandle::default();
        let mut devices = Vec::with_capacity(count);
        let mut directories = Vec::with_capacity(count);

        for index in 0..count {
            let directory = tempfile::tempdir().map_err(|error| {
                SyncError::Source(format!(
                    "could not create a simulated device directory: {error}"
                ))
            })?;

            let seed = SecretBytes::new(blake3::derive_key(
                "itsanas simulation device seed",
                &u32::try_from(index).unwrap_or(u32::MAX).to_le_bytes(),
            ));

            let store = Store::open_for_testing(
                directory.path(),
                UserKeys::derive(master),
                DeviceKeys::from_seed(&seed),
                itsanas_store::ChunkerConfig::default(),
            )?;

            devices.push(SimDevice {
                store,
                cloud: cloud.clone(),
                online: true,
            });
            directories.push(directory);
        }

        Ok(Self {
            devices,
            cloud,
            _directories: directories,
        })
    }

    /// How many devices are in the swarm.
    #[must_use]
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// One device.
    ///
    /// # Panics
    ///
    /// If `index` is out of range. This is a test harness; an out-of-range
    /// index is a bug in the scenario, not a runtime condition to handle.
    #[must_use]
    pub fn device(&self, index: usize) -> &SimDevice {
        &self.devices[index]
    }

    /// The shared cloud.
    #[must_use]
    pub const fn cloud(&self) -> &CloudHandle {
        &self.cloud
    }

    /// Bring a device online or take it offline.
    ///
    /// # Panics
    ///
    /// If `index` is out of range.
    pub fn set_online(&mut self, index: usize, online: bool) {
        self.devices[index].online = online;
    }

    /// Publish from every online device.
    pub fn publish_all(&self) -> Result<()> {
        for device in &self.devices {
            device.publish()?;
        }
        Ok(())
    }

    /// Run sync rounds until nothing changes.
    ///
    /// Publishing and syncing interleave, because a device that adopts a peer's
    /// work must then republish so a *third* device can learn it from a host
    /// the second one can reach. Bounded so a non-converging bug fails the test
    /// rather than hanging CI forever.
    pub fn settle(&self) -> Result<usize> {
        const MAX_ROUNDS: usize = 32;

        for round in 1..=MAX_ROUNDS {
            let mut changed = false;

            for device in &self.devices {
                if device.publish()? {
                    changed = true;
                }
            }
            for device in &self.devices {
                let report = device.sync()?;
                if report.changed_anything() || report.needs_another_round() {
                    changed = true;
                }
            }

            if !changed {
                return Ok(round);
            }
        }

        Err(SyncError::Source(format!(
            "the swarm did not settle within {MAX_ROUNDS} rounds; the merge \
             rules are not converging"
        )))
    }

    /// Publish, settle, and assert every online device agrees.
    pub fn settle_and_check(&self) -> Result<usize> {
        let rounds = self.settle()?;
        let divergences = self.divergences()?;

        if divergences.is_empty() {
            return Ok(rounds);
        }

        Err(SyncError::Source(format!(
            "devices did not converge after {rounds} rounds: {divergences:?}"
        )))
    }

    /// Every way in which any two online devices disagree.
    pub fn divergences(&self) -> Result<Vec<(usize, usize, Divergence)>> {
        let online: Vec<usize> = (0..self.devices.len())
            .filter(|index| self.devices[*index].online)
            .collect();

        let mut out = Vec::new();
        for pair in online.windows(2) {
            let (left, right) = (pair[0], pair[1]);
            for divergence in engine::diff(&self.devices[left].store, &self.devices[right].store)? {
                out.push((left, right, divergence));
            }
        }
        Ok(out)
    }

    /// The set of paths every online device agrees on, or an error naming the
    /// first disagreement.
    pub fn agreed_listing(&self) -> Result<Vec<String>> {
        let divergences = self.divergences()?;
        if !divergences.is_empty() {
            return Err(SyncError::Source(format!(
                "devices disagree about their contents: {divergences:?}"
            )));
        }

        let first = self
            .devices
            .iter()
            .position(|device| device.online)
            .ok_or_else(|| SyncError::Source("no device is online".to_owned()))?;

        self.devices[first].list()
    }
}
