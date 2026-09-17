//! Establishing an encrypted, mutually authenticated connection.
//!
//! # The certificate is deliberately anonymous
//!
//! Each process generates a fresh self-signed certificate at start-up, with a
//! key that has nothing to do with its device identity. That looks alarming and
//! is not, because the certificate is not what authenticates anybody —
//! [`crate::auth`] is, by having each side sign the TLS session's exporter value
//! with its device key.
//!
//! Two things fall out of it. There is no X.509 parsing anywhere in the trusted
//! path, because nothing ever needs to pull a key back out of a certificate.
//! And an observer cannot correlate two connections by their certificates, since
//! a device presents a different one every time it starts.
//!
//! # What TLS is doing here
//!
//! Confidentiality and integrity, nothing else. It is what stops the metadata
//! leak that plain TCP had — chunk identifiers, object sizes and timing visible
//! to anyone on the path. Identity is the layer above.

use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use itsanas_crypto::{DeviceId, DeviceKeys};
use itsanas_wire::Connection;
use rustls::{
    ClientConfig, ClientConnection, ServerConfig, ServerConnection, SignatureScheme, StreamOwned,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{CryptoProvider, ring},
    pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime},
};

use crate::{
    auth::{self, AuthHello, EXPORTER_LABEL, EXPORTER_LEN},
    error::{Result, TlsError},
};

/// The name presented in the certificate.
///
/// Constant and meaningless: nothing checks it, because nothing about the
/// certificate is checked. It exists because a certificate must contain
/// something.
const CERTIFICATE_NAME: &str = "itsanas.invalid";

/// A process's TLS material.
#[derive(Debug, Clone)]
pub struct Identity {
    certificate: CertificateDer<'static>,
    key: Arc<PrivateKeyDer<'static>>,
}

impl Identity {
    /// Generate a fresh anonymous certificate for this process.
    pub fn generate() -> Result<Self> {
        let certified = rcgen::generate_simple_self_signed(vec![CERTIFICATE_NAME.to_owned()])?;

        Ok(Self {
            certificate: CertificateDer::from(certified.cert.der().to_vec()),
            key: Arc::new(
                PrivateKeyDer::try_from(certified.signing_key.serialize_der()).map_err(
                    |reason| {
                        TlsError::Io(std::io::Error::other(format!(
                            "generated key was not usable: {reason}"
                        )))
                    },
                )?,
            ),
        })
    }

    fn provider() -> Arc<CryptoProvider> {
        Arc::new(ring::default_provider())
    }

    /// Configuration for accepting connections.
    pub fn server_config(&self) -> Result<Arc<ServerConfig>> {
        let config = ServerConfig::builder_with_provider(Self::provider())
            .with_safe_default_protocol_versions()?
            // No client certificate is requested. Asking for one would prove
            // nothing that the exporter signature does not prove better, and
            // would put an unnecessary X.509 chain in the path.
            .with_no_client_auth()
            .with_single_cert(vec![self.certificate.clone()], self.key.clone_key())?;

        Ok(Arc::new(config))
    }

    /// Configuration for dialling out.
    pub fn client_config(&self) -> Result<Arc<ClientConfig>> {
        let provider = Self::provider();

        let config = ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AnonymousServer { provider }))
            .with_no_client_auth();

        Ok(Arc::new(config))
    }
}

