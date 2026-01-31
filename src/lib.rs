//! Persistent B-tree implementation with pluggable storage backends
//!
//! This library provides a flexible B-tree implementation where the block storage
//! interface is abstracted through a trait, allowing different storage backends.
//!
//! # Features
//!
//! - Configurable block size
//! - Unified node structure for both internal and leaf nodes
//! - File-based storage with flat directory structure
//! - Serde support for key/value types
//! - Implicit node type detection
//! - Entry count tracking (`len()`, `is_empty()`)
//! - Iterator support (`iter()`, `iter_range()`)
//! - Integrity validation (`validate()`)
//!
//! # Example
//!
//! ```rust
//! use btree::{BTree, FileBlockStorage};
//! use tempfile::TempDir;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let temp_dir = TempDir::new()?;
//! // Create storage with 4KB blocks
//! let storage = FileBlockStorage::new(temp_dir.path(), 4096)?;
//!
//! // Create B-tree for string keys and values
//! let mut btree: BTree<String, String, _> = BTree::new(storage)?;
//!
//! // Insert data
//! btree.insert("hello".to_string(), "world".to_string())?;
//! btree.insert("foo".to_string(), "bar".to_string())?;
//!
//! // Check size
//! assert_eq!(btree.len(), 2);
//!
//! // Retrieve data
//! if let Some(value) = btree.get(&"hello".to_string())? {
//!     println!("Value: {}", value);
//! }
//!
//! // Iterate through all entries
//! for result in btree.iter()? {
//!     let (key, value) = result?;
//!     println!("{}: {}", key, value);
//! }
//!
//! // Range query
//! for result in btree.iter_range("a".to_string().."m".to_string())? {
//!     let (key, value) = result?;
//!     println!("Range match: {}: {}", key, value);
//! }
//!
//! // Delete data
//! btree.delete(&"hello".to_string())?;
//! assert_eq!(btree.len(), 1);
//!
//! // Validate integrity
//! btree.validate()?;
//!
//! # Ok(())
//! # }
//! ```

pub mod btree;
pub mod ser;
pub mod storage;

pub use btree::{BTree, BTreeError, UniversalNode, BTreeIter, BTreeRangeIter};
pub use ser::BlockSerializer;
pub use storage::{BlockId, BlockStorage, FileBlockStorage, StorageError};
