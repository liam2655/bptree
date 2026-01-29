use btree::{BTree, FileBlockStorage};
use tempfile::TempDir;

#[test]
fn test_multi_block_scenario() {
    let temp_dir = TempDir::new().unwrap();
    let storage_path = temp_dir.path();

    // Create storage with large block size
    let storage = FileBlockStorage::new(storage_path, 16384).unwrap();
    let mut btree: BTree<String, String, _> = BTree::new(storage).unwrap();

    // Insert enough data to trigger multiple blocks and splits
    // Use simple data to ensure we can test multi-node behavior
    for i in 0..100 {
        let key = format!("k{:03}", i);
        let value = format!("v{}", i);
        btree.insert(key, value).unwrap();
    }

    // Verify we can retrieve all inserted data
    for i in 0..100 {
        let key = format!("k{:03}", i);
        let expected_value = format!("v{}", i);

        let retrieved = btree.get(&key).unwrap();
        assert_eq!(
            retrieved,
            Some(expected_value),
            "Failed to retrieve key {}",
            i
        );
    }

    // Verify some missing keys
    assert_eq!(btree.get(&"missing_key".to_string()).unwrap(), None);
    assert_eq!(btree.get(&"k999".to_string()).unwrap(), None);
}

#[test]
fn test_persistence_across_instances() {
    let temp_dir = TempDir::new().unwrap();
    let storage_path = temp_dir.path();

    // First instance - insert data
    {
        let storage = FileBlockStorage::new(storage_path, 16384).unwrap();
        let mut btree: BTree<String, String, _> = BTree::new(storage).unwrap();

        // Insert test data
        for i in 0..200 {
            let key = format!("pk{}", i);
            let value = format!("pv{}", i);
            btree.insert(key, value).unwrap();
        }

        // Verify data exists in first instance
        for i in 0..200 {
            let key = format!("pk{}", i);
            let expected_value = format!("pv{}", i);
            assert_eq!(btree.get(&key).unwrap(), Some(expected_value));
        }

        // BTree is dropped here, ensuring data is persisted
    }

    // Second instance - reopen and verify persistence
    {
        let storage = FileBlockStorage::new(storage_path, 16384).unwrap();
        let mut btree: BTree<String, String, _> = BTree::new(storage).unwrap();

        // Verify all data persisted correctly
        for i in 0..200 {
            let key = format!("pk{}", i);
            let expected_value = format!("pv{}", i);
            let retrieved = btree.get(&key).unwrap();
            assert_eq!(
                retrieved,
                Some(expected_value),
                "Failed to retrieve persisted key {}",
                i
            );
        }

        // Add more data to the reopened tree
        for i in 200..400 {
            let key = format!("pk{}", i);
            let value = format!("pv{}", i);
            btree.insert(key, value).unwrap();
        }
    }

    // Third instance - verify all data including additions
    {
        let storage = FileBlockStorage::new(storage_path, 16384).unwrap();
        let btree: BTree<String, String, _> = BTree::new(storage).unwrap();

        // Verify all 400 keys exist
        for i in 0..400 {
            let key = format!("pk{}", i);
            let expected_value = format!("pv{}", i);
            let retrieved = btree.get(&key).unwrap();
            assert_eq!(
                retrieved,
                Some(expected_value),
                "Failed to retrieve key {} in final verification",
                i
            );
        }
    }
}

#[test]
fn test_large_data_persistence() {
    let temp_dir = TempDir::new().unwrap();
    let storage_path = temp_dir.path();

    // First instance - insert dataset
    {
        let storage = FileBlockStorage::new(storage_path, 16384).unwrap();
        let mut btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();

        // Insert items with integer values (simpler serialization)
        for i in 0..300 {
            btree.insert(i, i * 2).unwrap();
        }

        // Spot check some values
        for i in [0, 100, 200, 299] {
            assert_eq!(btree.get(&i).unwrap(), Some(i * 2));
        }
    }

    // Second instance - verify dataset persisted
    {
        let storage = FileBlockStorage::new(storage_path, 16384).unwrap();
        let btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();

        // Verify all 300 items persisted correctly
        for i in 0..300 {
            assert_eq!(
                btree.get(&i).unwrap(),
                Some(i * 2),
                "Failed to retrieve dataset item {}",
                i
            );
        }
    }
}

#[test]
fn test_ordered_insertion_and_persistence() {
    let temp_dir = TempDir::new().unwrap();
    let storage_path = temp_dir.path();

    // Insert data in sorted order to trigger right-side splits
    {
        let storage = FileBlockStorage::new(storage_path, 2048).unwrap();
        let mut btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();

        for i in 0..200 {
            btree.insert(i, i * 10).unwrap();
        }

        // Verify data exists
        for i in 0..200 {
            assert_eq!(btree.get(&i).unwrap(), Some(i * 10));
        }
    }

    // Reopen and verify
    {
        let storage = FileBlockStorage::new(storage_path, 2048).unwrap();
        let btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();

        for i in 0..200 {
            assert_eq!(btree.get(&i).unwrap(), Some(i * 10));
        }
    }
}

#[test]
fn test_reverse_ordered_insertion_and_persistence() {
    let temp_dir = TempDir::new().unwrap();
    let storage_path = temp_dir.path();

    // Insert data in reverse order to trigger left-side splits
    {
        let storage = FileBlockStorage::new(storage_path, 2048).unwrap();
        let mut btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();

        for i in (0..200).rev() {
            btree.insert(i, i * 10).unwrap();
        }

        // Verify data exists
        for i in 0..200 {
            assert_eq!(btree.get(&i).unwrap(), Some(i * 10));
        }
    }

    // Reopen and verify
    {
        let storage = FileBlockStorage::new(storage_path, 2048).unwrap();
        let btree: BTree<u32, u32, _> = BTree::new(storage).unwrap();

        for i in 0..200 {
            assert_eq!(btree.get(&i).unwrap(), Some(i * 10));
        }
    }
}
