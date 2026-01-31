use crate::storage::StorageError;

/// B-tree specific errors
#[derive(Debug)]
pub enum BTreeError {
    Storage(StorageError),
    KeyNotFound,
    NodeCorrupted,
    InvalidOperation,
    Underflow,
    MergeFailed,
    SiblingNotFound,
    ValidationFailed(String),
}

impl std::fmt::Display for BTreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BTreeError::Storage(err) => write!(f, "Storage error: {}", err),
            BTreeError::KeyNotFound => write!(f, "Key not found"),
            BTreeError::NodeCorrupted => write!(f, "Node corrupted"),
            BTreeError::InvalidOperation => write!(f, "Invalid operation"),
            BTreeError::Underflow => write!(f, "Node underflow"),
            BTreeError::MergeFailed => write!(f, "Cannot merge with siblings"),
            BTreeError::SiblingNotFound => write!(f, "No sibling available for rebalancing"),
            BTreeError::ValidationFailed(msg) => write!(f, "Validation failed: {}", msg),
        }
    }
}

impl std::error::Error for BTreeError {}

impl From<StorageError> for BTreeError {
    fn from(err: StorageError) -> Self {
        BTreeError::Storage(err)
    }
}

/// Result of a deletion operation
#[derive(Debug)]
pub enum DeleteResult<K, V> {
    NoChange,
    KeyRemoved(V),
    Underflow {
        node_id: crate::storage::BlockId,
        value: Option<V>,
        left_sibling: Option<crate::storage::BlockId>,
        right_sibling: Option<crate::storage::BlockId>,
        new_separator: Option<K>,
    },
    TreeEmpty,
}
