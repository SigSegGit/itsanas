//! NAT-PMP (RFC 6886): the router's port opened for routers that speak no
//! `UPnP` IGD, HANDOVER §8 0u.
//!
//! Tried after `UPnP` fails (`daemon.rs`). Two UDP requests to the gateway on
//! port 5351: one for the public address, one to map the listening TCP port.
//! PCP (RFC 6887) answers version-0 requests with a version error, so a PCP
//! router is not used here; it is the next step if the fleet shows one.
//!
//! The gateway is not discovered by broadcast as in `UPnP`: on Linux it is the
//! default route in `/proc/net/route`; elsewhere, and as a fallback, the `.1`
//! and `.254` of this machine's `/24`, the two addresses home boxes use. Only
//! an answer from the address asked is read, which stops a confused or
//! careless machine on the LAN, not a hostile one: a source address on a LAN
//! can be forged. What that buys an attacker is a wrong announced address,
//! never a connection: peers pin the device id, and a dial that reaches
//! another machine is refused.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

/// The NAT-PMP server port.
const PORT: u16 = 5351;

/// The lifetime asked for; renewed at half of what the router grants.
const LIFETIME: u32 = 3600;

/// Waits per try, doubling as RFC 6886 asks, kept short: a daemon starting
/// must not hang on a router that does not speak this.
const WAITS: [Duration; 2] = [Duration::from_millis(250), Duration::from_millis(500)];

/// The public address in an answer to opcode 0, if the answer is one.
pub(crate) fn parse_public(reply: &[u8]) -> Option<Ipv4Addr> {
    if reply.len() < 12 || reply[0] != 0 || reply[1] != 128 || reply[2..4] != [0, 0] {
        return None;
    }
    Some(Ipv4Addr::new(reply[8], reply[9], reply[10], reply[11]))
}

/// The external port and lifetime in an answer to a TCP mapping (opcode 2)
/// for `internal`, if the answer is one and grants it.
pub(crate) fn parse_mapping(reply: &[u8], internal: u16) -> Option<(u16, u32)> {
    if reply.len() < 16 || reply[0] != 0 || reply[1] != 130 || reply[2..4] != [0, 0] {
        return None;
    }
    if u16::from_be_bytes([reply[8], reply[9]]) != internal {
        return None;
    }
    let external = u16::from_be_bytes([reply[10], reply[11]]);
    let lifetime = u32::from_be_bytes([reply[12], reply[13], reply[14], reply[15]]);
    (external != 0 && lifetime != 0).then_some((external, lifetime))
}

fn mapping_request(port: u16, lifetime: u32) -> [u8; 12] {
    let mut out = [0u8; 12];
    out[1] = 2; // map TCP
    out[4..6].copy_from_slice(&port.to_be_bytes());
    out[6..8].copy_from_slice(&port.to_be_bytes());
    out[8..12].copy_from_slice(&lifetime.to_be_bytes());
    out
}

/// One request, an answer only from `gateway:5351`.
fn ask(gateway: Ipv4Addr, request: &[u8]) -> Option<Vec<u8>> {
    ask_at(SocketAddr::V4(SocketAddrV4::new(gateway, PORT)), request)
}

fn ask_at(target: SocketAddr, request: &[u8]) -> Option<Vec<u8>> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    for wait in WAITS {
        socket.send_to(request, target).ok()?;
        let deadline = Instant::now() + wait;
        let mut buffer = [0u8; 64];
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            socket
                .set_read_timeout(Some(left.max(Duration::from_millis(1))))
                .ok()?;
            match socket.recv_from(&mut buffer) {
                Ok((size, from)) if from == target => return Some(buffer[..size].to_vec()),
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }
    None
}

/// The default gateway from `/proc/net/route` (little-endian hex), Linux.
pub(crate) fn proc_route_gateway(table: &str) -> Option<Ipv4Addr> {
    table.lines().skip(1).find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 3 || fields[1] != "00000000" {
            return None;
        }
        let raw = u32::from_str_radix(fields[2], 16).ok()?;
        let gateway = Ipv4Addr::from(raw.to_le_bytes());
        (!gateway.is_unspecified()).then_some(gateway)
    })
}

