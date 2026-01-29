use crate::btree::node::UniversalNode;
use crate::ser::BlockSerializer;
use crate::storage::{BlockId, BlockStorage, StorageError};
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;

/// B-tree metadata stored in a special block
#[derive(Debug, Serialize, Deserialize)]
struct BTreeMetadata {
    root_id: Option<BlockId>,
    block_size: usize,
    max_keys_per_node: usize,
    next_key_id: u64,
}

/// Persistent B-tree implementation with pluggable storage
pub struct BTree<K, V, S>
where
    K: Ord + Clone + Serialize + for<'de> Deserialize<'de>,
    V: Clone + Serialize + for<'de> Deserialize<'de>,
    S: BlockStorage<Error = StorageError>,
{
    storage: S,
    metadata_id: BlockId,
    root_id: Option<BlockId>,
    max_keys_per_node: usize,
    _phantom: PhantomData<(K, V)>,
}

impl<K, V, S> BTree<K, V, S>
where
    K: Ord + Clone + Serialize + for<'de> Deserialize<'de>,
    V: Clone + Serialize + for<'de> Deserialize<'de>,
    S: BlockStorage<Error = StorageError>,
{
    pub fn new(storage: S) -> Result<Self, StorageError> {
        let block_size = storage.block_size();
        let max_keys_per_node = Self::estimate_max_keys(block_size);

        let mut btree = Self {
            storage,
            metadata_id: 0,
            root_id: None,
            max_keys_per_node,
            _phantom: PhantomData,
        };

        btree.load_or_init_metadata()?;
        Ok(btree)
    }

    fn estimate_max_keys(block_size: usize) -> usize {
        let overhead = 100;
        let pair_size = std::mem::size_of::<K>() + std::mem::size_of::<V>() + 20;
        ((block_size - overhead) / pair_size).max(2)
    }

    fn load_or_init_metadata(&mut self) -> Result<(), StorageError> {
        match self.storage.read_block(self.metadata_id) {
            Ok(data) => {
                if let Ok(metadata) = bincode::deserialize::<BTreeMetadata>(&data) {
                    self.root_id = metadata.root_id;
                    self.max_keys_per_node = metadata.max_keys_per_node;
                    Ok(())
                } else {
                    self.initialize_metadata()
                }
            }
            Err(StorageError::BlockNotFound(_)) => {
                // Block doesn't exist, initialize new metadata
                self.initialize_metadata()
            }
            Err(e) => Err(e),
        }
    }

    fn initialize_metadata(&mut self) -> Result<(), StorageError> {
        // Allocate block 0 for metadata if it doesn't exist
        if self.storage.read_block(self.metadata_id).is_err() {
            self.storage.allocate_block()?;
        }

        let metadata = BTreeMetadata {
            root_id: None,
            block_size: self.storage.block_size(),
            max_keys_per_node: self.max_keys_per_node,
            next_key_id: 0,
        };

        let metadata_data = bincode::serialize(&metadata)?;
        let padded_data = BlockSerializer::pad_to_block(metadata_data, self.storage.block_size());

        self.storage.write_block(self.metadata_id, &padded_data)?;
        self.storage.sync()?;

        self.root_id = None;
        Ok(())
    }

    fn allocate_node(&mut self) -> Result<BlockId, StorageError> {
        self.storage.allocate_block()
    }

    fn save_node(
        &mut self,
        node_id: BlockId,
        node: &UniversalNode<K, V>,
    ) -> Result<(), StorageError> {
        let data = bincode::serialize(node)?;
        let padded_data = BlockSerializer::pad_to_block(data, self.storage.block_size());
        self.storage.write_block(node_id, &padded_data)?;
        self.storage.sync()?;
        Ok(())
    }

    fn load_node(&self, node_id: BlockId) -> Result<UniversalNode<K, V>, StorageError> {
        let data = self.storage.read_block(node_id)?;
        let node: UniversalNode<K, V> = bincode::deserialize(&data)?;
        Ok(node)
    }

    fn save_metadata(&mut self) -> Result<(), StorageError> {
        let metadata = BTreeMetadata {
            root_id: self.root_id,
            block_size: self.storage.block_size(),
            max_keys_per_node: self.max_keys_per_node,
            next_key_id: 0,
        };

        let metadata_data = bincode::serialize(&metadata)?;
        let padded_data = BlockSerializer::pad_to_block(metadata_data, self.storage.block_size());

        self.storage.write_block(self.metadata_id, &padded_data)?;
        self.storage.sync()?;
        Ok(())
    }

    fn insert_recursive(
        &mut self,
        node_id: BlockId,
        key: K,
        value: V,
    ) -> Result<Option<(K, BlockId)>, StorageError> {
        let mut node = self.load_node(node_id)?;

        if node.is_leaf() {
            node.insert_leaf(key, value)?;
            self.save_node(node_id, &node)?;
            Ok(None)
        } else {
            let child_idx = node.find_key_index(&key);
            let child_id = node.child_ids[child_idx];

            if let Some((split_key, new_right_id)) = self.insert_recursive(child_id, key, value)? {
                if node.keys.len() >= self.max_keys_per_node {
                    // Split internal node
                    let (promoted_key, right_node) = node.split()?;
                    let new_right_id = self.allocate_node()?;
                    self.save_node(new_right_id, &right_node)?;
                    self.save_node(node_id, &node)?;
                    Ok(Some((promoted_key, new_right_id)))
                } else {
                    node.insert_internal(split_key, child_id, new_right_id)?;
                    self.save_node(node_id, &node)?;
                    Ok(None)
                }
            } else {
                self.save_node(node_id, &node)?;
                Ok(None)
            }
        }
    }

    pub fn insert(&mut self, key: K, value: V) -> Result<Option<V>, StorageError> {
        if self.root_id.is_none() {
            let mut root = UniversalNode::new_leaf();
            root.insert_leaf(key, value)?;

            let root_id = self.allocate_node()?;
            self.save_node(root_id, &root)?;
            self.root_id = Some(root_id);
            self.save_metadata()?;
            return Ok(None);
        }

        let root_id = self.root_id.unwrap();
        let result = self.insert_recursive(root_id, key, value)?;

        if let Some((split_key, new_right_id)) = result {
            let new_root_id = self.allocate_node()?;
            let mut new_root = UniversalNode::new_internal();
            new_root.insert_internal(split_key, root_id, new_right_id)?;
            self.save_node(new_root_id, &new_root)?;

            self.root_id = Some(new_root_id);
            self.save_metadata()?;
        }

        Ok(None)
    }

    pub fn get(&self, key: &K) -> Result<Option<V>, StorageError> {
        let Some(root_id) = self.root_id else {
            return Ok(None);
        };

        self.get_recursive(root_id, key)
    }

    fn get_recursive(&self, node_id: BlockId, key: &K) -> Result<Option<V>, StorageError> {
        let node = self.load_node(node_id)?;

        if node.is_leaf() {
            Ok(node.get(key).cloned())
        } else {
            let child_idx = node.find_key_index(key);
            let child_id = node.child_ids[child_idx];
            self.get_recursive(child_id, key)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::FileBlockStorage;
    use tempfile::TempDir;

    #[test]
    fn test_basic_insert_get() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let mut btree: BTree<String, String, _> = BTree::new(storage).unwrap();

        btree
            .insert("key1".to_string(), "value1".to_string())
            .unwrap();
        if let Err(e) = btree.insert("key2".to_string(), "value2".to_string()) {
            panic!("Insert failed: {:?}", e);
        }

        assert_eq!(
            btree.get(&"key1".to_string()).unwrap(),
            Some("value1".to_string())
        );
        assert_eq!(
            btree.get(&"key2".to_string()).unwrap(),
            Some("value2".to_string())
        );
        assert_eq!(btree.get(&"missing".to_string()).unwrap(), None);
    }
}
