use bptree::{BPTree, FileBlockStorage};
use rand::{Rng, seq::SliceRandom};
use std::collections::BTreeMap;
use std::error::Error;
use tempfile::TempDir;

#[madsim::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let temp_dir = TempDir::new()?;
    let storage_path = temp_dir.path();

    // Use a small block size to force many splits and merges
    let block_size = 512;
    let storage = FileBlockStorage::new(storage_path, block_size)?;
    let mut tree: BPTree<u64, u64, _> = BPTree::new(storage).await?;

    let mut expected_state = BTreeMap::new();
    let mut rng = rand::thread_rng();

    let iterations = 10000;
    println!("Starting simulation with {} iterations...", iterations);

    for i in 0..iterations {
        let op = rng.gen_range(0..100);

        if op < 40 {
            // Insert
            let key = rng.gen_range(0..10000);
            let value = rng.gen_range(0..100000);
            tree.insert(key, value).await?;
            expected_state.insert(key, value);
        } else if op < 70 {
            // Update (upsert)
            if !expected_state.is_empty() {
                let keys: Vec<_> = expected_state.keys().cloned().collect();
                let key = *keys.choose(&mut rng).unwrap();
                let value = rng.gen_range(0..10000);
                tree.insert(key, value).await?;
                expected_state.insert(key, value);
            }
        } else {
            // Delete
            if !expected_state.is_empty() {
                let keys: Vec<_> = expected_state.keys().cloned().collect();
                let key = *keys.choose(&mut rng).unwrap();
                tree.delete(&key).await?;
                expected_state.remove(&key);
            }
        }

        // Periodic full validation
        if i % 100 == 0 {
            tree.validate().await?;
            assert_eq!(
                tree.len() as usize,
                expected_state.len(),
                "Length mismatch at iteration {}",
                i
            );
        }
    }

    println!("Operations complete. Final validation...");
    tree.validate().await?;
    assert_eq!(
        tree.len() as usize,
        expected_state.len(),
        "Final length mismatch"
    );

    // Verify all keys
    for (key, expected_val) in &expected_state {
        let actual_val = tree.get(key).await?;
        assert_eq!(
            actual_val,
            Some(*expected_val),
            "Value mismatch for key {}",
            key
        );
    }

    // Verify all items via range
    let tree_items = tree.range(..).await?;
    let expected_items: Vec<_> = expected_state.iter().map(|(&k, &v)| (k, v)).collect();
    assert_eq!(tree_items, expected_items, "Range mismatch");

    println!("Simulation successful! {} items in tree.", tree.len());

    Ok(())
}