/// The `.1` and `.254` of `local`'s `/24`.
pub(crate) fn guesses(local: Ipv4Addr) -> [Ipv4Addr; 2] {
    let [a, b, c, _] = local.octets();
    [Ipv4Addr::new(a, b, c, 1), Ipv4Addr::new(a, b, c, 254)]
}

fn gateways() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    if let Some(gateway) = std::fs::read_to_string("/proc/net/route")
        .ok()
        .and_then(|table| proc_route_gateway(&table))
    {
        out.push(gateway);
    }
    let local = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .and_then(|socket| {
            socket.connect(("192.0.2.1", 9))?;
            socket.local_addr()
        })
        .ok()
        .and_then(|address| match address.ip() {
            std::net::IpAddr::V4(v4) if v4.is_private() => Some(v4),
            _ => None,
        });
    if let Some(local) = local {
        for guess in guesses(local) {
            if !out.contains(&guess) && guess != local {
                out.push(guess);
            }
        }
    }
    out
}

/// What was opened: where, and for how long the router granted it.
pub(crate) struct Mapping {
    pub(crate) gateway: Ipv4Addr,
    pub(crate) address: String,
    pub(crate) lifetime: u32,
}

/// Map `port` and return the mapping, or why not.
pub(crate) fn open_port(port: u16) -> Result<Mapping, String> {
    for gateway in gateways() {
        let Some((external, lifetime)) = ask(gateway, &mapping_request(port, LIFETIME))
            .and_then(|reply| parse_mapping(&reply, port))
        else {
            continue;
        };
        let Some(public) = ask(gateway, &[0, 0]).and_then(|reply| parse_public(&reply)) else {
            continue;
        };
        if !crate::upnp::is_public(public) {
            // Behind another router: the mapping is useless, give it back.
            let _ = ask(gateway, &mapping_request(port, 0));
            return Err("the router has no public address (it is behind another router)".into());
        }
        return Ok(Mapping {
            gateway,
            address: format!("{public}:{external}"),
            lifetime,
        });
    }
    Err("no router answered NAT-PMP either".to_owned())
}

/// When to renew a mapping the router granted for `lifetime` seconds: at
/// half of it, as RFC 6886 asks, and never sooner than a minute.
pub(crate) fn renew_after(lifetime: u32) -> Duration {
    Duration::from_secs(u64::from(lifetime / 2).max(60))
}

