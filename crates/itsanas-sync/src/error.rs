use itsanas_store::StoreError;

/// Everything that can go wrong while merging two devices' histories.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("store: {0}")]
    Store(Box<StoreError>),

    /// A host refused or failed to serve a chunk.
    ///
    /// Distinct from a chunk being *absent*, which is not an error — an absent
    /// chunk means the device holding it is asleep, and the operation is simply
    /// retried later.
    #[error("chunk source: {0}")]
    Source(String),
}

impl From<StoreError> for SyncError {
    fn from(error: StoreError) -> Self {
        Self::Store(Box::new(error))
    }
}

/// Result type used throughout the sync engine.
pub type Result<T> = std::result::Result<T, SyncError>;
