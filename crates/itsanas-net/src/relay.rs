//! A relay for a machine nobody can reach (§8 0v).
//!
//! A machine behind a router that opens nothing, with no global IPv6 address,
//! can still dial out. So it dials a peer that *is* reachable and parks a
//! connection there ([`crate::Request::Park`]); a caller who cannot reach it
//! asks that peer to splice ([`crate::Request::Splice`]), and the relay then
//! copies bytes between the two connections. Over the splice the two ends run
//! their own TLS and prove their device keys to each other as they would over
//! TCP: the relay carries ciphertext, and can drop or delay it, nothing else.
//!
//! What a relay spends is bounded here, because it spends it for others: a
//! few parks, a few splices, one shared rate for all of them, and a session as
//! long as an ordinary peer's. Large transfers are meant to wait for a direct
//! path; the relay keeps logs and small repairs moving.

use std::{
    collections::HashMap,
    io::{ErrorKind, Read, Write},
    net::TcpStream,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
    time::{Duration, Instant},
};

use itsanas_crypto::{DeviceId, UserId};
use itsanas_tls::Socket;

/// How often a relay tells a parked machine it is still there. Under
/// [`crate::IO_TIMEOUT`], the parked side's read timeout, so a quiet park
/// is never taken for a dead one; and under the half-minute after which
/// many home routers forget an idle mapping.
pub const KEEPALIVE: Duration = Duration::from_secs(20);

/// Parks kept for machines of the relay's own account. Apart from the
/// others' so strangers can never push the account's own machines out.
pub const MAX_PARKED_OWN: usize = 8;

/// Parks kept for machines of accounts this relay hosts for, in total.
pub const MAX_PARKED_HOSTED: usize = 8;

/// Parks kept for one hosted account: one account's spare devices cannot
/// take every place.
pub const MAX_PARKED_PER_ACCOUNT: usize = 2;

/// Splices carried at once.
pub const MAX_SPLICES: usize = 4;

/// Bytes a second the relay carries, all splices together: what it gives
/// others is its owner's uplink.
pub const RATE: u64 = 1024 * 1024;

/// A splice on which nothing has moved for this long is closed.
pub const SPLICE_IDLE: Duration = crate::IO_TIMEOUT;

/// How long one splice may last, whatever moves on it: one peer session.
pub const SPLICE_BUDGET: Duration = crate::PEER_SESSION_BUDGET;

/// How long one wait for bytes on either side of a splice lasts, which is
/// also how late the other side's bytes may be forwarded.
const POLL: Duration = Duration::from_millis(10);

/// What a peer older than [`crate::protocol::PROTOCOL_WITH_RELAY`] means.
pub const RELAY_TOO_OLD: &str = "this peer is too old to relay";
/// The one refusal of a park or splice: the same whether the caller is a
/// stranger or the machine asked for is not parked, so a relay is no oracle
/// on who is online.
pub const NOT_RELAYING: &str = "this peer relays nothing for this device";
/// What a session carried by a relay answers a park or a splice.
pub const NOT_RELAYED: &str = "relays are not chained";

/// Which pool a park is counted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// A machine of the relay's own account.
    Own,
    /// A machine of an account the relay hosts for.
    Hosted(UserId),
}

/// The relay's end of a caller's connection, handed to the park it asked for.
pub(crate) type Outer = itsanas_tls::session::ServerStream<itsanas_tls::Bounded<TcpStream>>;

/// A caller's connection, what was read past its request, and who it proved
/// to be.
pub(crate) struct Handoff {
    pub stream: Outer,
    pub rest: Vec<u8>,
    pub from: DeviceId,
}

struct Parked {
    class: Class,
    ticket: u64,
    hand: SyncSender<Handoff>,
}

/// What a relay keeps: who is parked, how many splices run, the shared rate.
#[derive(Default)]
pub struct Desk {
    parked: Mutex<HashMap<DeviceId, Parked>>,
    tickets: AtomicUsize,
    splices: AtomicUsize,
    /// When the shared rate next has room.
    next: Mutex<Option<Instant>>,
}

impl std::fmt::Debug for Desk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Desk")
            .field("splices", &self.splices.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// A park's place, given up when dropped.
pub(crate) struct Ticket<'a> {
    desk: &'a Desk,
    device: DeviceId,
    number: u64,
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        let mut parked = self.desk.lock();
        if parked
            .get(&self.device)
            .is_some_and(|p| p.ticket == self.number)
        {
            parked.remove(&self.device);
        }
    }
}