/// Keep the mapping alive until `stop`, at half the lifetime the router
/// grants each time; the announced address follows the router's (a new
/// public address after a reboot, a new external port). Given back on stop.
pub(crate) fn renew(
    mut mapping: Mapping,
    port: u16,
    stop: &std::sync::atomic::AtomicBool,
    announce: impl Fn(String),
) {
    let mut last = Instant::now();
    while !stop.load(std::sync::atomic::Ordering::SeqCst) {
        std::thread::sleep(Duration::from_secs(1));
        if last.elapsed() < renew_after(mapping.lifetime) {
            continue;
        }
        last = Instant::now();
        let renewed = ask(mapping.gateway, &mapping_request(port, LIFETIME))
            .and_then(|reply| parse_mapping(&reply, port));
        let public = ask(mapping.gateway, &[0, 0]).and_then(|reply| parse_public(&reply));
        match (renewed, public) {
            (Some((external, lifetime)), Some(public)) => {
                mapping.lifetime = lifetime;
                let address = format!("{public}:{external}");
                if address != mapping.address {
                    announce(address.clone());
                    mapping.address = address;
                }
            }
            _ => eprintln!("itsanas: the router stopped renewing the opened port (NAT-PMP)"),
        }
    }
    let _ = ask(mapping.gateway, &mapping_request(port, 0));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public_reply(result: u16, ip: [u8; 4]) -> Vec<u8> {
        let mut out = vec![0, 128];
        out.extend_from_slice(&result.to_be_bytes());
        out.extend_from_slice(&7u32.to_be_bytes());
        out.extend_from_slice(&ip);
        out
    }

    fn mapping_reply(op: u8, result: u16, internal: u16, external: u16, life: u32) -> Vec<u8> {
        let mut out = vec![0, op];
        out.extend_from_slice(&result.to_be_bytes());
        out.extend_from_slice(&7u32.to_be_bytes());
        out.extend_from_slice(&internal.to_be_bytes());
        out.extend_from_slice(&external.to_be_bytes());
        out.extend_from_slice(&life.to_be_bytes());
        out
    }

    #[test]
    fn red_team_only_a_granting_answer_opens_a_port_or_names_an_address() {
        assert_eq!(
            parse_public(&public_reply(0, [203, 0, 113, 9])),
            Some(Ipv4Addr::new(203, 0, 113, 9))
        );
        assert_eq!(
            parse_mapping(&mapping_reply(130, 0, 9797, 41000, 3600), 9797),
            Some((41000, 3600)),
            "the router's own external port must be announced, not ours"
        );
        for (reply, why) in [
            (
                public_reply(3, [203, 0, 113, 9]),
                "a refusal (result code 3)",
            ),
            (
                public_reply(0, [1, 2, 3, 4])[..11].to_vec(),
                "a truncated answer",
            ),
        ] {
            assert_eq!(parse_public(&reply), None, "{why} gave a public address");
        }
        for (reply, why) in [
            (
                mapping_reply(130, 2, 9797, 9797, 3600),
                "a refusal (result code 2)",
            ),
            (
                mapping_reply(129, 0, 9797, 9797, 3600),
                "a UDP mapping's answer",
            ),
            (
                mapping_reply(130, 0, 1234, 9797, 3600),
                "a mapping of another port",
            ),
            (
                mapping_reply(130, 0, 9797, 9797, 0),
                "a deletion (lifetime 0)",
            ),
        ] {
            assert_eq!(
                parse_mapping(&reply, 9797),
                None,
                "{why} was taken for an open port: the node would announce an address \
                 nobody can dial"
            );
        }
    }

    #[test]
    fn red_team_an_answer_from_another_machine_on_the_lan_is_ignored() {
        let router = UdpSocket::bind("127.0.0.1:0").unwrap();
        let liar = UdpSocket::bind("127.0.0.1:0").unwrap();
        let target = router.local_addr().unwrap();
        let answering = std::thread::spawn(move || {
            let mut buffer = [0u8; 16];
            let (_, asker) = router.recv_from(&mut buffer).unwrap();
            // Somebody else on the LAN answers first, with its own address.
            liar.send_to(&public_reply(0, [198, 51, 100, 66]), asker)
                .unwrap();
        });
        assert_eq!(
            ask_at(target, &[0, 0]),
            None,
            "an answer that did not come from the router was read: any machine on the LAN \
             could make this node announce an address of its choosing"
        );
        answering.join().unwrap();
    }

    #[test]
    fn the_gateway_is_read_from_the_routing_table_or_guessed_on_the_lan() {
        let table = "Iface\tDestination\tGateway \tFlags\n\
                     eth0\t0001A8C0\t00000000\t0001\n\
                     eth0\t00000000\tFE01A8C0\t0003\n";
        assert_eq!(
            proc_route_gateway(table),
            Some(Ipv4Addr::new(192, 168, 1, 254))
        );
        assert_eq!(
            guesses(Ipv4Addr::new(192, 168, 1, 20)),
            [
                Ipv4Addr::new(192, 168, 1, 1),
                Ipv4Addr::new(192, 168, 1, 254)
            ]
        );
        assert_eq!(mapping_request(9797, 3600)[..2], [0, 2]);
        assert_eq!(
            renew_after(120),
            Duration::from_secs(60),
            "a short lease lapsed before renewal"
        );
        assert_eq!(renew_after(3600), Duration::from_secs(1800));
        assert_eq!(renew_after(10), Duration::from_secs(60));
    }
}
