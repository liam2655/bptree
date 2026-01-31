use crate::btree::node::UniversalNode;
use crate::btree::tree::BTree;
use crate::storage::{BlockId, BlockStorage, StorageError};
use serde::{Deserialize, Serialize};
use std::ops::{Bound, RangeBounds};

pub struct BTreeIter<'a, K, V, S>
where
    K: Ord + Clone + Serialize + for<'de> Deserialize<'de>,
    V: Clone + Serialize + for<'de> Deserialize<'de>,
    S: BlockStorage<Error = StorageError>,
{
    btree: &'a BTree<K, V, S>,
    current_leaf_id: Option<BlockId>,
    current_index: usize,
    current_node: Option<UniversalNode<K, V>>,
}

impl<'a, K, V, S> BTreeIter<'a, K, V, S>
where
    K: Ord + Clone + Serialize + for<'de> Deserialize<'de>,
    V: Clone + Serialize + for<'de> Deserialize<'de>,
    S: BlockStorage<Error = StorageError>,
{
    pub fn new(btree: &'a BTree<K, V, S>) -> Result<Self, StorageError> {
        let mut current_leaf_id = btree.root_id;
        let mut current_node = None;

        if let Some(mut id) = current_leaf_id {
            loop {
                let node = btree.load_node(id)?;
                if node.is_leaf() {
                    current_leaf_id = Some(id);
                    current_node = Some(node);
                    break;
                }
                if node.child_ids.is_empty() {
                    current_leaf_id = None;
                    break;
                }
                id = node.child_ids[0];
            }
        }

        Ok(Self {
            btree,
            current_leaf_id,
            current_index: 0,
            current_node,
        })
    }
}

impl<'a, K, V, S> Iterator for BTreeIter<'a, K, V, S>
where
    K: Ord + Clone + Serialize + for<'de> Deserialize<'de>,
    V: Clone + Serialize + for<'de> Deserialize<'de>,
    S: BlockStorage<Error = StorageError>,
{
    type Item = Result<(K, V), StorageError>;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.current_node.as_ref()?;

        if self.current_index < node.keys.len() {
            let key = node.keys[self.current_index].clone();
            let value = node.values[self.current_index].clone();
            self.current_index += 1;
            Some(Ok((key, value)))
        } else {
            // Move to next leaf
            let next_leaf_id = node.next_leaf?;
            match self.btree.load_node(next_leaf_id) {
                Ok(next_node) => {
                    self.current_leaf_id = Some(next_leaf_id);
                    if next_node.keys.is_empty() {
                        self.current_node = None;
                        return None;
                    }
                    self.current_index = 1;
                    let key = next_node.keys[0].clone();
                    let value = next_node.values[0].clone();
                    self.current_node = Some(next_node);
                    Some(Ok((key, value)))
                }
                Err(e) => Some(Err(e)),
            }
        }
    }
}

pub struct BTreeRangeIter<'a, K, V, S, R>
where
    K: Ord + Clone + Serialize + for<'de> Deserialize<'de>,
    V: Clone + Serialize + for<'de> Deserialize<'de>,
    S: BlockStorage<Error = StorageError>,
    R: RangeBounds<K>,
{
    btree: &'a BTree<K, V, S>,
    range: R,
    current_leaf_id: Option<BlockId>,
    current_index: usize,
    current_node: Option<UniversalNode<K, V>>,
    finished: bool,
}

impl<'a, K, V, S, R> BTreeRangeIter<'a, K, V, S, R>
where
    K: Ord + Clone + Serialize + for<'de> Deserialize<'de>,
    V: Clone + Serialize + for<'de> Deserialize<'de>,
    S: BlockStorage<Error = StorageError>,
    R: RangeBounds<K>,
{
    pub fn new(btree: &'a BTree<K, V, S>, range: R) -> Result<Self, StorageError> {
        let root_id = match btree.root_id {
            Some(id) => id,
            None => {
                return Ok(Self {
                    btree,
                    range,
                    current_leaf_id: None,
                    current_index: 0,
                    current_node: None,
                    finished: true,
                })
            }
        };

        // Find the first leaf that could contain the start of the range
        let mut current_id = root_id;
        loop {
            let node = btree.load_node(current_id)?;
            if node.is_leaf() {
                break;
            }
            let start_key = match range.start_bound() {
                Bound::Included(k) => Some(k),
                Bound::Excluded(k) => Some(k),
                Bound::Unbounded => None,
            };

            let idx = if let Some(k) = start_key {
                node.find_key_index(k)
            } else {
                0
            };

            if node.child_ids.is_empty() {
                return Ok(Self {
                    btree,
                    range,
                    current_leaf_id: None,
                    current_index: 0,
                    current_node: None,
                    finished: true,
                });
            }
            current_id = node.child_ids[idx];
        }

        let node = btree.load_node(current_id)?;

        // Find start index
        let current_index = if let Bound::Included(start) = range.start_bound() {
            node.keys.binary_search(start).unwrap_or_else(|x| x)
        } else if let Bound::Excluded(start) = range.start_bound() {
            match node.keys.binary_search(start) {
                Ok(idx) => idx + 1,
                Err(idx) => idx,
            }
        } else {
            0
        };

        Ok(Self {
            btree,
            range,
            current_leaf_id: Some(current_id),
            current_index,
            current_node: Some(node),
            finished: false,
        })
    }
}

impl<'a, K, V, S, R> Iterator for BTreeRangeIter<'a, K, V, S, R>
where
    K: Ord + Clone + Serialize + for<'de> Deserialize<'de>,
    V: Clone + Serialize + for<'de> Deserialize<'de>,
    S: BlockStorage<Error = StorageError>,
    R: RangeBounds<K>,
{
    type Item = Result<(K, V), StorageError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }

        let node = self.current_node.as_ref()?;

        if self.current_index < node.keys.len() {
            let key = &node.keys[self.current_index];

            // Check end bound
            match self.range.end_bound() {
                Bound::Included(end) => {
                    if key > end {
                        self.finished = true;
                        return None;
                    }
                }
                Bound::Excluded(end) => {
                    if key >= end {
                        self.finished = true;
                        return None;
                    }
                }
                Bound::Unbounded => {}
            }

            let result = (key.clone(), node.values[self.current_index].clone());
            self.current_index += 1;
            Some(Ok(result))
        } else {
            // Move to next leaf
            let next_leaf_id = match node.next_leaf {
                Some(id) => id,
                None => {
                    self.finished = true;
                    return None;
                }
            };

            match self.btree.load_node(next_leaf_id) {
                Ok(next_node) => {
                    self.current_leaf_id = Some(next_leaf_id);
                    self.current_index = 0;
                    self.current_node = Some(next_node);
                    self.next() // Recursive call to handle empty nodes or immediate end bound
                }
                Err(e) => {
                    self.finished = true;
                    Some(Err(e))
                }
            }
        }
    }
}
