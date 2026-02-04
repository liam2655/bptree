use async_trait::async_trait;
use std::error::Error;
use thiserror::Error;

pub type BlockId = u64;

/// Trait for persistent block storage backends
#[async_trait]
pub trait BlockStorage: Send + Sync {
    type Error: Error + Send + Sync + 'static;

    /// Read a block by ID
    async fn read_block(&self, id: BlockId) -> Result<Vec<u8>, Self::Error>;

    /// Write data to a block
    async fn write_block(&mut self, id: BlockId, data: &[u8]) -> Result<(), Self::Error>;

    /// Allocate a new block and return its ID
    async fn allocate_block(&mut self) -> Result<BlockId, Self::Error>;

    /// Deallocate a block (mark as free)
    async fn deallocate_block(&mut self, id: BlockId) -> Result<(), Self::Error>;

    /// Deallocate multiple blocks at once
    async fn deallocate_blocks(&mut self, ids: Vec<BlockId>) -> Result<(), Self::Error> {
        for id in ids {
            self.deallocate_block(id).await?;
        }
        Ok(())
    }

    /// Get the fixed block size for this storage
    fn block_size(&self) -> usize;

    /// Sync all pending writes to disk
    async fn sync(&mut self) -> Result<(), Self::Error>;
}

/// Common errors for block storage operations
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Block {0} not found")]
    BlockNotFound(BlockId),
    #[error("Block {0} is corrupted")]
    BlockCorrupted(BlockId),
    #[error("Out of storage space")]
    OutOfSpace,
    #[error("Invalid block size: expected {expected}, got {actual}")]
    InvalidBlockSize { expected: usize, actual: usize },
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Version mismatch: expected {expected}, got {actual}")]
    VersionMismatch { expected: u8, actual: u8 },
    #[error("Metadata mismatch: {0}")]
    MetadataMismatch(String),
}

impl From<bincode::Error> for StorageError {
    fn from(err: bincode::Error) -> Self {
        StorageError::Serialization(err.to_string())
    }
}
