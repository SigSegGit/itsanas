//! Opening this machine's port on the home router by itself (`UPnP` IGD), so a
//! member never has to configure port forwarding (HANDOVER §8 0w (6b), 5a).
//!
//! Most home routers answer `UPnP`: a multicast `M-SEARCH` finds the router,
//! its description names the `WANIPConnection` (or `WANPPPConnection`)
//! control URL, and one SOAP call maps the router's port to this machine's.
//! The router also says its public address, which becomes what this node
//! announces -- the same as a hand-written `announce`, found instead of typed.
//!
//! What this trusts: whatever answers on the LAN. A host on the same network
//! that lies can make this node announce a wrong address -- which it could
//! already do by being the router -- and nothing more: peers check who they
//! reach by device id, never by address. Every read is capped, every wait
//! bounded, and a router that does not answer costs three seconds at start.
//!
//! Not done here: NAT-PMP / PCP (some Apple and recent routers), hole
//! punching and relays (5b). A machine whose router refuses stays a machine
//! that dials out, as before.

use std::{
    io::{Read as _, Write as _},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream, UdpSocket},
    time::{Duration, Instant},
};

/// How long the router has to answer the search.
const SEARCH_FOR: Duration = Duration::from_secs(3);
/// A description or a SOAP answer is a few kilobytes; more is not one.
const MAX_REPLY: u64 = 64 * 1024;
/// The mapping's lease, renewed at half of it.
pub(crate) const LEASE: Duration = Duration::from_secs(3600);
const SERVICES: [&str; 2] = [
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:service:WANPPPConnection:1",
];

/// A router that offered to map ports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Gateway {
    /// Where the SOAP calls go.
    pub(crate) control: SocketAddrV4,
    pub(crate) path: String,
    pub(crate) service: String,
    /// This machine's address on the router's network.
    pub(crate) local: Ipv4Addr,
}

/// The `LOCATION` of a search answer, when it is an `http://` URL on the LAN.
pub(crate) fn location(answer: &str) -> Option<(SocketAddrV4, String)> {
    let line = answer
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("location:"))?;
    let url = line.split_once(':')?.1.trim();
    split_url(url)
}

/// `http://a.b.c.d:port/path` into its address and path. Only a literal IPv4
/// address: a router names itself by number, and a name would be resolved by
/// whoever answers DNS.
pub(crate) fn split_url(url: &str) -> Option<(SocketAddrV4, String)> {
    let rest = url.strip_prefix("http://")?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let address = if host.contains(':') {
        host.parse().ok()?
    } else {
        SocketAddrV4::new(host.parse().ok()?, 80)
    };
    Some((address, format!("/{path}")))
}

/// Whether a search answer from `from` may send this node to `device`: only
/// to the machine that answered. Otherwise any host on the LAN could point
/// this node's requests at another machine's service.
pub(crate) fn answered_by(from: SocketAddr, device: SocketAddrV4) -> bool {
    from.ip() == std::net::IpAddr::V4(*device.ip())
}

/// The text of `<tag>` inside `xml`, after `from`.
fn element<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{tag}>"))? + start;
    Some(xml[start..end].trim())
}

/// In a device description, the control URL of the first service that maps
/// ports, and which service it is.
pub(crate) fn control_url(description: &str) -> Option<(String, String)> {
    description.split("<service>").skip(1).find_map(|block| {
        let kind = element(block, "serviceType")?;
        SERVICES
            .contains(&kind)
            .then(|| element(block, "controlURL").map(|url| (kind.to_owned(), url.to_owned())))
            .flatten()
    })
}

/// One plain HTTP/1.1 exchange on the LAN, capped and bounded.
fn http(to: SocketAddrV4, request: &str) -> Option<String> {
    let mut stream = TcpStream::connect_timeout(&SocketAddr::V4(to), SEARCH_FOR).ok()?;
    stream.set_read_timeout(Some(SEARCH_FOR)).ok()?;
    stream.set_write_timeout(Some(SEARCH_FOR)).ok()?;
    stream.write_all(request.as_bytes()).ok()?;
    let mut reply = Vec::new();
    let _ = (&mut stream).take(MAX_REPLY).read_to_end(&mut reply);
    let text = String::from_utf8_lossy(&reply).into_owned();
    let (head, body) = text.split_once("\r\n\r\n")?;
    head.split(' ').nth(1).filter(|code| *code == "200")?;
    Some(body.to_owned())
}

