/// Everything that can go wrong establishing an authenticated channel.
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("tls: {0}")]
    Rustls(#[from] rustls::Error),

    #[error("could not build this device's certificate: {0}")]
    Certificate(#[from] rcgen::Error),

    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),

    #[error("framing: {0}")]
    Stream(#[from] itsanas_wire::StreamError),

    #[error(
        "device {0} did not prove possession of its key; the connection is \
         encrypted but the peer is not who it says it is"
    )]
    AuthenticationFailed(String),

    #[error("expected to reach device {expected} but {found} answered")]
    WrongPeer { expected: String, found: String },

    #[error("the peer closed before proving who it was")]
    NoProof,

    #[error("the tls session refused to produce channel binding material: {0}")]
    NoExporter(String),
}

pub type Result<T> = std::result::Result<T, TlsError>;