/// A splice's place, given up when dropped.
pub(crate) struct Splicing<'a>(&'a Desk);

impl Drop for Splicing<'_> {
    fn drop(&mut self) {
        self.0.splices.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Desk {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<DeviceId, Parked>> {
        self.parked.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Take a park for `device`, or `None` over the caps. A device parking
    /// again replaces its old park, which is how a machine whose connection
    /// died unseen gets back in.
    pub(crate) fn park(
        &self,
        device: DeviceId,
        class: Class,
    ) -> Option<(Receiver<Handoff>, Ticket<'_>)> {
        let mut parked = self.lock();
        parked.remove(&device);
        let in_pool =
            |wanted: &dyn Fn(Class) -> bool| parked.values().filter(|p| wanted(p.class)).count();
        let room = match class {
            Class::Own => in_pool(&|c| c == Class::Own) < MAX_PARKED_OWN,
            Class::Hosted(account) => {
                in_pool(&|c| matches!(c, Class::Hosted(_))) < MAX_PARKED_HOSTED
                    && in_pool(&|c| c == Class::Hosted(account)) < MAX_PARKED_PER_ACCOUNT
            }
        };
        if !room {
            return None;
        }
        let ticket = self.tickets.fetch_add(1, Ordering::Relaxed) as u64;
        let (hand, receive) = sync_channel(1);
        parked.insert(
            device,
            Parked {
                class,
                ticket,
                hand,
            },
        );
        Some((
            receive,
            Ticket {
                desk: self,
                device,
                number: ticket,
            },
        ))
    }

    /// Hand `handoff` to the park of `to`, which then carries it and nothing
    /// else: one splice per park, the machine parks again for the next.
    /// Gives the handoff back if there is no such park or no splice to spare.
    /// Whether `to` is parked and a splice is to spare: asked before the
    /// caller is told yes, since the connection cannot be handed and then
    /// answered on.
    pub(crate) fn can_hand(&self, to: DeviceId) -> bool {
        self.splices.load(Ordering::Relaxed) < MAX_SPLICES && self.lock().contains_key(&to)
    }

    /// Hand `handoff` to the park of `to`, which then carries it and nothing
    /// else: one splice per park, the machine parks again for the next.
    /// `false` if the park went in between, or the splices ran out; the
    /// caller's connection is then dropped, and its end-to-end handshake
    /// fails at once.
    pub(crate) fn hand(&self, to: DeviceId, handoff: Handoff) -> bool {
        let Some(parked) = self.lock().remove(&to) else {
            return false;
        };
        if self.splices.fetch_add(1, Ordering::Relaxed) >= MAX_SPLICES {
            self.splices.fetch_sub(1, Ordering::Relaxed);
            // Put back: a full relay is not a reason to lose the park.
            self.lock().entry(to).or_insert(parked);
            return false;
        }
        match parked.hand.try_send(handoff) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.splices.fetch_sub(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// The place of a splice [`Self::hand`] counted, given back when the
    /// park carrying it drops this.
    pub(crate) const fn splicing(&self) -> Splicing<'_> {
        Splicing(self)
    }

    /// Wait until `bytes` fit in the shared rate.
    fn spend(&self, bytes: usize) {
        let cost = Duration::from_nanos(
            u64::try_from(bytes)
                .unwrap_or(u64::MAX)
                .saturating_mul(1_000_000_000)
                / RATE,
        );
        let wait = {
            let mut next = self.next.lock().unwrap_or_else(PoisonError::into_inner);
            let now = Instant::now();
            let start = next.map_or(now, |at| at.max(now));
            *next = Some(start + cost);
            start.saturating_duration_since(now)
        };
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }

    /// Copy bytes both ways between `a` and `b` until either closes, nothing
    /// moves for [`SPLICE_IDLE`], or [`SPLICE_BUDGET`] is spent. One thread:
    /// each side is waited on for [`POLL`] in turn, because a TLS session
    /// cannot be read on one thread and written on another.
    pub(crate) fn pump<A: Socket, B: Socket>(
        &self,
        a: &mut A,
        b: &mut B,
        budget: Duration,
        idle: Duration,
    ) {
        if a.set_read_timeout(Some(POLL)).is_err() || b.set_read_timeout(Some(POLL)).is_err() {
            return;
        }
        let until = Instant::now() + budget;
        let mut moved = Instant::now();
        let mut buffer = vec![0u8; 16 * 1024];
        while Instant::now() < until && moved.elapsed() < idle {
            for forward in [true, false] {
                let read = if forward {
                    a.read(&mut buffer)
                } else {
                    b.read(&mut buffer)
                };
                let n = match read {
                    Ok(0) => return,
                    Ok(n) => n,
                    Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                        continue;
                    }
                    Err(_) => return,
                };
                self.spend(n);
                let written = if forward {
                    b.write_all(&buffer[..n]).and_then(|()| b.flush())
                } else {
                    a.write_all(&buffer[..n]).and_then(|()| a.flush())
                };
                if written.is_err() {
                    return;
                }
                moved = Instant::now();
            }
        }
    }
}

/// A stream that first gives back bytes already read from it: what a framed
/// reader took past the last message before the stream became a splice.
#[derive(Debug)]
pub struct Prefixed<S> {
    head: Vec<u8>,
    at: usize,
    stream: S,
}

impl<S> Prefixed<S> {
    #[must_use]
    pub const fn new(head: Vec<u8>, stream: S) -> Self {
        Self {
            head,
            at: 0,
            stream,
        }
    }
}

impl<S: Read> Read for Prefixed<S> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.at < self.head.len() {
            let take = (self.head.len() - self.at).min(out.len());
            out[..take].copy_from_slice(&self.head[self.at..self.at + take]);
            self.at += take;
            return Ok(take);
        }
        self.stream.read(out)
    }
}

