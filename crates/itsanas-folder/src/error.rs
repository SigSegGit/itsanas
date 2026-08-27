use std::path::PathBuf;

use itsanas_store::StoreError;

/// Everything that can go wrong mirroring a directory.
#[derive(Debug, thiserror::Error)]
pub enum FolderError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("store: {0}")]
    Store(Box<StoreError>),

    #[error(
        "refusing to act on {0}: it resolves outside the synced folder. \
         A path arriving from a peer must never be able to reach the rest of \
         the filesystem."
    )]
    Escapes(PathBuf),

    #[error("the synced folder {0} does not exist")]
    NoFolder(PathBuf),

    #[error("file watcher: {0}")]
    Watch(#[from] notify::Error),
}

impl From<StoreError> for FolderError {
    fn from(error: StoreError) -> Self {
        Self::Store(Box::new(error))
    }
}

pub type Result<T> = std::result::Result<T, FolderError>;

impl FolderError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
