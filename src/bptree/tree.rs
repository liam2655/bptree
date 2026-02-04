use crate::bptree::error::{BPTreeError, DeleteResult};
use crate::bptree::node::UniversalNode;
use crate::ser::BlockSerializer;
use crate::storage::{BlockId, BlockStorage, StorageError};
use async_recursion::async_recursion;
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;

/// B-tree metadata version
const METADATA_V1: u8 = 1;

/// B-tree metadata stored at the beginning of the root block (block 1)
#[derive(Debug, Serialize, Deserialize)]
struct BPTreeMetadata {
    version: u8,
    block_size: usize,
    max_keys_per_node: usize,
    entry_count: u64,
    root_exists: bool,
}

/// Persistent B-tree implementation with pluggable storage
#[derive(Debug)]
pub struct BPTree<K, V, S>
where
    K: Ord + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync,
    V: Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync,
    S: BlockStorage<Error = StorageError>,
{
    pub(crate) storage: S,
    pub(crate) root_exists: bool,
    pub(crate) max_keys_per_node: usize,
    pub(crate) entry_count: u64,
    _phantom: PhantomData<(K, V)>,
}

impl<K, V, S> BPTree<K, V, S>
where
    K: Ord + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync,
    V: Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync,
    S: BlockStorage<Error = StorageError>,
{
    pub const ROOT_ID: BlockId = 1;

    pub async fn new(storage: S) -> Result<Self, StorageError> {
        let block_size = storage.block_size();
        let max_keys_per_node = Self::estimate_max_keys(block_size);

        let mut bptree = Self {
            storage,
            root_exists: false,
            max_keys_per_node,
            entry_count: 0,
            _phantom: PhantomData,
        };

        bptree.load_or_init_metadata().await?;
        Ok(bptree)
    }

    fn estimate_max_keys(block_size: usize) -> usize {
        // Account for metadata overhead in the root block
        let metadata_size = std::mem::size_of::<BPTreeMetadata>() + 32;
        let overhead = 100 + metadata_size;
        let pair_size = std::mem::size_of::<K>() + std::mem::size_of::<V>() + 20;
        ((block_size - overhead) / pair_size).max(2)
    }

    async fn load_or_init_metadata(&mut self) -> Result<(), StorageError> {
        match self.storage.read_block(Self::ROOT_ID).await {
            Ok(data) => {
                match bincode::deserialize::<BPTreeMetadata>(&data) {
                    Ok(metadata) => {
                        if metadata.version != METADATA_V1 {
                            return Err(StorageError::VersionMismatch {
                                expected: METADATA_V1,
                                actual: metadata.version,
                            });
                        }
                        if metadata.block_size != self.storage.block_size() {
                            return Err(StorageError::MetadataMismatch(format!(
                                "Block size mismatch: metadata says {}, storage says {}",
                                metadata.block_size,
                                self.storage.block_size()
                            )));
                        }
                        self.root_exists = metadata.root_exists;
                        self.max_keys_per_node = metadata.max_keys_per_node;
                        self.entry_count = metadata.entry_count;
                        Ok(())
                    }
                    Err(_) => {
                        // If block is all zeros, it might be an uninitialized block 1
                        if data.iter().all(|&b| b == 0) {
                            self.initialize_metadata().await
                        } else {
                            Err(StorageError::BlockCorrupted(Self::ROOT_ID))
                        }
                    }
                }
            }
            Err(StorageError::BlockNotFound(_)) => {
                // Block doesn't exist, initialize new metadata in block 1
                self.initialize_metadata().await
            }
            Err(e) => Err(e),
        }
    }

    async fn initialize_metadata(&mut self) -> Result<(), StorageError> {
        self.root_exists = false;
        self.entry_count = 0;

        let metadata = BPTreeMetadata {
            version: METADATA_V1,
            block_size: self.storage.block_size(),
            max_keys_per_node: self.max_keys_per_node,
            entry_count: 0,
            root_exists: false,
        };

        let metadata_data = bincode::serialize(&metadata)?;
        let padded_data = BlockSerializer::pad_to_block(metadata_data, self.storage.block_size());

        self.storage
            .write_block(Self::ROOT_ID, &padded_data)
            .await?;
        self.storage.sync().await?;

        Ok(())
    }

    async fn allocate_node(&mut self) -> Result<BlockId, StorageError> {
        self.storage.allocate_block().await
    }

    async fn save_node(
        &mut self,
        node_id: BlockId,
        node: &UniversalNode<K, V>,
    ) -> Result<(), StorageError> {
        let data = if node_id == Self::ROOT_ID {
            let metadata = BPTreeMetadata {
                version: METADATA_V1,
                block_size: self.storage.block_size(),
                max_keys_per_node: self.max_keys_per_node,
                entry_count: self.entry_count,
                root_exists: self.root_exists,
            };
            let mut buf = bincode::serialize(&metadata)?;
            let node_buf = bincode::serialize(node)?;
            buf.extend(node_buf);
            buf
        } else {
            bincode::serialize(node)?
        };

        let padded_data = BlockSerializer::pad_to_block(data, self.storage.block_size());
        self.storage.write_block(node_id, &padded_data).await?;
        self.storage.sync().await?;
        Ok(())
    }

    pub(crate) async fn load_node(
        &self,
        node_id: BlockId,
    ) -> Result<UniversalNode<K, V>, StorageError> {
        let data = self.storage.read_block(node_id).await?;
        if node_id == Self::ROOT_ID {
            let mut cursor = std::io::Cursor::new(&data);
            let metadata: BPTreeMetadata = bincode::deserialize_from(&mut cursor)?;

            // Verify metadata
            if metadata.version != METADATA_V1 {
                return Err(StorageError::VersionMismatch {
                    expected: METADATA_V1,
                    actual: metadata.version,
                });
            }
            if metadata.block_size != self.storage.block_size() {
                return Err(StorageError::MetadataMismatch(format!(
                    "Block size mismatch in root: metadata says {}, storage says {}",
                    metadata.block_size,
                    self.storage.block_size()
                )));
            }

            let node: UniversalNode<K, V> = bincode::deserialize_from(&mut cursor)?;
            Ok(node)
        } else {
            let node: UniversalNode<K, V> = bincode::deserialize(&data)?;
            Ok(node)
        }
    }

    async fn save_metadata(&mut self) -> Result<(), StorageError> {
        if self.root_exists {
            let root = self.load_node(Self::ROOT_ID).await?;
            self.save_node(Self::ROOT_ID, &root).await?;
        } else {
            let metadata = BPTreeMetadata {
                version: METADATA_V1,
                block_size: self.storage.block_size(),
                max_keys_per_node: self.max_keys_per_node,
                entry_count: self.entry_count,
                root_exists: self.root_exists,
            };
            let metadata_data = bincode::serialize(&metadata)?;
            let padded_data =
                BlockSerializer::pad_to_block(metadata_data, self.storage.block_size());
            self.storage
                .write_block(Self::ROOT_ID, &padded_data)
                .await?;
            self.storage.sync().await?;
        }
        Ok(())
    }

    #[async_recursion]
    async fn insert_recursive(
        &mut self,
        node_id: BlockId,
        key: K,
        value: V,
    ) -> Result<(Option<V>, Option<(K, BlockId)>), StorageError> {
        let mut node = self.load_node(node_id).await?;

        if node.is_leaf() {
            let old_value = node.get(&key).cloned();
            node.insert_leaf(key, value)?;

            if node.keys.len() > self.max_keys_per_node {
                let (split_key, right_node) = node.split()?;
                let right_id = self.allocate_node().await?;
                node.next_leaf = Some(right_id);
                self.save_node(right_id, &right_node).await?;
                self.save_node(node_id, &node).await?;
                Ok((old_value, Some((split_key, right_id))))
            } else {
                self.save_node(node_id, &node).await?;
                Ok((old_value, None))
            }
        } else {
            let child_idx = node.find_key_index(&key);
            let child_id = node.child_ids[child_idx];

            let (old_value, split_result) = self.insert_recursive(child_id, key, value).await?;

            if let Some((split_key, new_right_id)) = split_result {
                node.insert_internal(split_key, child_id, new_right_id)?;

                if node.keys.len() > self.max_keys_per_node {
                    let (promoted_key, right_node) = node.split()?;
                    let right_block_id = self.allocate_node().await?;
                    self.save_node(right_block_id, &right_node).await?;
                    self.save_node(node_id, &node).await?;
                    Ok((old_value, Some((promoted_key, right_block_id))))
                } else {
                    self.save_node(node_id, &node).await?;
                    Ok((old_value, None))
                }
            } else {
                // Node itself didn't change, but child might have been saved
                Ok((old_value, None))
            }
        }
    }

    pub async fn insert(&mut self, key: K, value: V) -> Result<Option<V>, StorageError> {
        self.upsert(key, value).await
    }

    pub async fn upsert(&mut self, key: K, value: V) -> Result<Option<V>, StorageError> {
        if !self.root_exists {
            let mut root = UniversalNode::new_leaf();
            root.insert_leaf(key, value)?;

            self.root_exists = true;
            self.entry_count = 1;
            self.save_node(Self::ROOT_ID, &root).await?;
            return Ok(None);
        }

        let (old_value, split_result) = self.insert_recursive(Self::ROOT_ID, key, value).await?;

        if old_value.is_none() {
            self.entry_count += 1;
        }

        if let Some((split_key, new_right_id)) = split_result {
            // Root split happened. Block 1 now contains the left node.
            // We need to move it to a new block and make Block 1 the new internal root.
            let new_left_id = self.allocate_node().await?;
            let left_node = self.load_node(Self::ROOT_ID).await?;
            self.save_node(new_left_id, &left_node).await?;

            let mut new_root = UniversalNode::new_internal();
            new_root.insert_internal(split_key, new_left_id, new_right_id)?;
            self.save_node(Self::ROOT_ID, &new_root).await?;
        } else {
            // Even if no split, we might need to save metadata (entry_count)
            self.save_metadata().await?;
        }

        Ok(old_value)
    }

    pub async fn update(&mut self, key: K, value: V) -> Result<Option<V>, StorageError> {
        if !self.root_exists {
            return Ok(None);
        }

        // Check if key exists first to avoid unnecessary inserts
        if self.get(&key).await?.is_none() {
            return Ok(None);
        }

        let (old_value, split_result) = self.insert_recursive(Self::ROOT_ID, key, value).await?;

        // If split happened (should be rare on update if types are same size)
        if let Some((split_key, new_right_id)) = split_result {
            let new_left_id = self.allocate_node().await?;
            let left_node = self.load_node(Self::ROOT_ID).await?;
            self.save_node(new_left_id, &left_node).await?;

            let mut new_root = UniversalNode::new_internal();
            new_root.insert_internal(split_key, new_left_id, new_right_id)?;
            self.save_node(Self::ROOT_ID, &new_root).await?;
        } else {
            self.save_metadata().await?;
        }

        Ok(old_value)
    }

    pub fn len(&self) -> u64 {
        self.entry_count
    }

    pub fn is_empty(&self) -> bool {
        self.entry_count == 0
    }

    /// Get all block IDs used by the B+ tree
    pub async fn get_all_block_ids(&self) -> Result<Vec<BlockId>, StorageError> {
        let mut ids = Vec::new();
        ids.push(Self::ROOT_ID);
        if self.root_exists {
            self.collect_blocks_recursive(Self::ROOT_ID, &mut ids)
                .await?;
        }
        Ok(ids)
    }

    #[async_recursion]
    async fn collect_blocks_recursive(
        &self,
        node_id: BlockId,
        ids: &mut Vec<BlockId>,
    ) -> Result<(), StorageError> {
        let node = self.load_node(node_id).await?;
        if !node.is_leaf() {
            for child_id in node.child_ids {
                ids.push(child_id);
                self.collect_blocks_recursive(child_id, ids).await?;
            }
        }
        Ok(())
    }

    pub async fn clear(&mut self) -> Result<(), StorageError> {
        if self.root_exists {
            self.deallocate_recursive(Self::ROOT_ID).await?;
            self.root_exists = false;
            self.entry_count = 0;
            self.save_metadata().await?;
        }
        Ok(())
    }

    #[async_recursion]
    async fn deallocate_recursive(&mut self, node_id: BlockId) -> Result<(), StorageError> {
        let node = self.load_node(node_id).await?;
        if !node.is_leaf() {
            for child_id in node.child_ids {
                self.deallocate_recursive(child_id).await?;
            }
        }
        if node_id != Self::ROOT_ID {
            self.deallocate_block(node_id).await?;
        }
        Ok(())
    }

    pub async fn get(&self, key: &K) -> Result<Option<V>, StorageError> {
        if !self.root_exists {
            return Ok(None);
        }

        self.get_recursive(Self::ROOT_ID, key).await
    }

    /// Range query returning key-value pairs within the specified range
    pub async fn range<R>(&self, range: R) -> Result<Vec<(K, V)>, StorageError>
    where
        R: std::ops::RangeBounds<K>,
    {
        if !self.root_exists {
            return Ok(Vec::new());
        };

        // Find the first leaf that could contain the start of the range
        let mut current_id = Self::ROOT_ID;
        loop {
            let node = self.load_node(current_id).await?;
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
            let node = self.load_node(id).await?;
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

    #[async_recursion]
    async fn get_recursive(&self, node_id: BlockId, key: &K) -> Result<Option<V>, StorageError> {
        let node = self.load_node(node_id).await?;

        if node.is_leaf() {
            Ok(node.get(key).cloned())
        } else {
            let child_idx = node.find_key_index(key);
            let child_id = node.child_ids[child_idx];
            self.get_recursive(child_id, key).await
        }
    }

    /// Calculate minimum keys per node
    fn min_keys(&self) -> usize {
        self.max_keys_per_node.div_ceil(2) - 1
    }

    /// Check if a node is the root
    fn is_root(&self, node_id: BlockId) -> bool {
        node_id == Self::ROOT_ID
    }

    /// Deallocate a block from storage
    async fn deallocate_block(&mut self, block_id: BlockId) -> Result<(), StorageError> {
        self.storage.deallocate_block(block_id).await
    }

    /// Delete a key from the B+ tree
    pub async fn validate(&self) -> Result<(), BPTreeError> {
        if !self.root_exists {
            if self.entry_count != 0 {
                return Err(BPTreeError::ValidationFailed(
                    "Root does not exist but entry_count is not 0".into(),
                ));
            }
            return Ok(());
        }

        let mut leaf_depth = None;
        let actual_count = self
            .validate_recursive(Self::ROOT_ID, 0, &mut leaf_depth, None, None)
            .await?;

        if actual_count != self.entry_count {
            return Err(BPTreeError::ValidationFailed(format!(
                "entry_count mismatch: expected {}, actual {}",
                self.entry_count, actual_count
            )));
        }

        Ok(())
    }

    #[async_recursion]
    async fn validate_recursive(
        &self,
        node_id: BlockId,
        depth: usize,
        leaf_depth: &mut Option<usize>,
        min_key: Option<&K>,
        max_key: Option<&K>,
    ) -> Result<u64, BPTreeError> {
        let node = self
            .load_node(node_id)
            .await
            .map_err(BPTreeError::Storage)?;
        node.validate().map_err(|_| BPTreeError::NodeCorrupted)?;

        // Check key range
        for key in &node.keys {
            if let Some(min) = min_key {
                if key < min {
                    return Err(BPTreeError::ValidationFailed(
                        "Key violates min bound".into(),
                    ));
                }
            }
            if let Some(max) = max_key {
                if key >= max {
                    return Err(BPTreeError::ValidationFailed(
                        "Key violates max bound".into(),
                    ));
                }
            }
        }

        if node.is_leaf() {
            if let Some(d) = *leaf_depth {
                if d != depth {
                    return Err(BPTreeError::ValidationFailed(format!(
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

                total_count += self
                    .validate_recursive(
                        node.child_ids[i],
                        depth + 1,
                        leaf_depth,
                        child_min,
                        child_max,
                    )
                    .await?;
            }
            Ok(total_count)
        }
    }

    pub async fn delete(&mut self, key: &K) -> Result<Option<V>, StorageError> {
        if !self.root_exists {
            return Ok(None);
        }

        match self.delete_recursive(Self::ROOT_ID, key, None).await? {
            DeleteResult::KeyRemoved(value) => {
                self.entry_count -= 1;
                self.handle_root_underflow().await?;
                self.save_metadata().await?;
                Ok(Some(value))
            }
            DeleteResult::NoChange => Ok(None),
            DeleteResult::TreeEmpty => {
                self.root_exists = false;
                self.entry_count = 0;
                self.save_metadata().await?;
                Ok(None)
            }
            DeleteResult::Underflow { value, .. } => {
                if value.is_some() {
                    self.entry_count -= 1;
                }
                // Root should handle underflow specially
                self.handle_root_underflow().await?;
                self.save_metadata().await?;
                match value {
                    Some(v) => Ok(Some(v)),
                    None => Ok(None),
                }
            }
        }
    }

    /// Safe delete with transaction-like rollback
    /// Note: This only protects root metadata. Individual blocks may still be modified on failure.
    pub async fn delete_safe(&mut self, key: &K) -> Result<Option<V>, StorageError> {
        // Save current state for rollback
        let original_root_exists = self.root_exists;
        let original_entry_count = self.entry_count;

        match self.delete(key).await {
            Ok(result) => Ok(result),
            Err(e) => {
                // Rollback root reference on error
                self.root_exists = original_root_exists;
                self.entry_count = original_entry_count;
                self.save_metadata().await?;
                Err(e)
            }
        }
    }

    /// Delete multiple keys efficiently
    pub async fn delete_batch(&mut self, keys: &[K]) -> Result<Vec<Option<V>>, StorageError> {
        let mut results = Vec::with_capacity(keys.len());
        for key in keys {
            results.push(self.delete(key).await?);
        }
        Ok(results)
    }

    /// Recursive deletion with rebalancing
    #[async_recursion]
    async fn delete_recursive(
        &mut self,
        node_id: BlockId,
        key: &K,
        _parent_id: Option<BlockId>,
    ) -> Result<DeleteResult<K, V>, StorageError> {
        let mut node = self.load_node(node_id).await?;

        if node.is_leaf() {
            // Leaf deletion case
            if let Some(removed_value) = node.remove_leaf(key) {
                self.save_node(node_id, &node).await?;

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

            let result = self.delete_recursive(child_id, key, Some(node_id)).await?;

            match result {
                DeleteResult::Underflow { value, .. } => {
                    // Handle child underflow
                    self.handle_child_underflow(node_id, child_idx).await?;

                    // Reload node because handle_child_underflow modified it on disk
                    node = self.load_node(node_id).await?;

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
                    self.save_node(node_id, &node).await?;
                    Ok(DeleteResult::KeyRemoved(val))
                }
                _ => {
                    self.save_node(node_id, &node).await?;
                    Ok(result)
                }
            }
        }
    }

    /// Handle underflow in a child node
    async fn handle_child_underflow(
        &mut self,
        parent_id: BlockId,
        child_idx: usize,
    ) -> Result<(), StorageError> {
        let mut parent = self.load_node(parent_id).await?;
        let child_id = parent.child_ids[child_idx];
        let mut child = self.load_node(child_id).await?;

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
            let mut sibling = self.load_node(sibling_id).await?;
            if sibling.key_count > self.min_keys() as u32 {
                // Borrow from left sibling
                child.borrow_from_left(&mut sibling, &mut parent.keys[child_idx - 1])?;

                self.save_node(sibling_id, &sibling).await?;
                self.save_node(child_id, &child).await?;
                self.save_node(parent_id, &parent).await?;
                return Ok(());
            }
        }

        if let Some(sibling_id) = right_sibling_id {
            let mut sibling = self.load_node(sibling_id).await?;
            if sibling.key_count > self.min_keys() as u32 {
                // Borrow from right sibling
                child.borrow_from_right(&mut sibling, &mut parent.keys[child_idx])?;

                self.save_node(sibling_id, &sibling).await?;
                self.save_node(child_id, &child).await?;
                self.save_node(parent_id, &parent).await?;
                return Ok(());
            }
        }

        // If redistribution fails, merge with a sibling
        if let Some(sibling_id) = left_sibling_id {
            self.merge_nodes(parent_id, sibling_id, child_id, child_idx - 1)
                .await?;
        } else if let Some(sibling_id) = right_sibling_id {
            self.merge_nodes(parent_id, child_id, sibling_id, child_idx)
                .await?;
        }

        Ok(())
    }

    /// Merge two nodes and update parent
    async fn merge_nodes(
        &mut self,
        parent_id: BlockId,
        left_id: BlockId,
        right_id: BlockId,
        separator_idx: usize,
    ) -> Result<(), StorageError> {
        let mut parent = self.load_node(parent_id).await?;
        let mut left_node = self.load_node(left_id).await?;
        let right_node = self.load_node(right_id).await?;

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
        self.save_node(left_id, &left_node).await?;
        self.deallocate_block(right_id).await?; // Free right node
        self.save_node(parent_id, &parent).await?;

        Ok(())
    }

    /// Handle special case when root underflows
    async fn handle_root_underflow(&mut self) -> Result<(), StorageError> {
        if !self.root_exists {
            return Ok(());
        }

        let root = self.load_node(Self::ROOT_ID).await?;

        // If root is empty and has children, replace root with its only child
        if root.key_count == 0 && !root.child_ids.is_empty() {
            let only_child_id = root.child_ids[0];
            let only_child_node = self.load_node(only_child_id).await?;
            // Move child to root
            self.save_node(Self::ROOT_ID, &only_child_node).await?;
            self.deallocate_block(only_child_id).await?;
        } else if root.key_count == 0 && root.is_leaf() {
            // Tree is completely empty
            self.root_exists = false;
            self.entry_count = 0;
            self.save_metadata().await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::FileBlockStorage;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_basic_insert_get() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let mut bptree: BPTree<String, String, _> = BPTree::new(storage).await.unwrap();

        bptree
            .insert("key1".to_string(), "value1".to_string())
            .await
            .unwrap();
        if let Err(e) = bptree
            .insert("key2".to_string(), "value2".to_string())
            .await
        {
            panic!("Insert failed: {:?}", e);
        }

        assert_eq!(
            bptree.get(&"key1".to_string()).await.unwrap(),
            Some("value1".to_string())
        );
        assert_eq!(
            bptree.get(&"key2".to_string()).await.unwrap(),
            Some("value2".to_string())
        );
        assert_eq!(bptree.get(&"missing".to_string()).await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_basic_delete() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let mut bptree: BPTree<String, String, _> = BPTree::new(storage).await.unwrap();

        bptree
            .insert("key1".to_string(), "value1".to_string())
            .await
            .unwrap();
        bptree
            .insert("key2".to_string(), "value2".to_string())
            .await
            .unwrap();

        assert_eq!(
            bptree.delete(&"key1".to_string()).await.unwrap(),
            Some("value1".to_string())
        );
        assert_eq!(bptree.get(&"key1".to_string()).await.unwrap(), None);
        assert_eq!(
            bptree.get(&"key2".to_string()).await.unwrap(),
            Some("value2".to_string())
        );

        assert_eq!(
            bptree.delete(&"key2".to_string()).await.unwrap(),
            Some("value2".to_string())
        );
        assert_eq!(bptree.get(&"key2".to_string()).await.unwrap(), None);
        assert!(!bptree.root_exists);
    }

    #[tokio::test]
    async fn test_delete_with_redistribution() {
        let temp_dir = TempDir::new().unwrap();
        // Use small block size to force splits/redistribution
        let storage = FileBlockStorage::new(temp_dir.path(), 512).unwrap();
        let mut bptree: BPTree<u32, u32, _> = BPTree::new(storage).await.unwrap();

        // Insert enough keys to cause a split
        for i in 0..20 {
            bptree.insert(i, i * 10).await.unwrap();
        }

        // Delete keys to trigger redistribution
        for i in 0..10 {
            bptree.delete(&i).await.unwrap();
        }

        for i in 10..20 {
            assert_eq!(bptree.get(&i).await.unwrap(), Some(i * 10));
        }
    }

    #[tokio::test]
    async fn test_delete_with_merge() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 512).unwrap();
        let mut bptree: BPTree<u32, u32, _> = BPTree::new(storage).await.unwrap();

        for i in 0..20 {
            bptree.insert(i, i * 10).await.unwrap();
        }

        // Delete most keys to trigger merges
        for i in 0..18 {
            bptree.delete(&i).await.unwrap();
        }

        assert_eq!(bptree.get(&19).await.unwrap(), Some(190));

        let root = bptree
            .load_node(BPTree::<u32, u32, FileBlockStorage>::ROOT_ID)
            .await
            .unwrap();
        assert!(root.is_leaf());
    }

    #[tokio::test]
    async fn test_iter_empty_tree() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let bptree: BPTree<u32, u32, _> = BPTree::new(storage).await.unwrap();

        assert_eq!(bptree.range(..).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_iter_range_empty() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let mut bptree: BPTree<u32, u32, _> = BPTree::new(storage).await.unwrap();

        bptree.insert(10, 100).await.unwrap();
        assert_eq!(bptree.range(0..5).await.unwrap().len(), 0);
        assert_eq!(bptree.range(15..20).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_validate_corruption() {
        let temp_dir = TempDir::new().unwrap();
        let mut storage = FileBlockStorage::new(temp_dir.path(), 512).unwrap();

        {
            let mut bptree: BPTree<u32, u32, _> = BPTree::new(storage).await.unwrap();
            // Insert enough to cause a split, so we have more blocks than just block 1
            for i in 0..50 {
                bptree.insert(i, i * 10).await.unwrap();
            }
            storage = bptree.storage;
        }

        // Corrupt block 2 (one of the children) by writing zeros
        storage.write_block(2, &vec![0u8; 512]).await.unwrap();

        let bptree: BPTree<u32, u32, _> = BPTree::new(storage).await.unwrap();
        // Validation should fail now because it will try to load block 2
        let res = bptree.validate().await;
        assert!(
            res.is_err(),
            "Validation should fail but returned {:?}",
            res
        );
    }

    #[tokio::test]
    async fn test_validate_empty_tree() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let bptree: BPTree<u32, u32, _> = BPTree::new(storage).await.unwrap();
        bptree.validate().await.unwrap();
    }

    #[tokio::test]
    async fn test_validate() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 512).unwrap();
        let mut bptree: BPTree<u32, u32, _> = BPTree::new(storage).await.unwrap();

        bptree.validate().await.unwrap();

        for i in 0..100 {
            bptree.insert(i, i * 10).await.unwrap();
            bptree.validate().await.unwrap();
        }

        for i in 0..100 {
            let res = bptree.delete(&i).await.unwrap();
            assert!(res.is_some(), "Key {} should exist", i);
            bptree.validate().await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_iter_basic() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let mut bptree: BPTree<u32, u32, _> = BPTree::new(storage).await.unwrap();

        for i in 0..100 {
            bptree.insert(i, i * 10).await.unwrap();
        }

        let results = bptree.range(..).await.unwrap();
        assert_eq!(results.len(), 100);
        for (idx, (key, value)) in results.into_iter().enumerate() {
            assert_eq!(key, idx as u32);
            assert_eq!(value, (idx * 10) as u32);
        }
    }

    #[tokio::test]
    async fn test_iter_range() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 512).unwrap();
        let mut bptree: BPTree<u32, u32, _> = BPTree::new(storage).await.unwrap();

        for i in 0..50 {
            bptree.insert(i, i * 10).await.unwrap();
        }

        let range_vec = bptree.range(10..20).await.unwrap();
        assert_eq!(range_vec.len(), 10);
        assert_eq!(range_vec[0].0, 10);
        assert_eq!(range_vec[9].0, 19);

        let full_range = bptree.range(..).await.unwrap();
        assert_eq!(full_range.len(), 50);
    }

    #[tokio::test]
    async fn test_len_and_is_empty() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let mut bptree: BPTree<u32, u32, _> = BPTree::new(storage).await.unwrap();

        assert!(bptree.is_empty());
        assert_eq!(bptree.len(), 0);

        bptree.insert(1, 10).await.unwrap();
        assert!(!bptree.is_empty());
        assert_eq!(bptree.len(), 1);

        bptree.insert(2, 20).await.unwrap();
        assert_eq!(bptree.len(), 2);

        bptree.delete(&1).await.unwrap();
        assert_eq!(bptree.len(), 1);

        bptree.delete(&2).await.unwrap();
        assert!(bptree.is_empty());
        assert_eq!(bptree.len(), 0);
    }

    #[tokio::test]
    async fn test_update_and_upsert() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let mut bptree: BPTree<u32, u32, _> = BPTree::new(storage).await.unwrap();

        assert_eq!(bptree.update(1, 10).await.unwrap(), None);
        assert_eq!(bptree.len(), 0);

        assert_eq!(bptree.upsert(1, 10).await.unwrap(), None);
        assert_eq!(bptree.len(), 1);

        assert_eq!(bptree.update(1, 11).await.unwrap(), Some(10));
        assert_eq!(bptree.get(&1).await.unwrap(), Some(11));
        assert_eq!(bptree.len(), 1);

        assert_eq!(bptree.upsert(1, 12).await.unwrap(), Some(11));
        assert_eq!(bptree.get(&1).await.unwrap(), Some(12));
        assert_eq!(bptree.len(), 1);
    }

    #[tokio::test]
    async fn test_clear() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let mut bptree: BPTree<u32, u32, _> = BPTree::new(storage).await.unwrap();

        for i in 0..10 {
            bptree.insert(i, i * 10).await.unwrap();
        }
        assert_eq!(bptree.len(), 10);

        bptree.clear().await.unwrap();
        assert!(bptree.is_empty());
        assert_eq!(bptree.len(), 0);
        assert!(!bptree.root_exists);
    }

    #[tokio::test]
    async fn test_range_query() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();
        let mut bptree: BPTree<u32, String, _> = BPTree::new(storage).await.unwrap();

        for i in 0..10 {
            bptree.insert(i, format!("val{}", i)).await.unwrap();
        }

        let range = bptree.range(3..7).await.unwrap();
        assert_eq!(range.len(), 4);
        assert_eq!(range[0].0, 3);
        assert_eq!(range[3].0, 6);

        let full_range = bptree.range(..).await.unwrap();
        assert_eq!(full_range.len(), 10);
    }

    #[tokio::test]
    async fn test_tree_version_mismatch() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();

        // Initialize tree
        let mut storage = {
            let _bptree: BPTree<u32, u32, _> = BPTree::new(storage).await.unwrap();
            _bptree.storage
        };

        // Manually corrupt root block with wrong version
        let mut data = storage
            .read_block(BPTree::<u32, u32, FileBlockStorage>::ROOT_ID)
            .await
            .unwrap();
        let mut metadata: BPTreeMetadata = bincode::deserialize(&data).unwrap();
        metadata.version = 99;
        let metadata_data = bincode::serialize(&metadata).unwrap();
        data[..metadata_data.len()].copy_from_slice(&metadata_data);
        storage
            .write_block(BPTree::<u32, u32, FileBlockStorage>::ROOT_ID, &data)
            .await
            .unwrap();

        // Try to open
        let result = BPTree::<u32, u32, _>::new(storage).await;
        match result {
            Err(StorageError::VersionMismatch { expected, actual }) => {
                assert_eq!(expected, METADATA_V1);
                assert_eq!(actual, 99);
            }
            _ => panic!("Expected VersionMismatch, got {:?}", result),
        }
    }

    #[tokio::test]
    async fn test_tree_block_size_mismatch() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FileBlockStorage::new(temp_dir.path(), 4096).unwrap();

        // Initialize tree
        {
            let _bptree: BPTree<u32, u32, _> = BPTree::new(storage).await.unwrap();
        }

        // Try to open with different block size storage
        let storage2 = FileBlockStorage::new(temp_dir.path(), 2048).unwrap();
        let result = BPTree::<u32, u32, _>::new(storage2).await;
        match result {
            Err(StorageError::MetadataMismatch(msg)) => {
                assert!(msg.contains("Block size mismatch"));
            }
            _ => panic!("Expected MetadataMismatch, got {:?}", result),
        }
    }
}
