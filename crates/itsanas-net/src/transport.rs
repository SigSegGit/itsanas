//! Moving frames between two machines, encrypted and mutually authenticated.
//!
//! # What changed, and why the warnings are gone
//!
//! This transport used to be plain TCP. Your *data* was never at risk — chunk
//! bodies and log segments are sealed before they reach the wire — but an
//! observer on the path saw chunk identifiers, object sizes and timing. The
//! threat model grants a host all three, because a host stores the chunks; it
//! does not grant them to an arbitrary network between two of your machines.
//! `PeerServer::bind` therefore refused non-loopback addresses.
//!
//! It no longer needs to. Every connection is TLS 1.3, and both ends prove
//! which device they are by signing the session's exporter value with their
//! device key ([`itsanas_tls`]). A man in the middle who terminates TLS gets a
//! different exporter and cannot forge either signature, so the encryption is
//! bound to the identity rather than sitting beside it.
//!
//! The certificates are anonymous and regenerated every start-up. That is not
//! a weakness — see [`itsanas_tls::session`] — and it means an observer cannot
//! correlate two connections by their certificates either.
//!
//! # Serving strangers is still deliberate
//!
//! A node answers any device that authenticates, including one it has never
//! met. That is what lets somebody offer storage to the network at all.
//! Everything it can serve is sealed or signed, so serving it to the wrong
//! person reveals nothing, and how much it will *store* is bounded by the
//! pledge. What is new is that the node now knows *who* it served, which is
//! what any future policy would need.
//!
//! # Why blocking sockets and threads
//!
//! A node talks to a handful of peers, not ten thousand. A thread per
//! connection costs a few megabytes and buys code that can be read top to
//! bottom. Async would buy scalability this design does not need, at the price
//! of an executor in every signature.

use std::{
    io::{Read, Write},
    time::{Duration, Instant},
};

use itsanas_tls::Socket;

/// How long a read or write may stall before the connection is abandoned.
///
/// Without this a single peer that opens a connection and then says nothing
/// holds a thread forever, and enough of them exhaust the node.
pub const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the daemon gives one peer in a round, everything included.
///
/// [`IO_TIMEOUT`] bounds one read or write, not a conversation: a peer that
/// answers a little at a time, or keeps a slow transfer just alive, holds the
/// round for as long as it likes, and with it the folder pass and every other
/// peer. Seen on 2026-10-06: a laptop's daemon spent 45 minutes on one
/// connection to an older-build node reached through a virtual network adapter, and
/// nothing it or its peers wrote moved in that time. Cutting a session loses
/// nothing -- every chunk is acknowledged on its own and the next round goes
/// on from there -- so the budget is one sync interval: a round with `n`
/// peers ends within `n` intervals whatever any of them does.
pub const PEER_SESSION_BUDGET: Duration = Duration::from_secs(300);

/// A socket whose whole conversation, not just each read, has a deadline.
///
/// Every read and write is given the smaller of [`IO_TIMEOUT`] and what is left
/// of the budget, and fails once nothing is. Without a budget it behaves as a
/// plain socket under [`IO_TIMEOUT`], which is what interactive commands want:
/// somebody watching `itsanas sync` can stop it themselves.
pub struct Budgeted {
    stream: Box<dyn Socket>,
    until: Option<Instant>,
}

impl std::fmt::Debug for Budgeted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Budgeted")
            .field("until", &self.until)
            .finish_non_exhaustive()
    }
}

impl Budgeted {
    fn left(&self) -> std::io::Result<Option<Duration>> {
        let Some(until) = self.until else {
            return Ok(None);
        };
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "this peer used up its time for this round; the rest waits for the next",
            ));
        }
        Ok(Some(left.min(IO_TIMEOUT)))
    }
}

impl Socket for Budgeted {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.stream.set_read_timeout(timeout)
    }
    fn set_write_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.stream.set_write_timeout(timeout)
    }
}

impl Read for Budgeted {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(limit) = self.left()? {
            self.stream.set_read_timeout(Some(limit))?;
        }
        self.stream.read(buf)
    }
}

impl Write for Budgeted {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(limit) = self.left()? {
            self.stream.set_write_timeout(Some(limit))?;
        }
        self.stream.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

mod asking;
mod client;
mod server;

pub use client::{Offer, PeerClient, Refusal};
pub use server::{
    HANDSHAKE_DEADLINE, MAX_CONNECTIONS, MAX_CONNECTIONS_PER_ADDRESS, MAX_CONNECTIONS_PER_DEVICE,
    PeerServer, TOO_MANY_FROM_DEVICE, Witness,
};