/// Find the router, or `None` when nothing answers in [`SEARCH_FOR`].
pub(crate) fn discover() -> Option<Gateway> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket
        .set_read_timeout(Some(Duration::from_millis(500)))
        .ok()?;
    let search = format!(
        "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\n\
         MX: 2\r\nST: {}\r\n\r\n",
        "urn:schemas-upnp-org:device:InternetGatewayDevice:1"
    );
    socket
        .send_to(search.as_bytes(), (Ipv4Addr::new(239, 255, 255, 250), 1900))
        .ok()?;
    let started = Instant::now();
    let mut buffer = [0_u8; 2048];
    while started.elapsed() < SEARCH_FOR {
        let Ok((length, from)) = socket.recv_from(&mut buffer) else {
            continue;
        };
        let answer = String::from_utf8_lossy(&buffer[..length]).into_owned();
        let Some((device, path)) =
            location(&answer).filter(|(device, _)| answered_by(from, *device))
        else {
            continue;
        };
        let description = http(
            device,
            &format!("GET {path} HTTP/1.1\r\nHost: {device}\r\nConnection: close\r\n\r\n"),
        )?;
        let (service, control) = control_url(&description)?;
        let (control, path) = if control.starts_with("http://") {
            split_url(&control).filter(|(at, _)| at.ip() == device.ip())?
        } else {
            (device, format!("/{}", control.trim_start_matches('/')))
        };
        // The address this machine uses to reach the router: the one the
        // router must forward to.
        let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
        probe.connect(SocketAddr::V4(control)).ok()?;
        let SocketAddr::V4(local) = probe.local_addr().ok()? else {
            return None;
        };
        return Some(Gateway {
            control,
            path,
            service,
            local: *local.ip(),
        });
    }
    None
}

/// The SOAP request for `action` with `arguments`, already escaped.
pub(crate) fn soap(gateway: &Gateway, action: &str, arguments: &str) -> String {
    let body = format!(
        "<?xml version=\"1.0\"?>\r\n<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body>\
         <u:{action} xmlns:u=\"{service}\">{arguments}</u:{action}></s:Body></s:Envelope>\r\n",
        service = gateway.service
    );
    format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: text/xml; charset=\"utf-8\"\r\n\
         SOAPAction: \"{service}#{action}\"\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}",
        path = gateway.path,
        host = gateway.control,
        service = gateway.service,
        length = body.len()
    )
}

/// Map the router's `port` (TCP) to this machine's same port for [`LEASE`].
pub(crate) fn map(gateway: &Gateway, port: u16) -> bool {
    let arguments = format!(
        "<NewRemoteHost></NewRemoteHost><NewExternalPort>{port}</NewExternalPort>\
         <NewProtocol>TCP</NewProtocol><NewInternalPort>{port}</NewInternalPort>\
         <NewInternalClient>{}</NewInternalClient><NewEnabled>1</NewEnabled>\
         <NewPortMappingDescription>ITSaNAS</NewPortMappingDescription>\
         <NewLeaseDuration>{}</NewLeaseDuration>",
        gateway.local,
        LEASE.as_secs()
    );
    http(
        gateway.control,
        &soap(gateway, "AddPortMapping", &arguments),
    )
    .is_some()
}

/// The router's public address, when it has one: a router behind another
/// router (carrier-grade NAT, a box behind a box) has a private one, and
/// announcing that would tell peers an address nobody outside can dial.
pub(crate) fn public_address(gateway: &Gateway) -> Option<Ipv4Addr> {
    let reply = http(gateway.control, &soap(gateway, "GetExternalIPAddress", ""))?;
    let address: Ipv4Addr = element(&reply, "NewExternalIPAddress")?.parse().ok()?;
    is_public(address).then_some(address)
}

/// An address the internet can route to.
pub(crate) fn is_public(address: Ipv4Addr) -> bool {
    !(address.is_private()
        || address.is_loopback()
        || address.is_link_local()
        || address.is_unspecified()
        || address.is_broadcast()
        || address.is_documentation()
        // 100.64.0.0/10, carrier-grade NAT.
        || (address.octets()[0] == 100 && (address.octets()[1] & 0xC0) == 64))
}

