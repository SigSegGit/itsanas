//! Which of this machine's networks are real, and which a hypervisor or a
//! VPN made up.
//!
//! The hand test of 2026-10-22: a Windows laptop with VMware and Hyper-V
//! adapters heard the portable key that ran on it at 192.168.1.142, at
//! 192.168.19.1 and at 192.168.117.1, one copy of each beacon per adapter,
//! and its log said the key had "moved" at every one. The two others are host
//! adapters of virtual networks: no machine of another house is behind them,
//! and an address on one is not where anybody else should dial this machine.
//!
//! An address alone cannot say so. VMware, Hyper-V, VirtualBox and WSL all
//! number their networks out of the same private ranges a home router uses,
//! so the adapter's *name* is what decides ([`is_virtual_adapter`]), plus the
//! address ranges Tailscale-like overlays take for themselves. Enumerating
//! the adapters needs the operating system (`if-addrs`: `getifaddrs` on Unix,
//! `GetAdaptersAddresses` on Windows), so this crate's unsafe-free rule holds
//! for its own code only; the dependency parses nothing that arrives from the
//! network.

use std::net::{IpAddr, Ipv4Addr};

/// Fragments of an adapter's name that mark it virtual, compared in lower
/// case: Windows's friendly names ("vEthernet (WSL)", "VMware Network Adapter
/// VMnet8", "VirtualBox Host-Only Network", "Tailscale"), and the Unix
/// interface names the same tools create (`vmnet1`, `vboxnet0`, `docker0`,
/// `virbr0`, `tailscale0`, `utun3`).
const VIRTUAL_NAMES: &[&str] = &[
    "vethernet",
    "hyper-v",
    "vmware",
    "vmnet",
    "virtualbox",
    "vboxnet",
    "wsl",
    "tailscale",
    "zerotier",
    "wireguard",
    "docker",
    "virbr",
    "veth",
    "utun",
    "lxcbr",
    "lxdbr",
];

/// Prefixes of Unix interface names that mark them virtual but would match
/// too much as fragments: `br-1a2b` is a Docker network, `zt0` ZeroTier,
/// `wg0` WireGuard.
const VIRTUAL_PREFIXES: &[&str] = &["br-", "zt", "wg"];

/// Whether an adapter of this name, holding `ip`, is a virtual network: a
/// hypervisor's host adapter, a container bridge, or an overlay VPN.
///
/// By address as well as by name for the overlays, which pick ranges no home
/// router hands out: 100.64.0.0/10 (shared address space, Tailscale's IPv4)
/// and `fd7a:115c:a1e0::/48` (Tailscale's IPv6), whatever the adapter is called.
#[must_use]
pub fn is_virtual_adapter(name: &str, ip: IpAddr) -> bool {
    let lower = name.to_ascii_lowercase();
    let overlay = match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            a == 100 && (b & 0xc0) == 64
        }
        IpAddr::V6(v6) => v6.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    };
    overlay
        || VIRTUAL_NAMES
            .iter()
            .any(|fragment| lower.contains(fragment))
        || VIRTUAL_PREFIXES
            .iter()
            .any(|prefix| lower.starts_with(prefix))
}

/// One address of this machine, on the network its mask spans.
#[derive(Clone, Debug, PartialEq, Eq)]
struct LocalNet {
    ip: IpAddr,
    netmask: IpAddr,
    virtual_adapter: bool,
}

impl LocalNet {
    fn spans(&self, other: IpAddr) -> bool {
        match (self.ip, self.netmask, other) {
            (IpAddr::V4(ip), IpAddr::V4(mask), IpAddr::V4(other)) => {
                let mask = u32::from(mask);
                u32::from(ip) & mask == u32::from(other) & mask
            }
            (IpAddr::V6(ip), IpAddr::V6(mask), IpAddr::V6(other)) => {
                let mask = u128::from(mask);
                u128::from(ip) & mask == u128::from(other) & mask
            }
            _ => false,
        }
    }
}

/// This machine's addresses, each marked real or virtual.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalNets {
    nets: Vec<LocalNet>,
}

impl LocalNets {
    /// The adapters of this machine as the operating system lists them now.
    /// Empty when it cannot: every question then answers as it did before
    /// this module existed, which is the safe way to be wrong.
    #[must_use]
    pub fn of_this_machine() -> Self {
        let Ok(interfaces) = if_addrs::get_if_addrs() else {
            return Self::default();
        };
        Self::from_interfaces(interfaces.into_iter().map(|interface| {
            let (ip, netmask) = match interface.addr {
                if_addrs::IfAddr::V4(v4) => (IpAddr::V4(v4.ip), IpAddr::V4(v4.netmask)),
                if_addrs::IfAddr::V6(v6) => (IpAddr::V6(v6.ip), IpAddr::V6(v6.netmask)),
            };
            (interface.name, ip, netmask)
        }))
    }

    /// From `(adapter name, address, netmask)` triples: what
    /// [`Self::of_this_machine`] reads, and what a test makes up.
    #[must_use]
    pub fn from_interfaces(interfaces: impl IntoIterator<Item = (String, IpAddr, IpAddr)>) -> Self {
        Self {
            nets: interfaces
                .into_iter()
                .filter(|(_, ip, _)| !ip.is_loopback())
                .map(|(name, ip, netmask)| LocalNet {
                    virtual_adapter: is_virtual_adapter(&name, ip),
                    ip,
                    netmask,
                })
                .collect(),
        }
    }

    /// Whether a datagram from `source` came in over a virtual network: the
    /// source lies on one of this machine's virtual networks and on none of
    /// its real ones. False for anything else, including when nothing is
    /// known about the adapters.
    #[must_use]
    pub fn via_virtual(&self, source: IpAddr) -> bool {
        let on = |virtual_adapter: bool| {
            self.nets
                .iter()
                .any(|net| net.virtual_adapter == virtual_adapter && net.spans(source))
        };
        on(true) && !on(false)
    }

