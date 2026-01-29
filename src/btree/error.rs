use crate::storage::StorageError;

/// B-tree specific errors
#[derive(Debug)]
pub enum BTreeError {
    Storage(StorageError),
    KeyNotFound,
    NodeCorrupted,
    InvalidOperation,
}

impl std::fmt::Display for BTreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BTreeError::Storage(err) => write!(f, "Storage error: {}", err),
            BTreeError::KeyNotFound => write!(f, "Key not found"),
            BTreeError::NodeCorrupted => write!(f, "Node corrupted"),
            BTreeError::InvalidOperation => write!(f, "Invalid operation"),
        }
    }
}

impl std::error::Error for BTreeError {}

impl From<StorageError> for BTreeError {
    fn from(err: StorageError) -> Self {
        BTreeError::Storage(err)
    }
}
