use std::path::PathBuf;

use itsanas_crypto::CryptoError;

/// Everything that can go wrong below the sync engine.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("i/o error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("i/o error: {0}")]
    BareIo(#[from] std::io::Error),

    #[error("cryptographic failure: {0}")]
    Crypto(#[from] CryptoError),

    #[error("index database: {0}")]
    Database(Box<redb::Error>),

    #[error("encoding: {0}")]
    Encoding(#[from] postcard::Error),

    #[error("chunk {0} is referenced by the index but missing from the blob store")]
    MissingChunk(String),

    #[error("segment {segment} is signed by a device that does not match its envelope")]
    SegmentSignature { segment: String },

    #[error(
        "segment {segment} claims to follow {expected}, but the previous segment on this chain is {found}"
    )]
    SegmentChainBroken {
        segment: String,
        expected: String,
        found: String,
    },

    #[error("path {0:?} is not valid for a store: {1}")]
    InvalidPath(String, &'static str),

    #[error("invalid chunker configuration: {0}")]
    ChunkerConfig(&'static str),

    #[error(
        "{0} is already open in another process.\n\
         Only one process at a time may hold a node's state — most likely \
         `itsanas serve` is running. Stop it and try again."
    )]
    Locked(PathBuf),

    #[error(
        "refusing to open a store for published test identity {0}: its recovery \
         phrase is printed in the documentation, so its data is public"
    )]
    PublishedTestIdentity(String),

    #[error("{0}")]
    Corrupt(String),
}

/// Result type used throughout the store.
pub type Result<T> = std::result::Result<T, StoreError>;

impl StoreError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

macro_rules! from_redb {
    ($($ty:ty),* $(,)?) => {
        $(
            impl From<$ty> for StoreError {
                fn from(error: $ty) -> Self {
                    Self::Database(Box::new(error.into()))
                }
            }
        )*
    };
}

from_redb!(
    redb::Error,
    redb::DatabaseError,
    redb::TransactionError,
    redb::TableError,
    redb::StorageError,
    redb::CommitError,
);
