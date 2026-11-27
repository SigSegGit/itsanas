//! Which address, if any, the coordinator may dial on a caller's behalf.

use itsanas_crypto::DeviceId;

use super::{CoordService, Probe};
use crate::error::Result;

impl CoordService<'_> {
    /// Which address, if any, may be probed on this caller's behalf.
    ///
    /// Four refusals, and each of them closes a way of turning a coordinator
    /// into somebody else's tool:
    ///
    /// * **Not an enrolled device.** A probe is work done for a member.
    /// * **Never announced.** There is nothing to probe, and the alternative --
    ///   letting the caller name an address -- is the scanner this design
    ///   exists to avoid.
    /// * **A private address.** Probing `192.168.1.1:80` or `10.0.0.5:22` from
    ///   the coordinator is a scan of the *coordinator's* own network, which is
    ///   the one network a member has no business reaching. This is the rule
    ///   that would matter if the coordinator ever ran somewhere with company
    ///   on its LAN.
    /// * **An address that is not `host:port`.** Nothing that reaches the
    ///   resolver is shaped by a stranger beyond what `Announce` already
    ///   accepted.
    ///
    /// What remains is: a member, who published a public address, may have that
    /// address dialled once an hour. The residual is written down rather than
    /// argued away -- an enrolled member can make the coordinator open one
    /// connection an hour to a public address of their choosing, which is a
    /// slow and attributable port scanner, and the answer to that is that they
    /// are enrolled and can be un-enrolled.
    pub fn probe_target(&self, caller: DeviceId, now_unix: u64) -> Result<Probe> {
        let Some(claim) = self.directory.claim_for(caller)? else {
            return Ok(Probe::Refuse(
                "this device is not enrolled under any account".to_owned(),
            ));
        };
        if claim.claim.revoked || claim.verify(now_unix).is_err() {
            return Ok(Probe::Refuse(
                "this device's enrolment is revoked or invalid".to_owned(),
            ));
        }

        let Some(presence) = self.directory.presence_of(caller)? else {
            return Ok(Probe::Refuse(
                "this device has not announced an address, so there is nothing to try".to_owned(),
            ));
        };

        let address = presence.presence.address;
        // A list of addresses (HANDOVER §8 0u): every entry must be one, and
        // no more of them than a dialler tries.
        let entries: Vec<&str> = address.split(',').map(str::trim).collect();
        if entries.len() > itsanas_tls::reach::MAX_LISTED
            || entries.iter().any(|entry| {
                entry.parse::<std::net::SocketAddr>().is_err() && !looks_like_host_port(entry)
            })
        {
            return Ok(Probe::Refuse(format!(
                "{address:?} is not an address this coordinator will dial"
            )));
        }
        if itsanas_tls::reach::is_private_address(&address) {
            return Ok(Probe::Refuse(format!(
                concat!(
                    "{} is private, so it means something only on the network it ",
                    "was announced from; a coordinator elsewhere cannot tell you ",
                    "anything about it"
                ),
                address
            )));
        }

        Ok(Probe::Address(address))
    }
}

/// Whether a string is plausibly `host:port` with a name rather than a literal.
///
/// The announce path already refuses everything else; this is the second lock
/// on the door that leads to a resolver, because the first one is in another
/// crate and could be relaxed by somebody who has not read this one.
fn looks_like_host_port(address: &str) -> bool {
    let Some((host, port)) = address.rsplit_once(':') else {
        return false;
    };
    !host.is_empty()
        && !host.contains([':', '[', ']', '/', ' ', '@'])
        && port.parse::<u16>().is_ok_and(|port| port != 0)
}
