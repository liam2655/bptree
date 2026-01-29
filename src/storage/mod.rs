pub mod file;
pub mod storage_trait;

pub use file::FileBlockStorage;
pub use storage_trait::{BlockId, BlockStorage, StorageError};
