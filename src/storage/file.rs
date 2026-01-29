use super::storage_trait::{BlockId, BlockStorage, StorageError};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// File-based block storage with flat directory structure
pub struct FileBlockStorage {
    directory: PathBuf,
    block_size: usize,
    next_block_id: BlockId,
    free_blocks: Vec<BlockId>,
}

impl FileBlockStorage {
    /// Create or open a file-based storage in the given directory
    pub fn new<P: AsRef<Path>>(directory: P, block_size: usize) -> Result<Self, StorageError> {
        let directory = directory.as_ref().to_path_buf();

        // Create directory if it doesn't exist
        fs::create_dir_all(&directory)?;

        let mut storage = Self {
            directory,
            block_size,
            next_block_id: 0,
            free_blocks: Vec::new(),
        };

        // Load existing metadata or initialize new storage
        storage.load_or_init_metadata()?;

        Ok(storage)
    }

    /// Get the file path for a block ID
    fn block_path(&self, id: BlockId) -> PathBuf {
        self.directory.join(format!("block_{}", id))
    }

    /// Get the metadata file path
    fn metadata_path(&self) -> PathBuf {
        self.directory.join("metadata.bin")
    }

    /// Load existing metadata or initialize new storage
    fn load_or_init_metadata(&mut self) -> Result<(), StorageError> {
        let metadata_path = self.metadata_path();

        if metadata_path.exists() {
            // Load existing metadata
            let mut file = File::open(&metadata_path)?;
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;

            let metadata: FileMetadata = bincode::deserialize(&buf)?;
            self.next_block_id = metadata.next_block_id;
            self.free_blocks = metadata.free_blocks;
        } else {
            // Initialize new storage
            self.save_metadata()?;
        }

        Ok(())
    }

    /// Save metadata to file
    fn save_metadata(&self) -> Result<(), StorageError> {
        let metadata = FileMetadata {
            next_block_id: self.next_block_id,
            free_blocks: self.free_blocks.clone(),
        };

        let data = bincode::serialize(&metadata)?;

        let temp_path = self.metadata_path().with_extension("tmp");
        let mut file = File::create(&temp_path)?;
        file.write_all(&data)?;
        file.sync_all()?;

        // Atomic rename
        fs::rename(&temp_path, self.metadata_path())?;

        Ok(())
    }

    /// Write data to file with validation
    fn write_block_file(&self, path: &Path, data: &[u8]) -> Result<(), StorageError> {
        if data.len() != self.block_size {
            return Err(StorageError::InvalidBlockSize {
                expected: self.block_size,
                actual: data.len(),
            });
        }

        let temp_path = path.with_extension("tmp");
        let mut file = File::create(&temp_path)?;
        file.write_all(data)?;
        file.sync_all()?;

        // Atomic rename
        fs::rename(&temp_path, path)?;

        Ok(())
    }
}

impl BlockStorage for FileBlockStorage {
    type Error = StorageError;

    fn read_block(&self, id: BlockId) -> Result<Vec<u8>, Self::Error> {
        let path = self.block_path(id);

        if !path.exists() {
            return Err(StorageError::BlockNotFound(id));
        }

        let mut file = File::open(&path)?;
        let mut data = vec![0u8; self.block_size];
        file.read_exact(&mut data)?;

        Ok(data)
    }

    fn write_block(&mut self, id: BlockId, data: &[u8]) -> Result<(), Self::Error> {
        let path = self.block_path(id);
        self.write_block_file(&path, data)?;
        self.save_metadata()
    }

    fn allocate_block(&mut self) -> Result<BlockId, Self::Error> {
        let block_id = if let Some(free_id) = self.free_blocks.pop() {
            free_id
        } else {
            let id = self.next_block_id;
            self.next_block_id += 1;
            id
        };

        // Create empty block file
        let path = self.block_path(block_id);
        let empty_data = vec![0u8; self.block_size];
        self.write_block_file(&path, &empty_data)?;
        self.save_metadata()?;

        Ok(block_id)
    }

    fn deallocate_block(&mut self, id: BlockId) -> Result<(), Self::Error> {
        let path = self.block_path(id);

        if path.exists() {
            fs::remove_file(&path)?;
            self.free_blocks.push(id);
            self.save_metadata()?;
        } else {
            return Err(StorageError::BlockNotFound(id));
        }

        Ok(())
    }

    fn block_size(&self) -> usize {
        self.block_size
    }

    fn sync(&mut self) -> Result<(), Self::Error> {
        self.save_metadata()
    }

    fn total_blocks(&self) -> Result<BlockId, Self::Error> {
        Ok(self.next_block_id)
    }

    fn free_blocks(&self) -> Result<BlockId, Self::Error> {
        Ok(self.free_blocks.len() as BlockId)
    }
}

/// Metadata stored alongside the block files
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct FileMetadata {
    next_block_id: BlockId,
    free_blocks: Vec<BlockId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_file_storage_creation() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        assert_eq!(storage.block_size(), 4096);
        assert_eq!(storage.total_blocks().unwrap(), 0);
    }

    #[test]
    fn test_block_allocation() {
        let temp_dir = TempDir::new().unwrap();
        let mut storage = FileBlockStorage::new(temp_dir.path(), 1024).unwrap();

        let block_id = storage.allocate_block().unwrap();
        assert_eq!(block_id, 0);
        assert_eq!(storage.total_blocks().unwrap(), 1);

        // Check block file exists
        assert!(storage.block_path(block_id).exists());
    }

    #[test]
    fn test_block_write_read() {
        let temp_dir = TempDir::new().unwrap();
        let mut storage = FileBlockStorage::new(temp_dir.path(), 1024).unwrap();

        let block_id = storage.allocate_block().unwrap();
        let data = vec![42u8; 1024];

        storage.write_block(block_id, &data).unwrap();
        let read_data = storage.read_block(block_id).unwrap();

        assert_eq!(data, read_data);
    }

    #[test]
    fn test_block_deallocation() {
        let temp_dir = TempDir::new().unwrap();
        let mut storage = FileBlockStorage::new(temp_dir.path(), 1024).unwrap();

        let block_id = storage.allocate_block().unwrap();
        storage.deallocate_block(block_id).unwrap();

        assert!(!storage.block_path(block_id).exists());
        assert_eq!(storage.free_blocks().unwrap(), 1);
    }
}
