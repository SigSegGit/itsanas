//! Probing a member on request: whether the address it announced reaches
//! that device, within a bound on every side.

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use itsanas_crypto::{DeviceId, DeviceKeys};
use itsanas_tls::{Identity, connect};

use super::now_unix;
use crate::protocol::Response;
use crate::service::{CoordService, EscrowLimiter, PROBE_WINDOW, PROBES_PER_DEVICE, Probe};

/// What a [`Request::CheckMe`](crate::protocol::Request::CheckMe) is charged
/// to: one probe per address of a device, and [`PROBES_PER_DEVICE`]
/// addresses per device, per [`PROBE_WINDOW`].
#[derive(Debug)]
pub struct ProbeBudget {
    per_device: EscrowLimiter,
    per_target: EscrowLimiter,
}

impl ProbeBudget {
    /// The budget, remembering at most `tracked` devices.
    #[must_use]
    pub fn new(tracked: usize) -> Self {
        Self {
            per_device: EscrowLimiter::with(PROBES_PER_DEVICE, PROBE_WINDOW, tracked),
            per_target: EscrowLimiter::with(
                1,
                PROBE_WINDOW,
                tracked.saturating_mul(PROBES_PER_DEVICE as usize),
            ),
        }
    }

    /// Whether `caller` may have `target` probed at `now`; charged only when
    /// it may, and the refusal to say when not.
    ///
    /// # Errors
    ///
    /// The sentence for the caller: a repeat, or one address too many.
    pub fn allow(
        &mut self,
        caller: DeviceId,
        target: &str,
        now: Instant,
    ) -> std::result::Result<(), &'static str> {
        // Keyed by device **and address**: the budget exists to stop a daemon
        // asking every round, and a *new* address is precisely the case where
        // the answer can have changed. Found by using it -- repointing a node
        // during a fleet migration, the second question was refused as a
        // repeat of the first, which is the moment somebody most needs it.
        let device = caller.to_hex();
        let key = format!("{device}@{target}");
        // Both checked before either is charged: a repeat must not spend one
        // of the device's addresses, nor one address too many take a row.
        if !self.per_target.would_allow(&key, now) {
            return Err(
                "this address was already probed within the hour; the answer from then stands",
            );
        }
        if !self.per_device.would_allow(&device, now) {
            return Err(
                "this device has had as many addresses probed as it may this hour; ask again later",
            );
        }
        self.per_target.allow(&key, now);
        self.per_device.allow(&device, now);
        Ok(())
    }
}

/// Decide whether to probe this caller, and probe them.
///
/// The decision is `CoordService`'s and is tested without a socket; everything
/// here is the acting on it, and every line between the two is a bound:
/// one probe per device per hour, at most [`MAX_PROBES_IN_FLIGHT`] at once, and
/// a three-second timeout. A member whose router is dead costs this coordinator
/// three seconds of one thread, once an hour.
pub(super) fn answer_check_me(
    service: &CoordService<'_>,
    caller: DeviceId,
    device: &DeviceKeys,
    probes: &Mutex<ProbeBudget>,
    in_flight: &AtomicUsize,
) -> Response {
    let target = match service.probe_target(caller, now_unix()) {
        Ok(Probe::Address(address)) => address,
        // A refusal to probe is not a finding about the member's network.
        // "Your address is private" and "nothing can reach you" are different
        // sentences and only one of them sends somebody to a router.
        Ok(Probe::Refuse(why)) => return Response::Unknown(why),
        Err(error) => return Response::Refused(error.to_string()),
    };

    {
        let Ok(mut probes) = probes.lock() else {
            return Response::Refused("this coordinator is not answering probes".to_owned());
        };
        if let Err(why) = probes.allow(caller, &target, Instant::now()) {
            return Response::Unknown(why.to_owned());
        }
    }

    // Taken before the dial and given back however it ends, including on an
    // early return: a counter that leaks on a failure path is a limit that
    // shrinks to zero, and every probe of an unreachable member is a failure
    // path.
    let Some(_slot) = InFlight::take(in_flight) else {
        return Response::Unknown(
            "this coordinator is already probing as many members as it will at once; ask again shortly"
                .to_owned(),
        );
    };

    let targets = match resolve_probe_target(&target) {
        Ok(targets) => targets,
        // Refusing to dial is not a verdict either: a name that resolves into a
        // private network says nothing about whether the member is reachable.
        Err(why) => return Response::Unknown(why),
    };

    let (reachable, detail) = probe(&target, &targets, caller, device);
    Response::Reachable { reachable, detail }
}

