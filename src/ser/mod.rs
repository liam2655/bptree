use crate::btree::node::UniversalNode;
use crate::storage::StorageError;

/// Serialization utilities for block storage
pub struct BlockSerializer;

impl BlockSerializer {
    /// Serialize a node to a byte vector, ensuring it fits within block_size
    pub fn serialize_node<K, V>(
        node: &UniversalNode<K, V>,
        block_size: usize,
    ) -> Result<Vec<u8>, StorageError>
    where
        K: serde::Serialize,
        V: serde::Serialize,
    {
        let data = bincode::serialize(node)?;

        if data.len() > block_size {
            return Err(StorageError::InvalidBlockSize {
                expected: block_size,
                actual: data.len(),
            });
        }

        Ok(data)
    }

    /// Deserialize a node from a byte vector
    pub fn deserialize_node<K, V>(data: &[u8]) -> Result<UniversalNode<K, V>, StorageError>
    where
        K: for<'de> serde::Deserialize<'de> + serde::Serialize + Ord + Clone,
        V: for<'de> serde::Deserialize<'de> + serde::Serialize + Clone,
    {
        let mut node: UniversalNode<K, V> = bincode::deserialize(data)?;

        // Set node type based on content - internal nodes have children, leaf nodes have values
        if !node.child_ids.is_empty() {
            node.node_type = crate::btree::node::NodeType::Internal;
        } else {
            node.node_type = crate::btree::node::NodeType::Leaf;
        }

        node.validate()?;
        Ok(node)
    }

    /// Pad data to fill the entire block
    pub fn pad_to_block(mut data: Vec<u8>, block_size: usize) -> Vec<u8> {
        if data.len() < block_size {
            data.resize(block_size, 0);
        }
        data
    }

    /// Serialize node and pad to block size
    pub fn serialize_node_padded<K, V>(
        node: &UniversalNode<K, V>,
        block_size: usize,
    ) -> Result<Vec<u8>, StorageError>
    where
        K: serde::Serialize,
        V: serde::Serialize,
    {
        let data = Self::serialize_node(node, block_size)?;
        Ok(Self::pad_to_block(data, block_size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::btree::node::UniversalNode;

    #[test]
    fn test_serialize_deserialize_leaf() {
        let mut node = UniversalNode::<String, String>::new_leaf();
        node.insert_leaf("key1".to_string(), "value1".to_string())
            .unwrap();
        node.insert_leaf("key2".to_string(), "value2".to_string())
            .unwrap();

        let serialized = BlockSerializer::serialize_node(&node, 1024).unwrap();
        let deserialized: UniversalNode<String, String> =
            BlockSerializer::deserialize_node(&serialized).unwrap();

        assert_eq!(deserialized.keys, node.keys);
        assert_eq!(deserialized.values, node.values);
        assert!(deserialized.is_leaf());
    }

    #[test]
    fn test_serialize_deserialize_internal() {
        let mut node = UniversalNode::<u32, ()>::new_internal();
        node.insert_internal(5, 0, 1).unwrap();
        node.insert_internal(3, 1, 2).unwrap();

        let serialized = BlockSerializer::serialize_node(&node, 1024).unwrap();
        let deserialized: UniversalNode<u32, ()> =
            BlockSerializer::deserialize_node(&serialized).unwrap();

        assert_eq!(deserialized.keys, node.keys);
        assert_eq!(deserialized.child_ids, node.child_ids);
        assert!(deserialized.is_internal());
    }

    #[test]
    fn test_block_padding() {
        let node = UniversalNode::<u32, u32>::new_leaf();
        let padded = BlockSerializer::serialize_node_padded(&node, 1024).unwrap();
        assert_eq!(padded.len(), 1024);
    }

    #[test]
    fn test_oversized_node() {
        let mut node = UniversalNode::<u32, u32>::new_leaf();
        // Create a node that would be too large for a tiny block
        for i in 0..1000 {
            node.insert_leaf(i, i).unwrap();
        }

        let result = BlockSerializer::serialize_node(&node, 100);
        assert!(result.is_err());
    }
}
