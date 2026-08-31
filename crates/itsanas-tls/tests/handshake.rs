//! A real TLS handshake between two devices over a real socket.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
};

use itsanas_crypto::{DeviceKeys, SecretBytes};
use itsanas_tls::{Identity, accept, connect};
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Note {
    text: String,
}

fn device(byte: u8) -> DeviceKeys {
    DeviceKeys::from_seed(&SecretBytes::new([byte; 32]))
}

/// A stream that keeps a copy of every byte written to the socket.
///
/// Used to prove the payload is encrypted before it leaves the process, which
/// is the whole reason TLS is here.
struct Recording {
    inner: TcpStream,
    written: Arc<Mutex<Vec<u8>>>,
}

impl Read for Recording {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(out)
    }
}

impl Write for Recording {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.written
            .lock()
            .expect("not poisoned")
            .extend_from_slice(data);
        self.inner.write(data)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Unblocks a server thread parked in `accept` however the body is left.
///
/// Each test here spawns a thread that blocks on `TcpListener::accept` and then
/// drives a client from the test thread. If the client half panics *before* it
/// connects — a failed assertion, a key that would not generate — nothing
/// ever accepts, `thread::scope` joins a thread that will never return, and the
/// harness reports a **hang** instead of the assertion.
///
/// That is the same defect that made every security test in `itsanas-net`
/// report a sixty-second timeout when it caught an attack. The guard existed in
/// `itsanas-coord`'s harness from the first day and was applied nowhere else,
/// which is how a good idea in one file becomes an outage in another.
struct UnblockOnDrop(std::net::SocketAddr);

impl Drop for UnblockOnDrop {
    fn drop(&mut self) {
        let _ = TcpStream::connect(self.0);
    }
}

#[test]
fn two_devices_authenticate_each_other_and_exchange_a_message() {
    let server_device = device(1);
    let client_device = device(2);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();

    let server_identity = Identity::generate().unwrap();
    let server_config = server_identity.server_config().unwrap();

    let server_id = server_device.device_id();
    let client_id = client_device.device_id();

    let _unblock = UnblockOnDrop(address);
    std::thread::scope(|scope| {
        let handle = scope.spawn(|| {
            let (stream, _) = listener.accept().unwrap();
            let mut session = accept(&server_config, &server_device, stream).unwrap();

            assert_eq!(
                session.peer, client_id,
                "the server learned the wrong client identity"
            );

            let note: Note = session.connection.receive().unwrap().unwrap();
            session
                .connection
                .send(&Note {
                    text: format!("heard: {}", note.text),
                })
                .unwrap();
        });

        let client_identity = Identity::generate().unwrap();
        let client_config = client_identity.client_config().unwrap();

        let stream = TcpStream::connect(address).unwrap();
        let mut session = connect(&client_config, &client_device, stream, Some(server_id)).unwrap();

        assert_eq!(session.peer, server_id);

        session
            .connection
            .send(&Note {
                text: "hello".to_owned(),
            })
            .unwrap();

        let reply: Note = session.connection.receive().unwrap().unwrap();
        assert_eq!(reply.text, "heard: hello");

        handle.join().unwrap();
    });
}

#[test]
fn the_payload_never_reaches_the_socket_in_plaintext() {
    // The metadata leak that plain TCP had is the reason this layer exists.
    // Everything written to the socket is captured and scanned for a canary
    // that was sent through the session.
    const CANARY: &str = "ITSANAS-TLS-CANARY-7b3f91ac";

    let server_device = device(3);
    let client_device = device(4);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();

    let server_identity = Identity::generate().unwrap();
    let server_config = server_identity.server_config().unwrap();
    let server_id = server_device.device_id();

    let recorded = Arc::new(Mutex::new(Vec::new()));

    let _unblock = UnblockOnDrop(address);
    std::thread::scope(|scope| {
        let handle = scope.spawn(|| {
            let (stream, _) = listener.accept().unwrap();
            let mut session = accept(&server_config, &server_device, stream).unwrap();
            let _: Note = session.connection.receive().unwrap().unwrap();
            session
                .connection
                .send(&Note {
                    text: "ack".to_owned(),
                })
                .unwrap();
        });

        let client_identity = Identity::generate().unwrap();
        let client_config = client_identity.client_config().unwrap();

        let stream = Recording {
            inner: TcpStream::connect(address).unwrap(),
            written: Arc::clone(&recorded),
        };

        let mut session = connect(&client_config, &client_device, stream, Some(server_id)).unwrap();

        session
            .connection
            .send(&Note {
                text: CANARY.to_owned(),
            })
            .unwrap();
        let _: Note = session.connection.receive().unwrap().unwrap();

        handle.join().unwrap();
    });

    let on_the_wire = recorded.lock().unwrap();
    assert!(
        !on_the_wire.is_empty(),
        "nothing was recorded, so this proves nothing"
    );
    assert!(
        !on_the_wire
            .windows(CANARY.len())
            .any(|window| window == CANARY.as_bytes()),
        "the payload went onto the socket in plaintext"
    );
}

#[test]
fn dialling_a_device_and_reaching_a_different_one_is_refused() {
    // The attack this closes: the coordinator hands out addresses, and it is
    // not trusted to say who lives at one. Without pinning, it could point a
    // caller at a machine of its choosing.
    let actual_server = device(5);
    let client_device = device(6);
    let someone_else = device(7).device_id();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();

    let server_identity = Identity::generate().unwrap();
    let server_config = server_identity.server_config().unwrap();

    let _unblock = UnblockOnDrop(address);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            if let Ok((stream, _)) = listener.accept() {
                // The server side may fail once the client hangs up; that is
                // the expected shape of this test.
                let _ = accept(&server_config, &actual_server, stream);
            }
        });

        let client_identity = Identity::generate().unwrap();
        let client_config = client_identity.client_config().unwrap();
        let stream = TcpStream::connect(address).unwrap();

        let outcome = connect(&client_config, &client_device, stream, Some(someone_else));

        assert!(
            matches!(outcome, Err(itsanas_tls::TlsError::WrongPeer { .. })),
            "a caller accepted a connection to a device it did not ask for"
        );
    });
}

#[test]
fn a_server_learns_who_called_without_being_told_in_advance() {
    // A host accepts callers it has never met — that is the point of a network
    // where anyone can offer storage. It still needs to know who they are.
    let server_device = device(8);
    let client_device = device(9);
    let client_id = client_device.device_id();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();

    let server_identity = Identity::generate().unwrap();
    let server_config = server_identity.server_config().unwrap();

    let _unblock = UnblockOnDrop(address);
    std::thread::scope(|scope| {
        let handle = scope.spawn(|| {
            let (stream, _) = listener.accept().unwrap();
            accept(&server_config, &server_device, stream).unwrap().peer
        });

        let client_identity = Identity::generate().unwrap();
        let client_config = client_identity.client_config().unwrap();
        let stream = TcpStream::connect(address).unwrap();
        let _ = connect(&client_config, &client_device, stream, None).unwrap();

        assert_eq!(handle.join().unwrap(), client_id);
    });
}

#[test]
fn every_process_presents_a_different_certificate() {
    // An observer must not be able to correlate two connections by their
    // certificates. Identity is proved at the application layer precisely so
    // the certificate can carry nothing.
    let first = Identity::generate().unwrap();
    let second = Identity::generate().unwrap();

    assert_ne!(
        format!("{first:?}"),
        format!("{second:?}"),
        "two processes generated identical certificates"
    );
}
