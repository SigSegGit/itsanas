use std::path::PathBuf;

/// Everything the CLI can fail at, phrased for someone reading a terminal.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{0}")]
    Config(String),

    #[error("cryptographic failure: {0}")]
    Crypto(#[from] itsanas_crypto::CryptoError),

    #[error("store: {0}")]
    Store(#[from] itsanas_store::StoreError),

    #[error("network: {0}")]
    Net(#[from] itsanas_net::NetError),

    #[error("encoding: {0}")]
    Encoding(#[from] postcard::Error),

    #[error(
        "no node found at {0}.\n\
         Run `itsanas init` to create one, or `itsanas login` to restore an \
         existing account from its recovery phrase."
    )]
    NoNode(PathBuf),

    #[error(
        "a node already exists at {0}.\n\
         Refusing to overwrite it: doing so would destroy the master secret and \
         make every chunk stored under it permanently unreadable."
    )]
    NodeExists(PathBuf),

    #[error("wrong passphrase, or the keystore has been tampered with")]
    Unlock,

    #[error("{0}")]
    Usage(String),
}

pub type Result<T> = std::result::Result<T, CliError>;
