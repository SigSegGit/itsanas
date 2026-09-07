use std::path::PathBuf;

/// Everything opening or driving a node can fail at, phrased for a person.
///
/// Written for a terminal reader first, and every shell shows the same words:
/// an Android dialog saying "wrong passphrase, or the keystore has been
/// tampered with" is telling the truth in the same terms as the command line,
/// which is what makes a support conversation possible at all.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
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

    #[error("synced folder: {0}")]
    Folder(#[from] itsanas_folder::FolderError),

    /// Something the coordinator, or the connection to it, refused.
    #[error("coordinator: {0}")]
    Coord(#[from] itsanas_coord::CoordError),

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

pub type Result<T> = std::result::Result<T, NodeError>;