/// One of the concurrent probe slots, given back when it is dropped.
struct InFlight<'a>(&'a AtomicUsize);

impl<'a> InFlight<'a> {
    fn take(count: &'a AtomicUsize) -> Option<Self> {
        // A compare-exchange loop rather than `fetch_update`: Rust 1.99
        // deprecated that name for `try_update`, which the MSRV (1.88) does
        // not have, and CI denies warnings on the newest stable.
        let mut current = count.load(Ordering::SeqCst);
        let taken = loop {
            if current >= MAX_PROBES_IN_FLIGHT {
                break false;
            }
            match count.compare_exchange_weak(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break true,
                Err(seen) => current = seen,
            }
        };
        // `then`, not `then_some`: `then_some` takes its argument by value, so
        // the guard would be *constructed* even when no slot was taken -- and
        // dropped an instant later, giving back a slot that was never held.
        // The counter underflowed to `usize::MAX` and the next probe panicked
        // on the increment. Caught by
        // `a_probe_slot_is_given_back_however_the_probe_ends`, which was
        // written for the opposite leak.
        taken.then(|| Self(count))
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// How many probes may be in flight at once, across the whole coordinator.
///
/// A probe holds an outbound socket for as long as a handshake takes, and the
/// coordinator's job is to answer members, not to wait on their routers. Four
/// is enough that a fleet never queues behind one unreachable member and small
/// enough that the count cannot become the load.
pub const MAX_PROBES_IN_FLIGHT: usize = 4;

/// How long a probe may take before it counts as unreachable.
///
/// Deliberately shorter than a member's own dial timeout: this is a
/// diagnostic, and "your router did not answer within three seconds" is the
/// same answer as "your router did not answer", for anybody reading it.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Resolve an address to probe, refusing anything that is not out on the
/// internet.
///
/// Split from the dialling for one reason: `probe_target` guards the announced
/// **string**, and a name is never private to that check -- correctly, because
/// what a name resolves to is the resolver's business and not a dialler's. Here
/// it becomes this process's business. `nas.example.org` pointing at
/// `192.168.1.10` walked straight through the string guard and had this
/// coordinator dial its own network, which is the one thing that guard exists
/// to prevent. Found by the Rodin audit of 2026-09-18, in a guard written the
/// same afternoon, and demonstrated by the sabotage run reaching the real
/// daemon on the machine the test ran on.
fn resolve_probe_target(address: &str) -> std::result::Result<Vec<SocketAddr>, String> {
    let targets: Vec<SocketAddr> = match itsanas_tls::reach::AddressList(address).to_socket_addrs()
    {
        Ok(found) => found.collect(),
        Err(error) => return Err(format!("{address} does not resolve: {error}")),
    };
    if targets.is_empty() {
        return Err(format!("{address} resolves to no address at all"));
    }

    if let Some(private) = targets
        .iter()
        .find(|target| itsanas_tls::reach::is_private_ip(target.ip()))
    {
        return Err(format!(
            "{address} resolves to {private}, a private address: dialling it would reach a network of this coordinator's own rather than tell you anything about yours"
        ));
    }

    Ok(targets)
}

/// Dial `targets`, expecting `expect` to answer, and say what happened.
///
/// A **device-authenticated handshake**, not a bare connection. An open port
/// proves that something is listening; completing this proves the address leads
/// to that member's machine, which is the question they actually asked. The
/// connection is dropped the moment it is proved: the coordinator has nothing
/// to say over the peer protocol and does not speak it.
///
/// Takes resolved addresses rather than a string, so that the refusal to dial
/// anything private happens once, in `resolve_probe_target`, on the only path
/// that reaches here in production.
fn probe(
    address: &str,
    targets: &[SocketAddr],
    expect: DeviceId,
    device: &DeviceKeys,
) -> (bool, String) {
    let stream = match itsanas_tls::reach::connect_within(targets, PROBE_TIMEOUT) {
        Ok(stream) => stream,
        Err(error) => {
            return (
                false,
                format!(
                    "nothing answered at {address}: {error}. A forward or a firewall rule is missing, or it points somewhere else"
                ),
            );
        }
    };

    let identity = match Identity::generate() {
        Ok(identity) => identity,
        Err(error) => {
            return (
                false,
                format!("this coordinator has no TLS identity: {error}"),
            );
        }
    };
    let config = match identity.client_config() {
        Ok(config) => config,
        Err(error) => {
            return (
                false,
                format!("this coordinator has no TLS config: {error}"),
            );
        }
    };

    match connect(&config, device, stream, Some(expect)) {
        Ok(_) => (true, format!("{address} answered, and it is this device")),
        Err(error) => (
            false,
            format!(
                "{address} answered, but not as this device: {error}. The address reaches some other machine -- a forward pointing at the wrong host, or an address that is not yours"
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use itsanas_crypto::SecretBytes;
    use itsanas_tls::accept_within;

    fn keys(seed: u8) -> DeviceKeys {
        DeviceKeys::from_seed(&SecretBytes::new([seed; 32]))
    }

    /// A listener that answers one connection as `device` and then stops.
    ///
    /// Everything a node's listener does that a probe can observe: TLS with a
    /// device proof. It deliberately does *not* speak the peer protocol,
    /// because the probe must not need it -- a coordinator that had to
    /// understand the peer protocol to answer this question would be a
    /// coordinator that knows what members store.
    fn listening_as(device: DeviceKeys) -> (SocketAddr, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let handle = std::thread::spawn(move || {
            let identity = Identity::generate().expect("identity");
            let config = identity.server_config().expect("server config");
            if let Ok((stream, _)) = listener.accept() {
                let _ = accept_within(
                    &config,
                    &device,
                    stream,
                    Duration::from_secs(5),
                    Duration::from_secs(5),
                );
            }
        });
        (address, handle)
    }

    #[test]
    fn a_probe_that_reaches_the_device_says_so() {
        // The same seed twice rather than a clone: a device key is not `Clone`
        // on purpose, because two copies of one are two machines sharing a
        // sequence counter.
        let expected = keys(0x21).device_id();
        let (address, handle) = listening_as(keys(0x21));

        let (reachable, detail) = probe(&address.to_string(), &[address], expected, &keys(0xC0));

        assert!(
            reachable,
            "a listening device was reported unreachable: {detail}"
        );
        let _ = handle.join();
    }

    /// THE POINT OF THE HANDSHAKE: an open port proves something is there. A
    /// forward pointing at the wrong machine -- the other Raspberry Pi, the
    /// printer, a neighbour on the same public address -- is an open port, and
    /// reporting it as success would tell a member their setup works while
    /// every peer that dialled them got refused by the pinning in
    /// `PeerClient::connect`.
    #[test]
    fn red_team_a_probe_that_reaches_a_different_machine_is_not_a_success() {
        let somebody_else = keys(0x22);
        let (address, handle) = listening_as(somebody_else);

        let (reachable, detail) = probe(
            &address.to_string(),
            &[address],
            keys(0x23).device_id(),
            &keys(0xC0),
        );

        assert!(
            !reachable,
            "a port answering as another device was reported as this one being reachable"
        );
        assert!(
            detail.contains("not as this device"),
            "the answer must say what is wrong, so a person can fix the forward; it said {detail:?}"
        );
        let _ = handle.join();
    }

    #[test]
    fn a_probe_of_an_address_where_nothing_listens_says_nothing_answered() {
        // Loopback port 1: refused immediately on every runner, so this is
        // fast and does not depend on a timeout.
        let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let (reachable, detail) =
            probe("127.0.0.1:1", &[dead], keys(0x24).device_id(), &keys(0xC0));

        assert!(!reachable);
        assert!(
            detail.contains("forward") || detail.contains("firewall"),
            "the answer must name what a person should go and look at; it said {detail:?}"
        );
    }

    /// THE BYPASS, and it was live for an afternoon. The guard in
    /// `probe_target` reads the announced *string*, and a name is never private
    /// to it -- correctly, because what a name resolves to is not a dialler's
    /// business. Then `probe` resolves it. So `nas.example.org` pointing at
    /// `192.168.1.10` walked straight through a guard written to stop exactly
    /// that, and turned the coordinator into a scanner of its own network by
    /// way of DNS. The check that counts is on the resolved address.
    #[test]
    fn red_team_a_name_that_resolves_into_a_private_network_is_not_dialled() {
        // `localhost` is the one name every machine resolves to a private
        // address, so this is the bypass without a resolver anybody controls.
        // Nothing is dialled here: the refusal happens before any socket, which
        // is the whole point -- during the sabotage run that proved this hole,
        // the probe reached the real daemon on the machine running the test.
        let refused = resolve_probe_target("localhost:9797")
            .expect_err("a name resolving into a private network was accepted");

        assert!(
            refused.contains("private address"),
            "the refusal must say why, so nobody 'fixes' it by widening the guard; it said {refused:?}"
        );
    }

    #[test]
    fn a_probe_of_a_name_that_does_not_resolve_says_that_rather_than_timing_out() {
        let refused = resolve_probe_target("this-name-does-not-exist.invalid:9797")
            .expect_err("a name that does not resolve is not somewhere to dial");

        assert!(
            refused.contains("does not resolve"),
            "DNS and a closed port are different problems, fixed in different places, and must read differently; it said {refused:?}"
        );
    }

    /// THE BUDGET, as a property of the counter rather than of the caller: a
    /// slot that is not given back on a failure path is a limit that shrinks to
    /// zero, and every probe of an unreachable member *is* a failure path.
    #[test]
    fn a_probe_slot_is_given_back_however_the_probe_ends() {
        let count = AtomicUsize::new(0);

        for _ in 0..MAX_PROBES_IN_FLIGHT * 3 {
            let slot = InFlight::take(&count).expect("a slot must be free");
            drop(slot);
        }
        assert_eq!(count.load(Ordering::SeqCst), 0);

        let held: Vec<_> = (0..MAX_PROBES_IN_FLIGHT)
            .map(|_| InFlight::take(&count).expect("within the bound"))
            .collect();
        assert!(
            InFlight::take(&count).is_none(),
            "more probes ran at once than the bound allows"
        );
        drop(held);
        assert!(
            InFlight::take(&count).is_some(),
            "the bound never reopened, so this coordinator stops probing for ever"
        );
    }

    /// THE ATTACK: an enrolled device announces a new public address before
    /// each probe, and each new address was a fresh budget: the coordinator
    /// dialled one chosen host after another. Sabotage: drop the per-device
    /// budget; charge it before checking the per-address one.
    #[test]
    fn red_team_one_device_cannot_probe_a_new_address_every_time() {
        let mut budget = ProbeBudget::new(crate::service::PROBE_TRACKED);
        let now = Instant::now();
        let (one, two) = (keys(1).device_id(), keys(2).device_id());
        let allowed = (1..=10)
            .filter(|n| budget.allow(one, &format!("203.0.113.{n}:22"), now).is_ok())
            .count();
        assert_eq!(
            allowed, PROBES_PER_DEVICE as usize,
            "one device had a new address probed every time it asked"
        );
        assert!(
            budget.allow(two, "203.0.113.1:22", now).is_ok(),
            "another device was refused for the first one's probes"
        );
        assert!(
            budget.allow(two, "203.0.113.1:22", now).is_err(),
            "the same address was probed twice within the hour"
        );
        for n in [2, 3] {
            assert!(
                budget.allow(two, &format!("203.0.113.{n}:22"), now).is_ok(),
                "a repeat refused spent the device's budget of addresses"
            );
        }
        assert!(
            budget
                .allow(one, "203.0.113.1:22", now + PROBE_WINDOW)
                .is_ok(),
            "the window never reopened"
        );
    }
}
