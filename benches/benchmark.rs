//! Benchmarks for B-tree implementation
//!
//! This module provides comprehensive testing including:
//! - Basic operations benchmarking
//! - Failure injection testing
//! - Performance measurement under various conditions

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use rand::{Rng, thread_rng};
use std::hint::black_box;

use async_trait::async_trait;
use bptree::{BPTree, BlockId, BlockStorage, StorageError};

/// Simulated storage backend with failure injection
#[derive(Clone)]
pub struct SimulatedStorage {
    blocks: Arc<Mutex<HashMap<BlockId, Vec<u8>>>>,
    failure_rate: f64,
    next_id: Arc<Mutex<u64>>,
}

#[derive(Debug, thiserror::Error)]
pub enum SimulatedError {
    #[error("Storage operation failed")]
    OperationFailed,
    #[error("Storage timeout")]
    Timeout,
    #[error("Block not found: {0}")]
    NotFound(BlockId),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl SimulatedStorage {
    pub fn new(failure_rate: f64, _latency: Duration) -> Self {
        Self {
            blocks: Arc::new(Mutex::new(HashMap::new())),
            failure_rate,
            next_id: Arc::new(Mutex::new(2)), // Start from 2, 1 is root, 0 is reserved
        }
    }

    fn inject_failure(&self) -> Result<(), SimulatedError> {
        let mut rng = thread_rng();
        if rng.r#gen::<f64>() < self.failure_rate {
            Err(SimulatedError::OperationFailed)
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl BlockStorage for SimulatedStorage {
    type Error = StorageError;

    async fn read_block(&self, id: BlockId) -> Result<Vec<u8>, Self::Error> {
        self.inject_failure()
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        let blocks = self.blocks.lock().unwrap();
        blocks
            .get(&id)
            .cloned()
            .ok_or(StorageError::BlockNotFound(id))
    }

    async fn write_block(&mut self, id: BlockId, data: &[u8]) -> Result<(), Self::Error> {
        self.inject_failure()
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        let mut blocks = self.blocks.lock().unwrap();
        blocks.insert(id, data.to_vec());
        Ok(())
    }

    async fn allocate_block(&mut self) -> Result<BlockId, Self::Error> {
        self.inject_failure()
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        let mut next_id = self.next_id.lock().unwrap();
        let id = *next_id;
        *next_id += 1;
        Ok(id)
    }

    async fn deallocate_block(&mut self, id: BlockId) -> Result<(), Self::Error> {
        self.inject_failure()
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        let mut blocks = self.blocks.lock().unwrap();
        blocks.remove(&id);
        Ok(())
    }

    async fn sync(&mut self) -> Result<(), Self::Error> {
        self.inject_failure()
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        Ok(())
    }

    fn block_size(&self) -> usize {
        4096
    }
}

/// Basic insertion benchmark
fn bench_insertion(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("insertion_1000", |b| {
        b.to_async(&rt).iter(|| async {
            let storage = SimulatedStorage::new(0.0, Duration::from_nanos(1));
            let mut bptree: BPTree<u64, String, _> =
                BPTree::new(storage).await.expect("Failed to create B-tree");

            for i in 0..1000 {
                let key = black_box(i);
                let value = format!("value-{}", key);
                bptree.insert(key, value).await.expect("Insert failed");
            }
        })
    });
}

/// Lookup benchmark
fn bench_lookup(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let storage = SimulatedStorage::new(0.0, Duration::from_nanos(1));
    let bptree = rt.block_on(async {
        let mut bptree: BPTree<u64, String, _> =
            BPTree::new(storage).await.expect("Failed to create B-tree");

        // Pre-populate with data
        for i in 0..1000 {
            let value = format!("value-{}", i);
            bptree.insert(i, value).await.expect("Insert failed");
        }
        bptree
    });

    c.bench_function("lookup_1000", |b| {
        b.to_async(&rt).iter(|| async {
            for i in 0..1000 {
                let key = black_box(i % 1000);
                bptree.get(&key).await.expect("Lookup failed");
            }
        })
    });
}

/// Mixed operations benchmark
fn bench_mixed_operations(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("mixed_operations", |b| {
        b.to_async(&rt).iter(|| async {
            let storage = SimulatedStorage::new(0.0, Duration::from_nanos(1));
            let mut bptree: BPTree<u32, String, _> =
                BPTree::new(storage).await.expect("Failed to create B-tree");
            let mut rng = thread_rng();

            for i in 0..1000 {
                let key = rng.gen_range(0..10000);
                let value = format!("value-{}", i);

                // 70% insert, 30% lookup
                if rng.gen_range(0.0..1.0) < 0.7 {
                    bptree.insert(key, value).await.expect("Insert failed");
                } else {
                    bptree.get(&key).await.expect("Lookup failed");
                }
            }
        })
    });
}

/// Failure injection benchmark
fn bench_with_failures(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("with_failures");

    for failure_rate in [0.01, 0.05, 0.1] {
        group.bench_with_input(
            format!("failure_rate_{}%", failure_rate * 100.0),
            &failure_rate,
            |b, &failure_rate| {
                b.to_async(&rt).iter(|| async {
                    let storage = SimulatedStorage::new(failure_rate, Duration::from_nanos(1));
                    if let Ok(mut bptree) = BPTree::<u32, String, _>::new(storage).await {
                        let mut rng = thread_rng();
                        let mut success_count = 0;
                        let mut total_count = 0;

                        for i in 0..100 {
                            let key = rng.gen_range(0..1000);
                            let value = format!("value-{}", i);

                            total_count += 1;
                            if bptree.insert(key, value).await.is_ok() {
                                success_count += 1;
                            }
                        }

                        // Return success rate to ensure operations are not optimized away
                        black_box(success_count as f64 / total_count as f64);
                    } else {
                        // B-tree creation failed, return 0 success rate
                        black_box(0.0_f64);
                    }
                })
            },
        );
    }

    group.finish();
}

/// Stress test with many operations
fn bench_stress_test(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("stress_test_10000", |b| {
        b.to_async(&rt).iter(|| async {
            let storage = SimulatedStorage::new(0.0, Duration::from_nanos(1));
            let mut bptree: BPTree<u64, String, _> =
                BPTree::new(storage).await.expect("Failed to create B-tree");

            // Insert many items
            for i in 0..10000 {
                let key = black_box(i);
                let value = format!("value-{}", key);
                if bptree.insert(key, value).await.is_err() {
                    break; // Stop on first error to avoid panicking
                }
            }
        })
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().without_plots().measurement_time(Duration::from_secs(10));
    targets = bench_insertion,
    bench_lookup,
    bench_mixed_operations,
    bench_with_failures,
    bench_stress_test
}

criterion_main!(benches);
