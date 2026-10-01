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

    /// Something the transport or the peer refused, with a runtime reason.
    ///
    /// Separate from [`CoordError::Rejected`], which carries a compile-time
    /// constant naming a rule this build enforces. This one carries text
    /// assembled at runtime, often about a stranger's connection — keeping them
    /// apart means the rules stay greppable.
    #[error("{0}")]
    Transport(String),

    #[error("the username {0:?} is already registered to a different key")]
    NameTaken(String),

    #[error("no such account: {0}")]
    NoSuchAccount(String),

    #[error("device {0} is not claimed by any registered account")]
    UnclaimedDevice(String),

    /// A live claim for a new device on an account already at
    /// [`crate::MAX_DEVICES_PER_ACCOUNT`].
    ///
    /// Carries the devices so the refusal is something a person can act on:
    /// it reaches every client as `Response::Refused` text, including clients
    /// older than the bound, which know nothing of it and print what they are
    /// given. The command named is `forget`, which every client has.
    #[error(
        "this account already has {live} live devices and the limit is {limit}: {devices}. \
         Its devices keep working; to add this one, withdraw one of them with \
         `itsanas device forget <id>`, from any machine of the account, this one included, \
         which frees its slot"
    )]
    TooManyDevices {
        live: usize,
        limit: usize,
        devices: String,
    },
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
