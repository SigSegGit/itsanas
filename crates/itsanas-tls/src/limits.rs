//! How many connections a listener serves at once, and from whom.
//!
//! Both listeners in this project -- a node's and the coordinator's -- serve
//! one thread per connection, and both can be dialled by strangers. A thread
//! is a few megabytes, so an unbounded number of them is a way to switch a
//! machine off; a single global cap is a way for one address to switch it off
//! for everyone else. The counts here are what both check before spawning.
//!
//! A slot is returned when its [`Slot`] is dropped, including when the thread
//! serving it panics. A slot that leaked would be a limit that shrank, one
//! connection at a time, until the listener served nobody.

use std::{
    collections::HashMap,
    hash::Hash,
    net::{IpAddr, Ipv6Addr},
    sync::{Mutex, PoisonError},
};

use itsanas_crypto::DeviceId;

/// Caps, and the live counts they are checked against.
#[derive(Debug)]
pub struct ConnectionLimits {
    total: usize,
    per_address: usize,
    per_device: usize,
    tally: Mutex<Tally>,
}

#[derive(Debug, Default)]
struct Tally {
    live: usize,
    by_address: HashMap<IpAddr, usize>,
    by_device: HashMap<DeviceId, usize>,
}

impl ConnectionLimits {
    /// At most `total` connections, `per_address` from one IP address and
    /// `per_device` for one proven device key.
    #[must_use]
    pub fn new(total: usize, per_address: usize, per_device: usize) -> Self {
        Self {
            total,
            per_address,
            per_device,
            tally: Mutex::new(Tally::default()),
        }
    }

    /// A slot for a new connection from `address`, if the caps allow one.
    ///
    /// Checked before the handshake, because the handshake is the expensive
    /// part and a caller that has not finished one has proved nothing.
    pub fn admit(&self, address: IpAddr) -> Option<Slot<'_>> {
        let address = counted_as(address);
        let mut tally = self.tally.lock().unwrap_or_else(PoisonError::into_inner);
        let from_here = tally.by_address.get(&address).copied().unwrap_or(0);
        if tally.live >= self.total || from_here >= self.per_address {
            return None;
        }
        tally.live += 1;
        tally.by_address.insert(address, from_here + 1);
        Some(Slot {
            limits: self,
            address,
            device: None,
        })
    }

    /// Connections currently held.
    #[must_use]
    pub fn live(&self) -> usize {
        self.tally
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .live
    }
}

/// One admitted connection. Dropping it gives everything back.
#[derive(Debug)]
pub struct Slot<'a> {
    limits: &'a ConnectionLimits,
    address: IpAddr,
    device: Option<DeviceId>,
}

impl Slot<'_> {
    /// Count this connection against the device it proved to be.
    ///
    /// False when that device already holds its share. Calling it a second
    /// time on the same slot changes nothing and answers as the first did.
    pub fn claim_device(&mut self, device: DeviceId) -> bool {
        if self.device.is_some() {
            return self.device == Some(device);
        }
        let mut tally = self
            .limits
            .tally
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let held = tally.by_device.get(&device).copied().unwrap_or(0);
        if held >= self.limits.per_device {
            return false;
        }
        tally.by_device.insert(device, held + 1);
        self.device = Some(device);
        true
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        // Recovered from a poisoned lock rather than skipped: the counts are
        // plain integers, and a panic elsewhere does not make them wrong.
        let mut tally = self
            .limits
            .tally
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        tally.live = tally.live.saturating_sub(1);
        release(&mut tally.by_address, &self.address);
        if let Some(device) = self.device {
            release(&mut tally.by_device, &device);
        }
    }
}

/// The address a connection is counted against.
///
/// An IPv6 address is counted by its /64. A household is given a whole /64 --
/// Free hands every subscriber one -- so counting single addresses would let
/// one machine appear as eighteen quintillion callers and the per-address cap
/// would stop nobody. An IPv4 address carried inside IPv6 (a dual-stack
/// listener on `[::]` sees `::ffff:a.b.c.d`) is counted as the IPv4 address it
/// is, so the same caller is one caller whichever way it arrived.
#[must_use]
pub fn counted_as(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V4(_) => address,
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or_else(
            || IpAddr::V6(Ipv6Addr::from(u128::from(v6) & !((1u128 << 64) - 1))),
            IpAddr::V4,
        ),
    }
}

