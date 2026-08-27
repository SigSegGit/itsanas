/// Everything the coordinator can refuse or fail at.
#[derive(Debug, thiserror::Error)]
pub enum CoordError {
    #[error("index database: {0}")]
    Database(Box<redb::Error>),

    #[error("encoding: {0}")]
    Encoding(#[from] postcard::Error),

    #[error("framing: {0}")]
    Wire(#[from] itsanas_wire::WireError),

    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),

    #[error("cryptographic failure: {0}")]
    Crypto(#[from] itsanas_crypto::CryptoError),

    #[error("the {0} signature does not verify")]
    BadSignature(&'static str),

    #[error(
        "refusing a message dated {issued} when it is {now}: supersession is by \
         timestamp, so a message from the future could never be replaced"
    )]
    FromTheFuture { issued: u64, now: u64 },

    #[error("refused: {0}")]
    Rejected(&'static str),

    #[error("the username {0:?} is already registered to a different key")]
    NameTaken(String),

    #[error("no such account: {0}")]
    NoSuchAccount(String),

    #[error("device {0} is not claimed by any registered account")]
    UnclaimedDevice(String),
}

pub type Result<T> = std::result::Result<T, CoordError>;

macro_rules! from_redb {
    ($($ty:ty),* $(,)?) => {
        $(
            impl From<$ty> for CoordError {
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