/// Accepts any server certificate, because the certificate proves nothing.
///
/// The handshake signature is still verified properly — the session must be
/// cryptographically sound even though it is anonymous. What is skipped is only
/// the chain-of-trust check, which has nothing to chain to.
#[derive(Debug)]
struct AnonymousServer {
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for AnonymousServer {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// A connection whose peer has proved which device it is.
#[derive(Debug)]
pub struct Authenticated<S: Read + Write> {
    pub peer: DeviceId,
    pub connection: Connection<S>,
}

/// Server side of an incoming connection.
pub type ServerStream<S> = StreamOwned<ServerConnection, S>;
/// Client side of an outgoing connection.
pub type ClientStream<S> = StreamOwned<ClientConnection, S>;

/// Accept a connection and find out which device is on the other end.
///
/// The peer speaks first: it proves itself, then this side proves itself back.
/// Fixing the order matters — both sides waiting to receive is a deadlock, and
/// both sending first works only until a message is large enough to fill a
/// socket buffer.
pub fn accept<S: Read + Write>(
    config: &Arc<ServerConfig>,
    device: &DeviceKeys,
    mut stream: S,
) -> Result<Authenticated<ServerStream<S>>> {
    let mut session = ServerConnection::new(config.clone())?;

    // The exporter does not exist until the handshake is finished, and the
    // proof is over the exporter, so the handshake has to be driven to
    // completion before anything can be said.
    session.complete_io(&mut stream)?;
    let exporter = exporter_of(&session)?;

    let mut connection = Connection::new(StreamOwned::new(session, stream));

    let hello: AuthHello = connection.receive()?.ok_or(TlsError::NoProof)?;
    // A server takes callers as they come, so there is nobody to expect.
    let peer = auth::check(&hello, &exporter, None)?;

    connection.send(&auth::prove(device, &exporter))?;

    Ok(Authenticated { peer, connection })
}

/// A socket whose opening exchange has to finish by a fixed moment.
///
/// A read timeout alone bounds how long one read may stall, not how long the
/// handshake may take: a caller trickling a byte every twenty-nine seconds
/// never trips a thirty-second timeout and holds the connection for as long as
/// it likes. On a public port that is a free way to occupy every slot a server
/// has. So until [`accept_within`] disarms it, every read and write is given
/// only what is left of the deadline, and nothing at all once it has passed.
#[derive(Debug)]
pub struct Bounded {
    stream: TcpStream,
    until: Instant,
    armed: Arc<AtomicBool>,
    /// The per-read timeout for after the handshake, put back by the first
    /// read or write once the deadline is lifted.
    ///
    /// Restored through this handle rather than a clone of the socket: on
    /// Windows a duplicated socket handle does not share its timeouts, so
    /// setting them on a clone left the handshake's last few hundred
    /// milliseconds in force and cut off the authenticated peer. Found by
    /// `the_deadline_is_lifted_once_the_caller_has_authenticated`.
    idle: Option<Duration>,
}

impl Bounded {
    fn remaining(&mut self) -> std::io::Result<Option<Duration>> {
        if !self.armed.load(Ordering::Relaxed) {
            if let Some(idle) = self.idle.take() {
                self.stream.set_read_timeout(Some(idle))?;
                self.stream.set_write_timeout(Some(idle))?;
            }
            return Ok(None);
        }
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the caller did not finish proving who it is in time",
            ));
        }
        Ok(Some(left))
    }
}

impl Read for Bounded {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(left) = self.remaining()? {
            self.stream.set_read_timeout(Some(left))?;
        }
        self.stream.read(buf)
    }
}

