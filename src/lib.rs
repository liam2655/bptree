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
//! - Range queries (`range()`)
//! - Integrity validation (`validate()`)
//!
//! # Example
//!
//! ```rust
//! use bptree::{BPTree, FileBlockStorage};
//! use tempfile::TempDir;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let temp_dir = TempDir::new()?;
//! // Create storage with 4KB blocks
//! let storage = FileBlockStorage::new(temp_dir.path(), 4096)?;
//!
//! // Create B-tree for string keys and values
//! let mut bptree: BPTree<String, String, _> = BPTree::new(storage).await?;
//!
//! // Insert data
//! bptree.insert("hello".to_string(), "world".to_string()).await?;
//! bptree.insert("foo".to_string(), "bar".to_string()).await?;
//!
//! // Check size
//! assert_eq!(bptree.len(), 2);
//!
//! // Retrieve data
//! if let Some(value) = bptree.get(&"hello".to_string()).await? {
//!     println!("Value: {}", value);
//! }
//!
//! // Iterate through all entries
//! for (key, value) in bptree.range(..).await? {
//!     println!("{}: {}", key, value);
//! }
//!
//! // Range query
//! for (key, value) in bptree.range("a".to_string().."m".to_string()).await? {
//!     println!("Range match: {}: {}", key, value);
//! }
//!
//! // Delete data
//! bptree.delete(&"hello".to_string()).await?;
//! assert_eq!(bptree.len(), 1);
//!
//! // Validate integrity
//! bptree.validate().await?;
//!
//! # Ok(())
//! # }
//! ```

pub mod bptree;
pub mod ser;
pub mod storage;

pub use bptree::{BPTree, BPTreeError, UniversalNode};
pub use ser::BlockSerializer;
pub use storage::{BlockId, BlockStorage, FileBlockStorage, StorageError};
