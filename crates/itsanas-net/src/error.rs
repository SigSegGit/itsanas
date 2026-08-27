use itsanas_store::StoreError;

/// Everything that can go wrong talking to a peer.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("store: {0}")]
    Store(Box<StoreError>),

    #[error("encoding: {0}")]
    Encoding(#[from] postcard::Error),

    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),

    #[error("the frame is incomplete")]
    Truncated,

    #[error("peer sent a {len}-byte frame; this build accepts at most {max}")]
    FrameTooLarge { len: usize, max: usize },

    #[error("peer speaks wire version {found}; this build speaks {supported}")]
    UnsupportedWireVersion { found: u8, supported: u8 },

    #[error("peer speaks protocol version {found}; this build speaks {supported}")]
    UnsupportedProtocolVersion { found: u16, supported: u16 },

    #[error("peer refused the request: {0}")]
    Refused(String),

    #[error("peer answered a {expected} request with something else")]
    UnexpectedResponse { expected: &'static str },

    #[error("peer failed a storage challenge for chunk {0}")]
    ChallengeFailed(String),

    #[error("the connection closed before the exchange finished")]
    ConnectionClosed,
}

impl From<StoreError> for NetError {
    fn from(error: StoreError) -> Self {
        Self::Store(Box::new(error))
    }
}

/// Result type used throughout the network layer.
pub type Result<T> = std::result::Result<T, NetError>;