impl<S: Write> Write for Prefixed<S> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.stream.write(data)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

impl<S: Socket> Socket for Prefixed<S> {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.stream.set_read_timeout(timeout)
    }
    fn set_write_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.stream.set_write_timeout(timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(seed: u8) -> DeviceId {
        DeviceId::from_bytes([seed; 32])
    }

    #[test]
    fn red_team_one_account_cannot_take_every_park_nor_push_out_the_relays_own() {
        let desk = Desk::default();
        let account = UserId::from_bytes([1; 32]);
        let mut held = Vec::new();
        for seed in 0..10 {
            if let Some(park) = desk.park(device(seed), Class::Hosted(account)) {
                held.push(park);
            }
        }
        assert_eq!(
            held.len(),
            MAX_PARKED_PER_ACCOUNT,
            "one account's spare devices filled the relay's parks"
        );
        for seed in 0..u8::try_from(MAX_PARKED_HOSTED).unwrap() {
            let other = UserId::from_bytes([10 + seed; 32]);
            if let Some(park) = desk.park(device(100 + seed), Class::Hosted(other)) {
                held.push(park);
            }
        }
        assert!(
            desk.park(device(200), Class::Hosted(UserId::from_bytes([99; 32])))
                .is_none(),
            "hosted parks went past their cap"
        );
        assert!(
            desk.park(device(201), Class::Own).is_some(),
            "strangers' parks pushed out the relay's own account"
        );
    }

    #[test]
    fn a_dropped_park_frees_its_place_and_a_reparked_device_replaces_its_old_one() {
        let desk = Desk::default();
        let account = UserId::from_bytes([1; 32]);
        let first = desk.park(device(1), Class::Hosted(account)).unwrap();
        let second = desk.park(device(1), Class::Hosted(account)).unwrap();
        // The stale ticket must not remove the fresh park.
        drop(first);
        assert!(desk.lock().contains_key(&device(1)));
        drop(second);
        assert!(desk.lock().is_empty());
    }

    #[test]
    fn the_shared_rate_holds_all_splices_together() {
        let desk = Desk::default();
        let start = Instant::now();
        // Three times a quarter of the rate, on top of the first free quarter.
        for _ in 0..4 {
            desk.spend((RATE / 4) as usize);
        }
        assert!(
            start.elapsed() >= Duration::from_millis(700),
            "a relay carried more than its rate: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn prefixed_gives_back_what_was_read_ahead_first() {
        let mut stream = Prefixed::new(b"ab".to_vec(), &b"cd"[..]);
        let mut out = String::new();
        stream.read_to_string(&mut out).unwrap();
        assert_eq!(out, "abcd");
    }
}
