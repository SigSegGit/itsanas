//! A network path that behaves like a censor's: slow, throttled, cut.
//!
//! A member in China reaches a coordinator or a peer in Europe through the
//! Great Firewall, which does not need to block a protocol to break it: a
//! quarter of a second each way, a foreign flow throttled to a few kilobytes
//! a second, a connection cut in the middle of an answer, or a path that lets
//! one byte through now and then. [`HostilePath`] sits between a client and
//! a server on loopback and does each of these on demand, so a test can say
//! what the program does when it happens. It also keeps what the client sent,
//! for a test that reads the clear part of a handshake as a censor's filter
//! does.
//!
//! A cut is a reset (RST), as a censor forges one: the path closes both ends
//! with a zero linger, which discards what was queued and sends a reset
//! rather than a polite close. Either way the connection ends mid-answer,
//! which is what a client must survive, and it is not a timeout.

use std::{
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

/// What the path does to the bytes it carries.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    /// Added before each piece is passed on, in either direction.
    pub delay: Duration,
    /// Bytes a second, in either direction; `None` is the link's own speed.
    pub rate: Option<usize>,
    /// Pass one byte at a time, this long apart.
    pub trickle: Option<Duration>,
    /// Close the first connection once this many bytes went through it, both
    /// directions counted; later connections are not cut.
    pub cut_first_after: Option<usize>,
}

/// A path between a client and `target`, listening on loopback.
#[derive(Debug)]
pub struct HostilePath {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    sent: Arc<Mutex<Vec<u8>>>,
}

impl HostilePath {
    /// Start a path to `target` under `rules`.
    ///
    /// # Panics
    ///
    /// If loopback cannot be bound, which no test can work around.
    #[must_use]
    pub fn start(target: SocketAddr, rules: Rules) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the path");
        listener
            .set_nonblocking(true)
            .expect("non-blocking listener");
        let address = listener.local_addr().expect("the path's address");
        let stop = Arc::new(AtomicBool::new(false));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let (stop_loop, sent_loop) = (Arc::clone(&stop), Arc::clone(&sent));
        thread::spawn(move || {
            let connections = AtomicUsize::new(0);
            while !stop_loop.load(Ordering::Relaxed) {
                let Ok((client, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                };
                let first = connections.fetch_add(1, Ordering::Relaxed) == 0;
                let cut = rules.cut_first_after.filter(|_| first);
                carry(&client, target, &rules, cut, &sent_loop);
            }
        });
        Self {
            address,
            stop,
            sent,
        }
    }

    /// Where a client dials to go through the path.
    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    /// Every byte any client sent through the path so far, in order.
    #[must_use]
    pub fn sent_by_clients(&self) -> Vec<u8> {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for HostilePath {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Join `client` to `target` through `rules`, a thread each way.
fn carry(
    client: &TcpStream,
    target: SocketAddr,
    rules: &Rules,
    cut: Option<usize>,
    sent: &Arc<Mutex<Vec<u8>>>,
) {
    let _ = client.set_nonblocking(false);
    let Ok(server) = TcpStream::connect(target) else {
        return;
    };
    let carried = Arc::new(AtomicUsize::new(0));
    let ends = [
        (
            client.try_clone(),
            server.try_clone(),
            Some(Arc::clone(sent)),
        ),
        (server.try_clone(), client.try_clone(), None),
    ];
    for (from, to, record) in ends {
        let (Ok(from), Ok(to)) = (from, to) else {
            return;
        };
        let (rules, carried) = (rules.clone(), Arc::clone(&carried));
        thread::spawn(move || pump(from, to, &rules, cut, &carried, record.as_deref()));
    }
}

/// Close `stream` with a reset rather than a close: a zero linger.
fn reset(stream: &TcpStream) {
    let _ = socket2::SockRef::from(stream).set_linger(Some(Duration::ZERO));
    let _ = stream.shutdown(Shutdown::Both);
}

/// Copy one direction until either end closes or the cut is reached.
fn pump(
    mut from: TcpStream,
    mut to: TcpStream,
    rules: &Rules,
    cut: Option<usize>,
    carried: &AtomicUsize,
    record: Option<&Mutex<Vec<u8>>>,
) {
    let mut buffer = [0u8; 4096];
    let piece = if rules.trickle.is_some() {
        1
    } else {
        buffer.len()
    };
    loop {
        let read = match from.read(&mut buffer[..piece]) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        if let Some(record) = record {
            record
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(&buffer[..read]);
        }
        let total = carried.fetch_add(read, Ordering::Relaxed) + read;
        if cut.is_some_and(|cut| total > cut) {
            reset(&from);
            reset(&to);
            return;
        }
        let mut wait = rules.delay + rules.trickle.unwrap_or_default();
        if let Some(rate) = rules.rate {
            let micros = (read as u64).saturating_mul(1_000_000) / (rate.max(1) as u64);
            wait += Duration::from_micros(micros);
        }
        thread::sleep(wait);
        if to.write_all(&buffer[..read]).is_err() {
            break;
        }
    }
    let _ = from.shutdown(Shutdown::Both);
    let _ = to.shutdown(Shutdown::Both);
}
