//! "Same network" as a proxy for hosts that go dark together (§8 7c).
//!
//! The empirical half of [`crate::miss_ppm`] sees correlation only once it has
//! happened, and only over hours this node tried every holder: two machines in
//! one house, both on for three weeks, read as independent until the first
//! power cut. Where a host was last seen *from* is evidence that arrives before
//! that, so hosts seen from one network count for availability as **one**
//! unit, as this account's own devices already do. It can only ever ask for
//! more copies, and [`crate::Goal::cap`] still bounds them.
//!
//! What a [`Network`] is, and is not:
//!
//! - **A prefix, never an address.** IPv4 /24, IPv6 /48 (a site), a private or
//!   unique-local range by its own /24 or /64, so two houses that both use
//!   `192.168.1.0/24` are still one key for this observer only: the key is
//!   this node's view, used by this node, never compared across nodes.
//! - **None where the address says nothing about a house.** Loopback (another
//!   node on this machine, or a test), unspecified, link local, and
//!   100.64.0.0/10: carrier-grade NAT and every overlay (Tailscale hands it
//!   out), where one "network" spans the world. Grouping those would make a
//!   whole tailnet one unit and ask every chunk for the cap.
//! - **Not serialisable.** It has no serde derive on purpose: it must not
//!   reach the wire, a claim or the coordinator. The store writes its bytes
//!   itself ([`Network::to_bytes`]).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Length of [`Network::to_bytes`]: one tag byte, then up to 16 prefix bytes.
pub const NETWORK_BYTES: usize = 17;

const PUBLIC_V4: u8 = 1;
const PRIVATE_V4: u8 = 2;
const PUBLIC_V6: u8 = 3;
const UNIQUE_LOCAL_V6: u8 = 4;

/// The network a host was seen from, as this node observed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Network([u8; NETWORK_BYTES]);

impl Network {
    /// The network `ip` belongs to, or `None` where an address names no house.
    #[must_use]
    pub fn of(ip: IpAddr) -> Option<Self> {
        match ip {
            IpAddr::V4(v4) => Self::of_v4(v4),
            // An IPv4 caller on a dual-stack socket is that IPv4 address.
            IpAddr::V6(v6) => v6
                .to_ipv4_mapped()
                .map_or_else(|| Self::of_v6(v6), Self::of_v4),
        }
    }

    fn of_v4(v4: Ipv4Addr) -> Option<Self> {
        let [a, b, c, _] = v4.octets();
        let overlay = a == 100 && (64..128).contains(&b);
        if v4.is_loopback()
            || v4.is_unspecified()
            || v4.is_broadcast()
            || v4.is_link_local()
            || v4.is_multicast()
            || overlay
        {
            return None;
        }
        let tag = if v4.is_private() {
            PRIVATE_V4
        } else {
            PUBLIC_V4
        };
        let mut bytes = [0; NETWORK_BYTES];
        bytes[0] = tag;
        bytes[1..4].copy_from_slice(&[a, b, c]);
        Some(Self(bytes))
    }

    fn of_v6(v6: Ipv6Addr) -> Option<Self> {
        let octets = v6.octets();
        let link_local = octets[0] == 0xfe && octets[1] & 0xc0 == 0x80;
        if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || link_local {
            return None;
        }
        let mut bytes = [0; NETWORK_BYTES];
        if octets[0] & 0xfe == 0xfc {
            bytes[0] = UNIQUE_LOCAL_V6;
            bytes[1..9].copy_from_slice(&octets[..8]);
        } else {
            bytes[0] = PUBLIC_V6;
            bytes[1..7].copy_from_slice(&octets[..6]);
        }
        Some(Self(bytes))
    }

    /// The store's encoding.
    #[must_use]
    pub const fn to_bytes(&self) -> [u8; NETWORK_BYTES] {
        self.0
    }

    /// Back from [`Self::to_bytes`]; `None` for a tag this version does not
    /// know, which then groups nothing.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let bytes: [u8; NETWORK_BYTES] = bytes.try_into().ok()?;
        matches!(
            bytes[0],
            PUBLIC_V4 | PRIVATE_V4 | PUBLIC_V6 | UNIQUE_LOCAL_V6
        )
        .then_some(Self(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(text: &str) -> Option<Network> {
        Network::of(text.parse().unwrap())
    }

    #[test]
    fn one_ipv4_slash_24_or_ipv6_slash_48_is_one_network() {
        assert_eq!(of("203.0.113.7"), of("203.0.113.200"));
        assert_ne!(of("203.0.113.7"), of("203.0.114.7"));
        assert_eq!(of("2001:db8:1:aaaa::1"), of("2001:db8:1:bbbb::2"));
        assert_ne!(of("2001:db8:1::1"), of("2001:db8:2::1"));
        assert_eq!(
            of("::ffff:203.0.113.7"),
            of("203.0.113.9"),
            "a mapped IPv4 is IPv4"
        );
        assert!(of("203.0.113.7").is_some());
    }

    #[test]
    fn a_private_range_is_never_the_public_one_with_the_same_digits() {
        // 10.1.2.0/24 private and a public /24 are different keys even if a
        // tag-less encoding would collide on the prefix bytes.
        assert_ne!(of("192.168.1.10"), of("192.168.2.10"));
        assert_eq!(of("192.168.1.10"), of("192.168.1.20"));
        assert_ne!(
            of("10.1.2.3").unwrap().to_bytes()[0],
            of("11.1.2.3").unwrap().to_bytes()[0]
        );
        assert_eq!(of("fd00:1:2:3::1"), of("fd00:1:2:3::9"));
        assert_ne!(
            of("fd00:1:2:3::1"),
            of("fd00:1:2:4::1"),
            "unique local is a /64"
        );
    }

    #[test]
    fn red_team_an_overlay_or_a_loopback_address_names_no_network() {
        // A tailnet hands every member a 100.64.0.0/10 address wherever it
        // is: one key for all of them would make a world-wide fleet one unit
        // and ask every chunk for the cap.
        for text in [
            "100.64.0.1",
            "100.101.102.103",
            "100.127.255.254",
            "127.0.0.1",
            "0.0.0.0",
            "169.254.1.1",
            "255.255.255.255",
            "::1",
            "::",
            "fe80::1",
            "::ffff:100.100.1.1",
        ] {
            assert_eq!(of(text), None, "{text} must group nothing");
        }
        assert!(
            of("100.128.0.1").is_some(),
            "just past the shared range is public"
        );
    }

    #[test]
    fn the_store_encoding_round_trips_and_an_unknown_tag_groups_nothing() {
        for text in ["203.0.113.7", "192.168.1.1", "2001:db8::1", "fd12::1"] {
            let network = of(text).unwrap();
            assert_eq!(Network::from_bytes(&network.to_bytes()), Some(network));
        }
        let mut bytes = of("203.0.113.7").unwrap().to_bytes();
        bytes[0] = 99;
        assert_eq!(Network::from_bytes(&bytes), None);
        assert_eq!(Network::from_bytes(&bytes[..4]), None);
    }
}
