use crate::storage::StorageError;

/// B-tree specific errors
#[derive(Debug)]
pub enum BPTreeError {
    Storage(StorageError),
    KeyNotFound,
    NodeCorrupted,
    InvalidOperation,
    Underflow,
    MergeFailed,
    SiblingNotFound,
    ValidationFailed(String),
}

impl std::fmt::Display for BPTreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BPTreeError::Storage(err) => write!(f, "Storage error: {}", err),
            BPTreeError::KeyNotFound => write!(f, "Key not found"),
            BPTreeError::NodeCorrupted => write!(f, "Node corrupted"),
            BPTreeError::InvalidOperation => write!(f, "Invalid operation"),
            BPTreeError::Underflow => write!(f, "Node underflow"),
            BPTreeError::MergeFailed => write!(f, "Cannot merge with siblings"),
            BPTreeError::SiblingNotFound => write!(f, "No sibling available for rebalancing"),
            BPTreeError::ValidationFailed(msg) => write!(f, "Validation failed: {}", msg),
        }
    }
}

impl std::error::Error for BPTreeError {}

impl From<StorageError> for BPTreeError {
    fn from(err: StorageError) -> Self {
        BPTreeError::Storage(err)
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
