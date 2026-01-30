use crate::storage::{BlockId, StorageError};
use serde::{Deserialize, Serialize};

/// Universal node structure that can represent both internal and leaf nodes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniversalNode<K, V> {
    /// Number of keys in this node
    pub key_count: u32,

    /// Keys for this node (always present)
    pub keys: Vec<K>,

    /// Child block IDs (only for internal nodes, empty for leaf nodes)
    pub child_ids: Vec<BlockId>,

    /// Values for keys (only for leaf nodes, empty for internal nodes)
    pub values: Vec<V>,

    /// Pointer to next leaf node (only for leaf nodes)
    pub next_leaf: Option<BlockId>,

    /// Whether this node needs to be written back to storage
    #[serde(skip)]
    pub is_dirty: bool,

    /// Explicit node type to avoid ambiguity
    pub node_type: NodeType,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum NodeType {
    #[default]
    Leaf,
    Internal,
}

impl<K, V> UniversalNode<K, V>
where
    K: Ord + Clone + Serialize + for<'de> Deserialize<'de>,
    V: Clone + Serialize + for<'de> Deserialize<'de>,
{
    /// Create a new empty internal node
    pub fn new_internal() -> Self {
        Self {
            key_count: 0,
            keys: Vec::new(),
            child_ids: Vec::new(),
            values: Vec::new(),
            next_leaf: None,
            is_dirty: true,
            node_type: NodeType::Internal,
        }
    }

    /// Create a new empty leaf node
    pub fn new_leaf() -> Self {
        Self {
            key_count: 0,
            keys: Vec::new(),
            child_ids: Vec::new(),
            values: Vec::new(),
            next_leaf: None,
            is_dirty: true,
            node_type: NodeType::Leaf,
        }
    }

    /// Check if this is an internal node
    pub fn is_internal(&self) -> bool {
        matches!(self.node_type, NodeType::Internal)
    }

    /// Check if this is a leaf node
    pub fn is_leaf(&self) -> bool {
        matches!(self.node_type, NodeType::Leaf)
    }

    /// Validate node consistency
    pub fn validate(&self) -> Result<(), StorageError> {
        // Check key count matches actual keys
        if self.key_count as usize != self.keys.len() {
            return Err(StorageError::BlockCorrupted(0));
        }

        if self.is_internal() {
            // Internal node validation
            if self.child_ids.len() != self.key_count as usize + 1 {
                return Err(StorageError::BlockCorrupted(0));
            }
            if !self.values.is_empty() {
                return Err(StorageError::BlockCorrupted(0));
            }
            if self.next_leaf.is_some() {
                return Err(StorageError::BlockCorrupted(0));
            }
        } else {
            // Leaf node validation
            if !self.child_ids.is_empty() {
                return Err(StorageError::BlockCorrupted(0));
            }
            if self.values.len() != self.key_count as usize {
                return Err(StorageError::BlockCorrupted(0));
            }
        }

        Ok(())
    }

    /// Find the index where a key should be inserted
    pub fn find_key_index(&self, key: &K) -> usize {
        self.keys.binary_search(key).unwrap_or_else(|x| x)
    }

    /// Get the child ID for a given key index (internal nodes only)
    pub fn get_child_id(&self, key_idx: usize) -> Option<BlockId> {
        if self.is_internal() && key_idx < self.child_ids.len() {
            Some(self.child_ids[key_idx])
        } else {
            None
        }
    }

    /// Insert a key-value pair into a leaf node
    pub fn insert_leaf(&mut self, key: K, value: V) -> Result<(), StorageError> {
        if !self.is_leaf() {
            return Err(StorageError::BlockCorrupted(0));
        }

        let index = self.find_key_index(&key);
        self.keys.insert(index, key);
        self.values.insert(index, value);
        self.key_count += 1;
        self.is_dirty = true;

        Ok(())
    }

    /// Insert a key and split point into an internal node
    pub fn insert_internal(
        &mut self,
        key: K,
        left_child: BlockId,
        right_child: BlockId,
    ) -> Result<(), StorageError> {
        if !self.is_internal() {
            return Err(StorageError::BlockCorrupted(0));
        }

        // For empty internal node, we need to set up child_ids correctly
        if self.child_ids.is_empty() {
            self.keys.push(key);
            self.child_ids.push(left_child);
            self.child_ids.push(right_child);
        } else {
            let index = self.find_key_index(&key);
            self.keys.insert(index, key);
            // Insert right child at index + 1, replace left child at index
            self.child_ids.insert(index + 1, right_child);
            self.child_ids[index] = left_child;
        }

        self.key_count += 1;
        self.is_dirty = true;

        Ok(())
    }

    /// Get a value by key (leaf nodes only)
    pub fn get(&self, key: &K) -> Option<&V> {
        if !self.is_leaf() {
            return None;
        }

        match self.keys.binary_search(key) {
            Ok(index) => self.values.get(index),
            Err(_) => None,
        }
    }

    /// Remove a key-value pair from a leaf node
    pub fn remove_leaf(&mut self, key: &K) -> Option<V> {
        if !self.is_leaf() {
            return None;
        }

        match self.keys.binary_search(key) {
            Ok(index) => {
                self.keys.remove(index);
                let value = self.values.remove(index);
                self.key_count -= 1;
                self.is_dirty = true;
                Some(value)
            }
            Err(_) => None,
        }
    }

    /// Split this node into two, returning the split key and new node
    pub fn split(&mut self) -> Result<(K, UniversalNode<K, V>), StorageError>
    where
        K: Clone,
    {
        if self.key_count < 2 {
            return Err(StorageError::BlockCorrupted(0));
        }

        let split_idx = self.keys.len() / 2;

        if self.is_leaf() {
            let split_key = self.keys[split_idx].clone();
            let right_keys = self.keys.split_off(split_idx);
            let right_values = self.values.split_off(split_idx);

            let new_node = Self {
                key_count: right_keys.len() as u32,
                keys: right_keys,
                child_ids: Vec::new(),
                values: right_values,
                next_leaf: self.next_leaf,
                is_dirty: true,
                node_type: NodeType::Leaf,
            };

            self.key_count = self.keys.len() as u32;
            self.is_dirty = true;

            Ok((split_key, new_node))
        } else {
            let split_key = self.keys[split_idx].clone();
            let right_keys = self.keys.split_off(split_idx + 1);
            let right_children = self.child_ids.split_off(split_idx + 1);
            self.keys.pop(); // Remove the promoted key from left node

            let new_node = Self {
                key_count: right_keys.len() as u32,
                keys: right_keys,
                child_ids: right_children,
                values: Vec::new(),
                next_leaf: None,
                is_dirty: true,
                node_type: NodeType::Internal,
            };

            self.key_count = self.keys.len() as u32;
            self.is_dirty = true;

            Ok((split_key, new_node))
        }
    }

    /// Merge this node with another (used during deletion)
    pub fn merge(
        &mut self,
        other: &UniversalNode<K, V>,
        separator: Option<K>,
    ) -> Result<(), StorageError>
    where
        K: Clone,
    {
        if self.is_internal() != other.is_internal() {
            return Err(StorageError::BlockCorrupted(0));
        }

        if self.is_internal() {
            if let Some(sep) = separator {
                self.keys.push(sep);
            }
            self.keys.extend(other.keys.iter().cloned());
            self.child_ids.extend(&other.child_ids);
        } else {
            self.keys.extend(other.keys.iter().cloned());
            self.values.extend(other.values.iter().cloned());
            self.next_leaf = other.next_leaf;
        }

        self.key_count = self.keys.len() as u32;
        self.is_dirty = true;

        Ok(())
    }

    pub fn borrow_from_left(
        &mut self,
        left_sibling: &mut UniversalNode<K, V>,
        parent_separator: &mut K,
    ) -> Result<(), StorageError>
    where
        K: Clone,
    {
        if self.is_leaf() {
            if let Some(key) = left_sibling.keys.pop() {
                if let Some(value) = left_sibling.values.pop() {
                    self.keys.insert(0, key.clone());
                    self.values.insert(0, value);
                    *parent_separator = key;
                }
            }
        } else {
            // Internal node redistribution (B-tree style)
            if let Some(key) = left_sibling.keys.pop() {
                if let Some(child) = left_sibling.child_ids.pop() {
                    // Parent separator comes down to this node
                    self.keys.insert(0, parent_separator.clone());
                    self.child_ids.insert(0, child);
                    // Left sibling's key moves up to parent
                    *parent_separator = key;
                }
            }
        }

        left_sibling.key_count = left_sibling.keys.len() as u32;
        self.key_count = self.keys.len() as u32;
        left_sibling.is_dirty = true;
        self.is_dirty = true;

        Ok(())
    }

    pub fn borrow_from_right(
        &mut self,
        right_sibling: &mut UniversalNode<K, V>,
        parent_separator: &mut K,
    ) -> Result<(), StorageError>
    where
        K: Clone,
    {
        if self.is_leaf() {
            if !right_sibling.keys.is_empty() {
                let key = right_sibling.keys.remove(0);
                let value = right_sibling.values.remove(0);
                self.keys.push(key);
                self.values.push(value);
                // Update parent separator to the NEW first key of right sibling
                if let Some(next_key) = right_sibling.keys.first() {
                    *parent_separator = next_key.clone();
                }
            }
        } else {
            // Internal node redistribution
            if !right_sibling.keys.is_empty() {
                let key = right_sibling.keys.remove(0);
                let child = right_sibling.child_ids.remove(0);
                // Parent separator comes down to this node
                self.keys.push(parent_separator.clone());
                self.child_ids.push(child);
                // Right sibling's key moves up to parent
                *parent_separator = key;
            }
        }

        right_sibling.key_count = right_sibling.keys.len() as u32;
        self.key_count = self.keys.len() as u32;
        right_sibling.is_dirty = true;
        self.is_dirty = true;

        Ok(())
    }

    pub fn first_key(&self) -> Option<&K> {
        self.keys.first()
    }

    pub fn last_key(&self) -> Option<&K> {
        self.keys.last()
    }

    pub fn min_keys(&self, max_keys: usize) -> usize {
        (max_keys + 1) / 2 - 1
    }
}