fn release<K: Hash + Eq>(counts: &mut HashMap<K, usize>, key: &K) {
    if let Some(count) = counts.get_mut(key) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            counts.remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, last))
    }

    /// The failure this module exists to prevent: one address holding every
    /// slot, so the listener is up and serves nobody else.
    #[test]
    fn red_team_one_address_cannot_take_every_slot() {
        let limits = ConnectionLimits::new(32, 8, 4);
        let flood: Vec<_> = std::iter::from_fn(|| limits.admit(ip(66)))
            .take(100)
            .collect();
        assert_eq!(
            flood.len(),
            8,
            "one address held {} connections; a single machine could shut \
             everybody else out of this listener",
            flood.len()
        );
        assert!(
            limits.admit(ip(7)).is_some(),
            "another address was refused while one address flooded the listener"
        );
    }

    /// THE ATTACK: the same flood from IPv6, where one subscriber owns a /64
    /// and can give every connection its own source address.
    #[test]
    fn red_team_one_ipv6_subnet_cannot_take_every_slot_by_changing_address() {
        let limits = ConnectionLimits::new(32, 8, 4);
        let flood: Vec<_> = (1..=100u128)
            .filter_map(|n| {
                let address = (0x2a01_0e0a_0001_0002u128 << 64) | n;
                limits.admit(IpAddr::V6(Ipv6Addr::from(address)))
            })
            .collect();
        assert_eq!(
            flood.len(),
            8,
            "one /64 held {} connections by using a new address each time; \
             the per-address cap stops nobody on IPv6",
            flood.len()
        );
    }

    #[test]
    fn an_ipv4_caller_is_one_caller_however_it_arrives() {
        let limits = ConnectionLimits::new(32, 1, 4);
        let _plain = limits.admit(ip(5)).expect("first");
        let mapped = IpAddr::V6(Ipv4Addr::new(203, 0, 113, 5).to_ipv6_mapped());
        assert!(
            limits.admit(mapped).is_none(),
            "an IPv4 caller seen through a dual-stack socket was counted twice"
        );
    }

    #[test]
    fn the_overall_cap_holds_across_addresses() {
        let limits = ConnectionLimits::new(4, 8, 4);
        let held: Vec<_> = (0..10).filter_map(|n| limits.admit(ip(n))).collect();
        assert_eq!(held.len(), 4, "more connections than the cap were admitted");
    }

    /// A slot that is not handed back makes the limit shrink for ever, and the
    /// listener ends up refusing everyone while serving nobody.
    #[test]
    fn a_closed_connection_gives_its_slot_back_even_after_a_panic() {
        let limits = ConnectionLimits::new(1, 1, 1);
        {
            let _slot = limits.admit(ip(1)).expect("first connection admitted");
            assert!(limits.admit(ip(1)).is_none());
        }
        assert_eq!(limits.live(), 0, "a dropped slot was not returned");

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut slot = limits.admit(ip(1)).expect("admitted again");
            assert!(slot.claim_device(DeviceId::from_bytes([1; 32])));
            panic!("a connection thread fails mid-request");
        }));
        assert!(outcome.is_err());
        assert_eq!(
            limits.live(),
            0,
            "a panicking connection kept its slot; the listener would shrink \
             to nothing one crash at a time"
        );
        let mut again = limits.admit(ip(1)).expect("the slot came back");
        assert!(
            again.claim_device(DeviceId::from_bytes([1; 32])),
            "the device count leaked when its connection panicked"
        );
    }

    #[test]
    fn red_team_one_device_key_cannot_hold_more_than_its_share() {
        let limits = ConnectionLimits::new(32, 32, 2);
        let device = DeviceId::from_bytes([9; 32]);
        let mut slots: Vec<_> = (0..3).filter_map(|n| limits.admit(ip(n))).collect();
        let granted = slots
            .iter_mut()
            .filter_map(|slot| slot.claim_device(device).then_some(()))
            .count();
        assert_eq!(
            granted, 2,
            "one proven device held {granted} connections from different addresses"
        );
    }
}
