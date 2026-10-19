//! Shared test fixtures for the availability modules.

use crate::uptime::Uptime;
use itsanas_crypto::{ChunkId, DeviceId, UserId};

/// A ring attempted every hour of `start..start + hours`, seen whenever
/// `seen(hour)` says so.
pub fn ring(start: u64, hours: u64, seen: impl Fn(u64) -> bool) -> Uptime {
    let mut uptime = Uptime::new(start);
    for hour in start..start + hours {
        uptime.note(hour, seen(hour));
    }
    uptime
}

/// A ring seen `per_mille` of its hours (to the 10 ‰) over the 600 hours
/// ending just before `now`. Rings with phases 50 apart and ≥ 500 ‰ never miss
/// the same hour, so they behave as independent or better.
pub fn steady(now: u64, per_mille: u64, phase: u64) -> Uptime {
    ring(now - 600, 600, |hour| {
        ((hour + phase) % 100) * 10 < per_mille
    })
}

pub fn device(index: u16) -> DeviceId {
    let mut bytes = [0u8; 32];
    bytes[..2].copy_from_slice(&index.to_le_bytes());
    // Spread the rest so ids are not near-identical, which would make a
    // sorting bug invisible.
    let filler = blake3::hash(&index.to_le_bytes());
    bytes[2..].copy_from_slice(&filler.as_bytes()[..30]);
    DeviceId::from_bytes(bytes)
}

pub fn user(index: u8) -> UserId {
    UserId::from_bytes([index; 32])
}

pub fn chunk(index: u32) -> ChunkId {
    ChunkId::from_bytes(*blake3::hash(&index.to_le_bytes()).as_bytes())
}