/// Map `port` and return the address to announce, `"a.b.c.d:port"`, or why
/// not. Called once before the daemon serves, then by [`renew`].
pub(crate) fn open_port(port: u16) -> Result<(Gateway, String), String> {
    let gateway = discover().ok_or("no router answered UPnP on this network")?;
    if !map(&gateway, port) {
        return Err(
            "the router refused to open the port (UPnP may be off in its settings)".to_owned(),
        );
    }
    let address = public_address(&gateway)
        .ok_or("the router has no public address (it is behind another router)")?;
    Ok((gateway, format!("{address}:{port}")))
}

/// Keep the mapping alive until `stop`: renewed at half its lease.
pub(crate) fn renew(gateway: &Gateway, port: u16, stop: &std::sync::atomic::AtomicBool) {
    let mut last = Instant::now();
    while !stop.load(std::sync::atomic::Ordering::SeqCst) {
        std::thread::sleep(Duration::from_secs(1));
        if last.elapsed() >= LEASE / 2 {
            if !map(gateway, port) {
                eprintln!("itsanas: the router stopped renewing the opened port");
            }
            last = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESCRIPTION: &str = "<root><device><serviceList>\
        <service><serviceType>urn:schemas-upnp-org:service:Layer3Forwarding:1</serviceType>\
        <controlURL>/l3f</controlURL></service>\
        <service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>\
        <controlURL>/upnp/control/WANIPConn1</controlURL></service>\
        </serviceList></device></root>";

    #[test]
    fn the_router_s_answers_are_read_as_routers_write_them() {
        assert_eq!(
            location(
                "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nLocation: http://192.168.1.254:5678/desc.xml\r\n\r\n"
            ),
            Some((
                SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 254), 5678),
                "/desc.xml".to_owned()
            )),
            "a Freebox's search answer was not understood: no port is ever opened"
        );
        assert_eq!(
            control_url(DESCRIPTION),
            Some((
                SERVICES[0].to_owned(),
                "/upnp/control/WANIPConn1".to_owned()
            )),
            "the port-mapping service was not found among the router's services"
        );
    }

    #[test]
    fn red_team_a_router_naming_itself_by_name_is_not_followed() {
        assert_eq!(
            split_url("http://evil.example:80/desc.xml"),
            None,
            "a search answer naming a host by name was followed: whoever answers DNS chooses \
             where this node sends its requests"
        );
        assert_eq!(split_url("https://192.168.1.1/x"), None);
        let router = SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 254), 5678);
        assert!(answered_by(
            "192.168.1.254:1900".parse().expect("addr"),
            router
        ));
        assert!(
            !answered_by("192.168.1.66:1900".parse().expect("addr"), router),
            "an answer from one host sent this node to another's service: any machine on the \
             LAN could aim it at a router's admin page"
        );
    }

    #[test]
    fn red_team_a_private_public_address_is_never_announced() {
        for private in [
            "192.168.1.10",
            "10.0.0.1",
            "100.64.3.4",
            "172.16.0.9",
            "0.0.0.0",
        ] {
            assert!(
                !is_public(private.parse().expect("ip")),
                "{private} was taken for a public address: peers elsewhere would be told to \
                 dial something only this network can reach"
            );
        }
        assert!(is_public(Ipv4Addr::new(82, 67, 35, 234)));
    }

    #[test]
    fn the_mapping_request_says_what_a_router_expects() {
        let gateway = Gateway {
            control: SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 254), 5678),
            path: "/upnp/control/WANIPConn1".to_owned(),
            service: SERVICES[0].to_owned(),
            local: Ipv4Addr::new(192, 168, 1, 20),
        };
        let request = soap(
            &gateway,
            "AddPortMapping",
            "<NewExternalPort>9797</NewExternalPort>",
        );
        assert!(request.starts_with("POST /upnp/control/WANIPConn1 HTTP/1.1\r\n"));
        assert!(request.contains(&format!("SOAPAction: \"{}#AddPortMapping\"", SERVICES[0])));
        let body = request.split_once("\r\n\r\n").expect("body").1;
        assert!(
            request.contains(&format!("Content-Length: {}\r\n", body.len())),
            "a wrong Content-Length makes the router read half a request and refuse it"
        );
    }
}