    /// Whether `local` is an address of one of this machine's virtual
    /// adapters.
    #[must_use]
    pub fn is_virtual(&self, local: IpAddr) -> bool {
        self.nets
            .iter()
            .any(|net| net.virtual_adapter && net.ip == local)
    }

    /// The first IPv4 address of a real adapter that a neighbour could dial:
    /// not loopback, not link-local.
    #[must_use]
    pub fn physical_v4(&self) -> Option<Ipv4Addr> {
        self.nets.iter().find_map(|net| match net.ip {
            IpAddr::V4(v4) if !net.virtual_adapter && !v4.is_link_local() => Some(v4),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    /// The laptop of the hand test: its Wi-Fi, a VMware host-only and NAT
    /// pair, WSL's Hyper-V switch, and Tailscale.
    fn hand_test_laptop() -> LocalNets {
        let mask = v4("255.255.255.0");
        LocalNets::from_interfaces([
            ("Wi-Fi".to_owned(), v4("192.168.1.142"), mask),
            (
                "VMware Network Adapter VMnet1".to_owned(),
                v4("192.168.19.1"),
                mask,
            ),
            (
                "VMware Network Adapter VMnet8".to_owned(),
                v4("192.168.117.1"),
                mask,
            ),
            (
                "vEthernet (WSL)".to_owned(),
                v4("172.27.160.1"),
                v4("255.255.240.0"),
            ),
            (
                "Tailscale".to_owned(),
                v4("100.101.102.103"),
                v4("255.192.0.0"),
            ),
            (
                "Loopback Pseudo-Interface 1".to_owned(),
                v4("127.0.0.1"),
                v4("255.0.0.0"),
            ),
        ])
    }

    /// Every adapter the hand test's laptop had, and the Unix names the same
    /// tools use, are told apart from a real network card -- by name, since
    /// their addresses come from the ranges a home router uses too.
    #[test]
    fn red_team_virtual_adapters_are_told_from_real_ones_by_name() {
        let lan = v4("192.168.1.142");
        for name in [
            "vEthernet (WSL)",
            "vEthernet (Default Switch)",
            "VMware Network Adapter VMnet1",
            "VirtualBox Host-Only Network",
            "Tailscale",
            "ZeroTier One [8056c2e21c000001]",
            "vmnet8",
            "vboxnet0",
            "docker0",
            "br-1a2b3c4d5e6f",
            "virbr0",
            "veth12ab",
            "tailscale0",
            "utun3",
            "wg0",
            "zt5u4y5yvb",
        ] {
            assert!(
                is_virtual_adapter(name, lan),
                "{name} was taken for a real network"
            );
        }
        for name in [
            "Wi-Fi",
            "Ethernet",
            "Ethernet 2",
            "eth0",
            "wlan0",
            "en0",
            "enp3s0",
            "wlp2s0",
        ] {
            assert!(
                !is_virtual_adapter(name, lan),
                "{name} was taken for a virtual network"
            );
        }
        assert!(
            is_virtual_adapter("Ethernet 3", v4("100.101.102.103")),
            "a Tailscale address under another name is still an overlay"
        );
        assert!(is_virtual_adapter(
            "en5",
            "fd7a:115c:a1e0::1".parse().unwrap()
        ));
    }

    #[test]
    fn red_team_a_beacon_over_a_virtual_network_is_known_for_one() {
        let laptop = hand_test_laptop();
        assert!(
            !laptop.via_virtual(v4("192.168.1.142")),
            "the Wi-Fi is real"
        );
        assert!(
            !laptop.via_virtual(v4("192.168.1.20")),
            "the Wi-Fi's neighbours are real"
        );
        assert!(laptop.via_virtual(v4("192.168.19.1")));
        assert!(laptop.via_virtual(v4("192.168.117.1")));
        assert!(
            laptop.via_virtual(v4("172.27.170.4")),
            "a WSL guest is behind Hyper-V"
        );
        assert!(
            !laptop.via_virtual(v4("10.0.0.7")),
            "on no network of this machine: unknown"
        );
        assert!(
            !LocalNets::default().via_virtual(v4("192.168.19.1")),
            "with no adapters listed, nothing is called virtual"
        );
        // A real network inside a virtual one's range -- Docker's /16 over
        // an office LAN numbered 172.17.5.x -- is a real network.
        let overlap = LocalNets::from_interfaces([
            ("docker0".to_owned(), v4("172.17.0.1"), v4("255.255.0.0")),
            ("eth0".to_owned(), v4("172.17.5.10"), v4("255.255.255.0")),
        ]);
        assert!(
            overlap.via_virtual(v4("172.17.0.2")),
            "a container is behind Docker"
        );
        assert!(
            !overlap.via_virtual(v4("172.17.5.20")),
            "a neighbour on the real LAN was taken for a container"
        );
    }

    #[test]
    fn the_address_to_announce_is_on_a_real_adapter() {
        let laptop = hand_test_laptop();
        assert!(laptop.is_virtual(v4("192.168.19.1")));
        assert!(laptop.is_virtual(v4("100.101.102.103")));
        assert!(!laptop.is_virtual(v4("192.168.1.142")));
        assert_eq!(laptop.physical_v4(), Some("192.168.1.142".parse().unwrap()));
    }

    /// Whatever this machine has, reading it does not fail and the loopback
    /// is never listed.
    #[test]
    fn this_machines_adapters_can_be_read() {
        let here = LocalNets::of_this_machine();
        assert!(!here.is_virtual(v4("127.0.0.1")));
    }
}
