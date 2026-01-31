use crate::btree::error::{BTreeError, DeleteResult};
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
    entry_count: u64,
}

/// Persistent B-tree implementation with pluggable storage
pub struct BTree<K, V, S>
where
    K: Ord + Clone + Serialize + for<'de> Deserialize<'de>,
    V: Clone + Serialize + for<'de> Deserialize<'de>,
    S: BlockStorage<Error = StorageError>,
{
    pub(crate) storage: S,
    pub(crate) metadata_id: BlockId,
    pub(crate) root_id: Option<BlockId>,
    pub(crate) max_keys_per_node: usize,
    pub(crate) entry_count: u64,
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
            entry_count: 0,
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
                    self.entry_count = metadata.entry_count;
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
            entry_count: 0,
        };

        let metadata_data = bincode::serialize(&metadata)?;
        let padded_data = BlockSerializer::pad_to_block(metadata_data, self.storage.block_size());

        self.storage.write_block(self.metadata_id, &padded_data)?;
        self.storage.sync()?;

        self.root_id = None;
        self.entry_count = 0;
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

    pub(crate) fn load_node(&self, node_id: BlockId) -> Result<UniversalNode<K, V>, StorageError> {
        let data = self.storage.read_block(node_id)?;
        let node: UniversalNode<K, V> = bincode::deserialize(&data)?;
        Ok(node)
    }

    fn save_metadata(&mut self) -> Result<(), StorageError> {
        let metadata = BTreeMetadata {
            root_id: self.root_id,
            block_size: self.storage.block_size(),
            max_keys_per_node: self.max_keys_per_node,
            entry_count: self.entry_count,
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
    ) -> Result<(Option<V>, Option<(K, BlockId)>), StorageError> {
        let mut node = self.load_node(node_id)?;

        if node.is_leaf() {
            let old_value = node.get(&key).cloned();
            node.insert_leaf(key, value)?;

            if node.keys.len() > self.max_keys_per_node {
                let (split_key, right_node) = node.split()?;
                let right_id = self.allocate_node()?;
                node.next_leaf = Some(right_id);
                self.save_node(right_id, &right_node)?;
                self.save_node(node_id, &node)?;
                Ok((old_value, Some((split_key, right_id))))
            } else {
                self.save_node(node_id, &node)?;
                Ok((old_value, None))
            }
        } else {
            let child_idx = node.find_key_index(&key);
            let child_id = node.child_ids[child_idx];

            let (old_value, split_result) = self.insert_recursive(child_id, key, value)?;

            if let Some((split_key, new_right_id)) = split_result {
                node.insert_internal(split_key, child_id, new_right_id)?;

                if node.keys.len() > self.max_keys_per_node {
                    let (promoted_key, right_node) = node.split()?;
                    let right_block_id = self.allocate_node()?;
                    self.save_node(right_block_id, &right_node)?;
                    self.save_node(node_id, &node)?;
                    Ok((old_value, Some((promoted_key, right_block_id))))
                } else {
                    self.save_node(node_id, &node)?;
                    Ok((old_value, None))
                }
            } else {
                // Node itself didn't change, but child might have been saved
                Ok((old_value, None))
            }
        }
    }

    pub fn insert(&mut self, key: K, value: V) -> Result<Option<V>, StorageError> {
        self.upsert(key, value)
    }

    pub fn upsert(&mut self, key: K, value: V) -> Result<Option<V>, StorageError> {
        if self.root_id.is_none() {
            let mut root = UniversalNode::new_leaf();
            root.insert_leaf(key, value)?;

            let root_id = self.allocate_node()?;
            self.save_node(root_id, &root)?;
            self.root_id = Some(root_id);
            self.entry_count = 1;
            self.save_metadata()?;
            return Ok(None);
        }

        let root_id = self.root_id.unwrap();
        let (old_value, split_result) = self.insert_recursive(root_id, key, value)?;

        if old_value.is_none() {
            self.entry_count += 1;
        }

        if let Some((split_key, new_right_id)) = split_result {
            let new_root_id = self.allocate_node()?;
            let mut new_root = UniversalNode::new_internal();
            new_root.insert_internal(split_key, root_id, new_right_id)?;
            self.save_node(new_root_id, &new_root)?;

            self.root_id = Some(new_root_id);
        }

        self.save_metadata()?;
        Ok(old_value)
    }

    pub fn update(&mut self, key: K, value: V) -> Result<Option<V>, StorageError> {
        if self.root_id.is_none() {
            return Ok(None);
        }

        // Check if key exists first to avoid unnecessary inserts
        if self.get(&key)?.is_none() {
            return Ok(None);
        }

        let root_id = self.root_id.unwrap();
        let (old_value, split_result) = self.insert_recursive(root_id, key, value)?;

        // If split happened (should be rare on update if types are same size, but possible with generic types)
        if let Some((split_key, new_right_id)) = split_result {
            let new_root_id = self.allocate_node()?;
            let mut new_root = UniversalNode::new_internal();
            new_root.insert_internal(split_key, root_id, new_right_id)?;
            self.save_node(new_root_id, &new_root)?;

            self.root_id = Some(new_root_id);
        }

        self.save_metadata()?;
        Ok(old_value)
    }

    pub fn len(&self) -> u64 {
        self.entry_count
    }

    pub fn is_empty(&self) -> bool {
        self.entry_count == 0
    }

    pub fn clear(&mut self) -> Result<(), StorageError> {
        if let Some(root_id) = self.root_id {
            self.deallocate_recursive(root_id)?;
            self.root_id = None;
            self.entry_count = 0;
            self.save_metadata()?;
        }
        Ok(())
    }

    fn deallocate_recursive(&mut self, node_id: BlockId) -> Result<(), StorageError> {
        let node = self.load_node(node_id)?;
        if !node.is_leaf() {
            for child_id in node.child_ids {
                self.deallocate_recursive(child_id)?;
            }
        }
        self.deallocate_block(node_id)?;
        Ok(())
    }

    pub fn iter(&self) -> Result<crate::btree::iterator::BTreeIter<'_, K, V, S>, StorageError> {
        crate::btree::iterator::BTreeIter::new(self)
    }

    pub fn iter_range<R>(
        &self,
        range: R,
    ) -> Result<crate::btree::iterator::BTreeRangeIter<'_, K, V, S, R>, StorageError>
    where
        R: std::ops::RangeBounds<K>,
    {
        crate::btree::iterator::BTreeRangeIter::new(self, range)
    }

    pub fn get(&self, key: &K) -> Result<Option<V>, StorageError> {
        let Some(root_id) = self.root_id else {
            return Ok(None);
        };

        self.get_recursive(root_id, key)
    }

    /// Range query returning key-value pairs within the specified range
    pub fn range<R>(&self, range: R) -> Result<Vec<(K, V)>, StorageError>
    where
        R: std::ops::RangeBounds<K>,
    {
        let Some(root_id) = self.root_id else {
            return Ok(Vec::new());
        };

        // Find the first leaf that could contain the start of the range
        let mut current_id = root_id;
        loop {
            let node = self.load_node(current_id)?;
            if node.is_leaf() {
                break;
            }
            let start_key = match range.start_bound() {
                std::ops::Bound::Included(k) => Some(k),
                std::ops::Bound::Excluded(k) => Some(k),
                std::ops::Bound::Unbounded => None,
            };

            let idx = if let Some(k) = start_key {
                node.find_key_index(k)
            } else {
                0
            };
            current_id = node.child_ids[idx];
        }

        let mut results = Vec::new();
        let mut leaf_id = Some(current_id);

        while let Some(id) = leaf_id {
            let node = self.load_node(id)?;
            for (i, key) in node.keys.iter().enumerate() {
                if range.contains(key) {
                    results.push((key.clone(), node.values[i].clone()));
                } else if match range.end_bound() {
                    std::ops::Bound::Included(k) => key > k,
                    std::ops::Bound::Excluded(k) => key >= k,
                    std::ops::Bound::Unbounded => false,
                } {
                    // Past the end of the range
                    return Ok(results);
                }
            }
            leaf_id = node.next_leaf;
        }

        Ok(results)
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

    /// Calculate minimum keys per node
    fn min_keys(&self) -> usize {
        self.max_keys_per_node.div_ceil(2) - 1
    }

    /// Check if a node is the root
    fn is_root(&self, node_id: BlockId) -> bool {
        self.root_id == Some(node_id)
    }

    /// Deallocate a block from storage
    fn deallocate_block(&mut self, block_id: BlockId) -> Result<(), StorageError> {
        self.storage.deallocate_block(block_id)
    }

    /// Delete a key from the B+ tree
    pub fn validate(&self) -> Result<(), BTreeError> {
        let Some(root_id) = self.root_id else {
            if self.entry_count != 0 {
                return Err(BTreeError::ValidationFailed(
                    "Root is None but entry_count is not 0".into(),
                ));
            }
            return Ok(());
        };

        let mut leaf_depth = None;
        let actual_count = self.validate_recursive(root_id, 0, &mut leaf_depth, None, None)?;

        if actual_count != self.entry_count {
            return Err(BTreeError::ValidationFailed(format!(
                "entry_count mismatch: expected {}, actual {}",
                self.entry_count, actual_count
            )));
        }

        Ok(())
    }

    fn validate_recursive(
        &self,
        node_id: BlockId,
        depth: usize,
        leaf_depth: &mut Option<usize>,
        min_key: Option<&K>,
        max_key: Option<&K>,
    ) -> Result<u64, BTreeError> {
        let node = self.load_node(node_id).map_err(BTreeError::Storage)?;
        node.validate().map_err(|_| BTreeError::NodeCorrupted)?;

        // Check key range
        for key in &node.keys {
            if let Some(min) = min_key {
                if key < min {
                    return Err(BTreeError::ValidationFailed(
                        "Key violates min bound".into(),
                    ));
                }
            }
            if let Some(max) = max_key {
                if key >= max {
                    return Err(BTreeError::ValidationFailed(
                        "Key violates max bound".into(),
                    ));
                }
            }
        }

        if node.is_leaf() {
            if let Some(d) = *leaf_depth {
                if d != depth {
                    return Err(BTreeError::ValidationFailed(format!(
                        "Leaf depth mismatch: expected {}, got {}",
                        d, depth
                    )));
                }
            } else {
                *leaf_depth = Some(depth);
            }
            Ok(node.key_count as u64)
        } else {
            let mut total_count = 0;
            for i in 0..node.child_ids.len() {
                let child_min = if i == 0 {
                    min_key
                } else {
                    Some(&node.keys[i - 1])
                };
                let child_max = if i == node.keys.len() {
                    max_key
                } else {
                    Some(&node.keys[i])
                };

                total_count += self.validate_recursive(
                    node.child_ids[i],
                    depth + 1,
                    leaf_depth,
                    child_min,
                    child_max,
                )?;
            }
            Ok(total_count)
        }
    }

    pub fn delete(&mut self, key: &K) -> Result<Option<V>, StorageError> {
        let Some(root_id) = self.root_id else {
            return Ok(None);
        };

        match self.delete_recursive(root_id, key, None)? {
            DeleteResult::KeyRemoved(value) => {
                self.entry_count -= 1;
                self.handle_root_underflow()?;
                self.save_metadata()?;
                Ok(Some(value))
            }
            DeleteResult::NoChange => Ok(None),
            DeleteResult::TreeEmpty => {
                self.root_id = None;
                self.entry_count = 0;
                self.save_metadata()?;
                Ok(None)
            }
            DeleteResult::Underflow { value, .. } => {
                if value.is_some() {
                    self.entry_count -= 1;
                }
                // Root should handle underflow specially
                self.handle_root_underflow()?;
                self.save_metadata()?;
                match value {
                    Some(v) => Ok(Some(v)),
                    None => Ok(None),
                }
            }
        }
    }

    /// Safe delete with transaction-like rollback
    /// Note: This only protects root metadata. Individual blocks may still be modified on failure.
    pub fn delete_safe(&mut self, key: &K) -> Result<Option<V>, StorageError> {
        // Save current state for rollback
        let original_root = self.root_id;

        match self.delete(key) {
            Ok(result) => Ok(result),
            Err(e) => {
                // Rollback root reference on error
                self.root_id = original_root;
                self.save_metadata()?;
                Err(e)
            }
        }
    }

    /// Delete multiple keys efficiently
    pub fn delete_batch(&mut self, keys: &[K]) -> Result<Vec<Option<V>>, StorageError> {
        let mut results = Vec::with_capacity(keys.len());
        for key in keys {
            results.push(self.delete(key)?);
        }
        Ok(results)
    }

    /// Recursive deletion with rebalancing
    fn delete_recursive(
        &mut self,
        node_id: BlockId,
        key: &K,
        _parent_id: Option<BlockId>,
    ) -> Result<DeleteResult<K, V>, StorageError> {
        let mut node = self.load_node(node_id)?;

        if node.is_leaf() {
            // Leaf deletion case
            if let Some(removed_value) = node.remove_leaf(key) {
                self.save_node(node_id, &node)?;

                // Check for underflow (except root)
                if !self.is_root(node_id) && (node.key_count as usize) < self.min_keys() {
                    Ok(DeleteResult::Underflow {
                        node_id,
                        value: Some(removed_value),
                        left_sibling: None,
                        right_sibling: None,
                        new_separator: None,
                    })
                } else {
                    Ok(DeleteResult::KeyRemoved(removed_value))
                }
            } else {
                Ok(DeleteResult::NoChange)
            }
        } else {
            // Internal node deletion case
            let child_idx = node.find_key_index(key);
            let child_id = node.child_ids[child_idx];

            let result = self.delete_recursive(child_id, key, Some(node_id))?;

            match result {
                DeleteResult::Underflow { value, .. } => {
                    // Handle child underflow
                    self.handle_child_underflow(node_id, child_idx)?;

                    // Reload node because handle_child_underflow modified it on disk
                    node = self.load_node(node_id)?;

                    // After rebalancing, we need to check if this node now underflows
                    let parent_underflows =
                        !self.is_root(node_id) && (node.key_count as usize) < self.min_keys();

                    if parent_underflows {
                        Ok(DeleteResult::Underflow {
                            node_id,
                            value,
                            left_sibling: None,
                            right_sibling: None,
                            new_separator: None,
                        })
                    } else {
                        match value {
                            Some(v) => Ok(DeleteResult::KeyRemoved(v)),
                            None => Ok(DeleteResult::NoChange),
                        }
                    }
                }
                DeleteResult::KeyRemoved(val) => {
                    // Node might have been updated (separator updates not yet implemented)
                    self.save_node(node_id, &node)?;
                    Ok(DeleteResult::KeyRemoved(val))
                }
                _ => {
                    self.save_node(node_id, &node)?;
                    Ok(result)
                }
            }
        }
    }

    /// Handle underflow in a child node
    fn handle_child_underflow(
        &mut self,
        parent_id: BlockId,
        child_idx: usize,
    ) -> Result<(), StorageError> {
        let mut parent = self.load_node(parent_id)?;
        let child_id = parent.child_ids[child_idx];
        let mut child = self.load_node(child_id)?;

        // Find siblings
        let left_sibling_id = if child_idx > 0 {
            Some(parent.child_ids[child_idx - 1])
        } else {
            None
        };

        let right_sibling_id = if child_idx + 1 < parent.child_ids.len() {
            Some(parent.child_ids[child_idx + 1])
        } else {
            None
        };

        // Try redistribution first
        if let Some(sibling_id) = left_sibling_id {
            let mut sibling = self.load_node(sibling_id)?;
            if sibling.key_count > self.min_keys() as u32 {
                // Borrow from left sibling
                child.borrow_from_left(&mut sibling, &mut parent.keys[child_idx - 1])?;

                self.save_node(sibling_id, &sibling)?;
                self.save_node(child_id, &child)?;
                self.save_node(parent_id, &parent)?;
                return Ok(());
            }
        }

        if let Some(sibling_id) = right_sibling_id {
            let mut sibling = self.load_node(sibling_id)?;
            if sibling.key_count > self.min_keys() as u32 {
                // Borrow from right sibling
                child.borrow_from_right(&mut sibling, &mut parent.keys[child_idx])?;

                self.save_node(sibling_id, &sibling)?;
                self.save_node(child_id, &child)?;
                self.save_node(parent_id, &parent)?;
                return Ok(());
            }
        }

        // If redistribution fails, merge with a sibling
        if let Some(sibling_id) = left_sibling_id {
            self.merge_nodes(parent_id, sibling_id, child_id, child_idx - 1)?;
        } else if let Some(sibling_id) = right_sibling_id {
            self.merge_nodes(parent_id, child_id, sibling_id, child_idx)?;
        }

        Ok(())
    }

    /// Merge two nodes and update parent
    fn merge_nodes(
        &mut self,
        parent_id: BlockId,
        left_id: BlockId,
        right_id: BlockId,
        separator_idx: usize,
    ) -> Result<(), StorageError> {
        let mut parent = self.load_node(parent_id)?;
        let mut left_node = self.load_node(left_id)?;
        let right_node = self.load_node(right_id)?;

        // Get separator key
        let separator_key = parent.keys.remove(separator_idx);
        parent.child_ids.remove(separator_idx + 1);
        parent.key_count -= 1;

        // Merge nodes
        left_node.merge(
            &right_node,
            if left_node.is_internal() {
                Some(separator_key)
            } else {
                None
            },
        )?;

        // Save changes
        self.save_node(left_id, &left_node)?;
        self.deallocate_block(right_id)?; // Free right node
        self.save_node(parent_id, &parent)?;

        Ok(())
    }

    /// Handle special case when root underflows
    fn handle_root_underflow(&mut self) -> Result<(), StorageError> {
        let Some(root_id) = self.root_id else {
            return Ok(());
        };

        let root = self.load_node(root_id)?;

        // If root is empty and has children, replace root with its only child
        if root.key_count == 0 && !root.child_ids.is_empty() {
            let new_root_id = root.child_ids[0];
            self.root_id = Some(new_root_id);
            self.deallocate_block(root_id)?; // Free old root
            self.save_metadata()?;
        } else if root.key_count == 0 && root.is_leaf() {
            // Tree is completely empty
            self.root_id = None;
            self.deallocate_block(root_id)?;
            self.save_metadata()?;
        }

        Ok(())
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

    #[test]
    fn test_basic_delete() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let mut btree: BTree<String, String, _> = BTree::new(storage).unwrap();

        btree
            .insert("key1".to_string(), "value1".to_string())
            .unwrap();
        btree
            .insert("key2".to_string(), "value2".to_string())
            .unwrap();

        assert_eq!(
            btree.delete(&"key1".to_string()).unwrap(),
            Some("value1".to_string())
        );
        assert_eq!(btree.get(&"key1".to_string()).unwrap(), None);
        assert_eq!(
            btree.get(&"key2".to_string()).unwrap(),
            Some("value2".to_string())
        );

        assert_eq!(
            btree.delete(&"key2".to_string()).unwrap(),
            Some("value2".to_string())
        );
        assert_eq!(btree.get(&"key2".to_string()).unwrap(), None);
        assert!(btree.root_id.is_none());
    }

    #[test]
    fn test_delete_with_redistribution() {
        let temp_dir = TempDir::new().unwrap();
        // Use small block size to force splits/redistribution
        let storage = FileBlockStorage::new(temp_dir.path(), 512).unwrap();
        let mut btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();

        // Insert enough keys to cause a split
        for i in 0..20 {
            btree.insert(i, i * 10).unwrap();
        }

        // Delete keys to trigger redistribution
        for i in 0..10 {
            btree.delete(&i).unwrap();
        }

        for i in 10..20 {
            assert_eq!(btree.get(&i).unwrap(), Some(i * 10));
        }
    }

    #[test]
    fn test_delete_with_merge() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 512).unwrap();
        let mut btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();

        for i in 0..20 {
            btree.insert(i, i * 10).unwrap();
        }

        // Delete most keys to trigger merges
        for i in 0..18 {
            btree.delete(&i).unwrap();
        }

        assert_eq!(btree.get(&18).unwrap(), Some(180));
        assert_eq!(btree.get(&19).unwrap(), Some(190));

        let root = btree.load_node(btree.root_id.unwrap()).unwrap();
        assert!(root.is_leaf());
    }

    #[test]
    fn test_iter_empty_tree() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();

        assert_eq!(btree.iter().unwrap().count(), 0);
    }

    #[test]
    fn test_iter_range_empty() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let mut btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();

        btree.insert(10, 100).unwrap();
        assert_eq!(btree.iter_range(0..5).unwrap().count(), 0);
        assert_eq!(btree.iter_range(15..20).unwrap().count(), 0);
    }

    #[test]
    fn test_validate_corruption() {
        let temp_dir = TempDir::new().unwrap();
        let mut storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();

        {
            let mut btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();
            btree.insert(1, 10).unwrap();
            storage = btree.storage; // Get storage back
        }

        // Corrupt block 1 (root) by writing zeros
        storage.write_block(1, &vec![0u8; 4096]).unwrap();

        let btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();
        // Validation should fail now
        let res = btree.validate();
        assert!(
            res.is_err(),
            "Validation should fail but returned {:?}",
            res
        );
    }

    #[test]
    fn test_validate_empty_tree() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();
        btree.validate().unwrap();
    }

    #[test]
    fn test_validate() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 512).unwrap();
        let mut btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();

        btree.validate().unwrap();

        for i in 0..100 {
            btree.insert(i, i * 10).unwrap();
            btree.validate().unwrap();
        }

        for i in 0..100 {
            let res = btree.delete(&i).unwrap();
            assert!(res.is_some(), "Key {} should exist", i);
            btree.validate().unwrap();
        }
    }

    #[test]
    fn test_iter_basic() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let mut btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();

        for i in 0..100 {
            btree.insert(i, i * 10).unwrap();
        }

        let mut count = 0;
        for (idx, result) in btree.iter().unwrap().enumerate() {
            let (key, value) = result.unwrap();
            assert_eq!(key, idx as u32);
            assert_eq!(value, (idx * 10) as u32);
            count += 1;
        }
        assert_eq!(count, 100);
    }

    #[test]
    fn test_iter_range() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 512).unwrap();
        let mut btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();

        for i in 0..50 {
            btree.insert(i, i * 10).unwrap();
        }

        let range_vec: Vec<_> = btree
            .iter_range(10..20)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(range_vec.len(), 10);
        assert_eq!(range_vec[0].0, 10);
        assert_eq!(range_vec[9].0, 19);

        let full_range: Vec<_> = btree
            .iter_range(..)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(full_range.len(), 50);
    }

    #[test]
    fn test_len_and_is_empty() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let mut btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();

        assert!(btree.is_empty());
        assert_eq!(btree.len(), 0);

        btree.insert(1, 10).unwrap();
        assert!(!btree.is_empty());
        assert_eq!(btree.len(), 1);

        btree.insert(2, 20).unwrap();
        assert_eq!(btree.len(), 2);

        btree.delete(&1).unwrap();
        assert_eq!(btree.len(), 1);

        btree.delete(&2).unwrap();
        assert!(btree.is_empty());
        assert_eq!(btree.len(), 0);
    }

    #[test]
    fn test_update_and_upsert() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let mut btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();

        assert_eq!(btree.update(1, 10).unwrap(), None);
        assert_eq!(btree.len(), 0);

        assert_eq!(btree.upsert(1, 10).unwrap(), None);
        assert_eq!(btree.len(), 1);

        assert_eq!(btree.update(1, 11).unwrap(), Some(10));
        assert_eq!(btree.get(&1).unwrap(), Some(11));
        assert_eq!(btree.len(), 1);

        assert_eq!(btree.upsert(1, 12).unwrap(), Some(11));
        assert_eq!(btree.get(&1).unwrap(), Some(12));
        assert_eq!(btree.len(), 1);
    }

    #[test]
    fn test_clear() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let mut btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();

        for i in 0..10 {
            btree.insert(i, i * 10).unwrap();
        }
        assert_eq!(btree.len(), 10);

        btree.clear().unwrap();
        assert!(btree.is_empty());
        assert_eq!(btree.len(), 0);
        assert!(btree.root_id.is_none());
    }

    #[test]
    fn test_range_query() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let mut btree: BTree<u32, String, _> = BTree::new(storage).unwrap();

        for i in 0..10 {
            btree.insert(i, format!("val{}", i)).unwrap();
        }

        let range = btree.range(3..7).unwrap();
        assert_eq!(range.len(), 4);
        assert_eq!(range[0].0, 3);
        assert_eq!(range[3].0, 6);

        let full_range = btree.range(..).unwrap();
        assert_eq!(full_range.len(), 10);
    }
}