impl Write for Bounded {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(left) = self.remaining()? {
            self.stream.set_write_timeout(Some(left))?;
        }
        self.stream.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

/// [`accept`], but the caller has `limit` to finish authenticating, in total.
///
/// Afterwards the socket goes back to `idle`, the per-read timeout the rest of
/// the conversation runs under. Only the part before the caller has proved
/// anything is bounded this way: an authenticated peer pushing a large chunk
/// over a slow link is doing what it is for.
pub fn accept_within(
    config: &Arc<ServerConfig>,
    device: &DeviceKeys,
    stream: TcpStream,
    limit: Duration,
    idle: Duration,
) -> Result<Authenticated<ServerStream<Bounded>>> {
    let armed = Arc::new(AtomicBool::new(true));
    let bounded = Bounded {
        stream,
        until: Instant::now() + limit,
        armed: Arc::clone(&armed),
        idle: Some(idle),
    };

    let authenticated = accept(config, device, bounded)?;

    // The wrapper now belongs to rustls, so it cannot be reached to change;
    // lowering the flag tells it to put the idle timeout back itself.
    armed.store(false, Ordering::Relaxed);
    Ok(authenticated)
}

/// Dial a peer and prove who each of you is.
///
/// `expected` pins the answer. Pass it whenever the identity is already known —
/// which is almost always, because addresses come from the coordinator and the
/// coordinator is not trusted to say who lives at one.
pub fn connect<S: Read + Write>(
    config: &Arc<ClientConfig>,
    device: &DeviceKeys,
    mut stream: S,
    expected: Option<DeviceId>,
) -> Result<Authenticated<ClientStream<S>>> {
    // A fixed name, because nothing checks it. Verifying a hostname against an
    // anonymous certificate would be theatre.
    let name = ServerName::try_from(CERTIFICATE_NAME)
        .map_err(|_| TlsError::Io(std::io::Error::other("invalid certificate name")))?
        .to_owned();

    let mut session = ClientConnection::new(config.clone(), name)?;
    session.complete_io(&mut stream)?;
    let exporter = exporter_of(&session)?;

    let mut connection = Connection::new(StreamOwned::new(session, stream));

    connection.send(&auth::prove(device, &exporter))?;

    let hello: AuthHello = connection.receive()?.ok_or(TlsError::NoProof)?;
    let peer = auth::check(&hello, &exporter, expected)?;

    Ok(Authenticated { peer, connection })
}

/// This session's channel binding material.
fn exporter_of<T>(session: &rustls::ConnectionCommon<T>) -> Result<[u8; EXPORTER_LEN]> {
    session
        .export_keying_material([0u8; EXPORTER_LEN], EXPORTER_LABEL, None)
        .map_err(|error| TlsError::NoExporter(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use itsanas_crypto::SecretBytes;
    use std::net::TcpListener;

    /// THE ATTACK: open a connection and feed the handshake one byte at a time,
    /// each a little inside the read timeout. No single read ever stalls long
    /// enough to trip it, so a per-read timeout never fires and the caller
    /// holds a server slot for as long as it keeps trickling. On a public port
    /// that is a way to fill every slot with nothing.
    #[test]
    fn red_team_a_handshake_trickled_a_byte_at_a_time_is_cut_off_at_the_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let config = Identity::generate().unwrap().server_config().unwrap();
        let device = DeviceKeys::from_seed(&SecretBytes::new([3; 32]));

        let trickler = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            // A TLS record header promising a full-sized record, so the server
            // keeps waiting for the rest rather than rejecting it on sight.
            let _ = stream.write_all(&[0x16, 0x03, 0x01, 0x40, 0x00]);
            for _ in 0..40 {
                if stream.write_all(&[0]).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });

        let (stream, _) = listener.accept().unwrap();
        let started = Instant::now();
        let outcome = accept_within(
            &config,
            &device,
            stream,
            Duration::from_millis(500),
            Duration::from_secs(30),
        );
        let took = started.elapsed();

        assert!(outcome.is_err(), "a trickled handshake was accepted");
        assert!(
            took < Duration::from_secs(2),
            "a caller trickling bytes held the handshake for {took:?} against a \
             500 ms deadline; every slot on a public listener can be held this way"
        );
        let _ = trickler.join();
    }

    /// The deadline is for proving who you are, not for the conversation after.
    /// Left armed, it would cut off an honest peer pushing a large chunk over
    /// a slow link fifteen seconds into its session.
    #[test]
    fn the_deadline_is_lifted_once_the_caller_has_authenticated() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server_config = Identity::generate().unwrap().server_config().unwrap();
        let client_config = Identity::generate().unwrap().client_config().unwrap();
        let server_device = DeviceKeys::from_seed(&SecretBytes::new([4; 32]));
        let client_device = DeviceKeys::from_seed(&SecretBytes::new([5; 32]));

        let client = std::thread::spawn(move || {
            let stream = TcpStream::connect(address).unwrap();
            let Authenticated { mut connection, .. } =
                connect(&client_config, &client_device, stream, None).unwrap();
            // Longer than the deadline, after authenticating.
            std::thread::sleep(Duration::from_millis(800));
            connection.send(&7u32).unwrap();
        });

        let (stream, _) = listener.accept().unwrap();
        let Authenticated { mut connection, .. } = accept_within(
            &server_config,
            &server_device,
            stream,
            Duration::from_millis(400),
            Duration::from_secs(30),
        )
        .unwrap();
        let received: Option<u32> = connection.receive().unwrap_or(None);
        assert_eq!(
            received,
            Some(7),
            "an authenticated peer was cut off by the handshake deadline"
        );
        client.join().unwrap();
    }
}