impl<K, V> Default for UniversalNode<K, V>
where
    K: Ord + Clone + Serialize + for<'de> Deserialize<'de>,
    V: Clone + Serialize + for<'de> Deserialize<'de>,
{
    fn default() -> Self {
        Self::new_leaf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_internal_vs_leaf_detection() {
        let internal: UniversalNode<u32, u32> = UniversalNode {
            key_count: 2,
            keys: vec![1, 3],
            child_ids: vec![0, 1, 2],
            values: vec![],
            next_leaf: None,
            is_dirty: false,
            node_type: NodeType::Internal,
        };

        let leaf: UniversalNode<u32, u32> = UniversalNode {
            key_count: 2,
            keys: vec![1, 3],
            child_ids: vec![],
            values: vec![10, 30],
            next_leaf: None,
            is_dirty: false,
            node_type: NodeType::Leaf,
        };

        assert!(internal.is_internal());
        assert!(!internal.is_leaf());
        assert!(leaf.is_leaf());
        assert!(!leaf.is_internal());
    }

    #[test]
    fn test_leaf_insert_get() {
        let mut node = UniversalNode::<u32, u32>::new_leaf();

        node.insert_leaf(5, 50).unwrap();
        node.insert_leaf(3, 30).unwrap();
        node.insert_leaf(7, 70).unwrap();

        assert_eq!(node.get(&3), Some(&30));
        assert_eq!(node.get(&5), Some(&50));
        assert_eq!(node.get(&7), Some(&70));
        assert_eq!(node.get(&4), None);
    }

    #[test]
    fn test_internal_operations() {
        let mut node = UniversalNode::<u32, u32>::new_internal();

        node.insert_internal(5, 0, 1).unwrap();
        assert_eq!(node.keys, vec![5]);
        assert_eq!(node.child_ids, vec![0, 1]);

        assert_eq!(node.get_child_id(0), Some(0));
        assert_eq!(node.get_child_id(1), Some(1));

        node.validate().unwrap();
    }

    #[test]
    fn test_node_split() {
        let mut leaf = UniversalNode::<u32, u32>::new_leaf();
        for i in 0..4 {
            leaf.insert_leaf(i * 2, i * 20).unwrap();
        }

        let (split_key, right_node) = leaf.split().unwrap();
        assert_eq!(split_key, 4);
        assert_eq!(leaf.keys, vec![0, 2]);
        assert_eq!(right_node.keys, vec![4, 6]);
    }
}
