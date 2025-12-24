/// Performance benchmarks for memory module
///
/// Measures:
/// - Embedding inference time
/// - Vector search performance
/// - End-to-end memory retrieval latency
use aethecore::config::MemoryConfig;
use aethecore::memory::{
    ContextAnchor, EmbeddingModel, MemoryIngestion, MemoryRetrieval, VectorDatabase,
};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::sync::Arc;
use uuid::Uuid;

// Helper to create test database with sample data
async fn create_benchmark_db(num_memories: usize) -> Arc<VectorDatabase> {
    let temp_dir = std::env::temp_dir();
    let db_path = temp_dir.join(format!("bench_{}.db", Uuid::new_v4()));
    let db = Arc::new(VectorDatabase::new(db_path).unwrap());

    let model = Arc::new(EmbeddingModel::new(None).unwrap());
    let config = Arc::new(MemoryConfig::default());
    let ingestion = MemoryIngestion::new(db.clone(), model, config);

    let context = ContextAnchor::now("com.apple.Notes".to_string(), "Benchmark.txt".to_string());

    // Pre-populate with memories
    for i in 0..num_memories {
        ingestion
            .store_memory(
                context.clone(),
                &format!("benchmark input {}", i),
                &format!("benchmark output {}", i),
            )
            .await
            .unwrap();
    }

    db
}

// Benchmark: Embedding inference
fn benchmark_embedding_inference(c: &mut Criterion) {
    let model = EmbeddingModel::new(None).unwrap();

    let mut group = c.benchmark_group("embedding_inference");

    // Short text (10 words)
    group.bench_function("short_text", |b| {
        b.iter(|| {
            let text = black_box("This is a short test input for benchmarking purposes");
            model.embed_text(text).unwrap()
        })
    });

    // Medium text (50 words)
    group.bench_function("medium_text", |b| {
        let text = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(10);
        b.iter(|| {
            let text_ref = black_box(&text);
            model.embed_text(text_ref).unwrap()
        })
    });

    // Long text (200 words)
    group.bench_function("long_text", |b| {
        let text = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(40);
        b.iter(|| {
            let text_ref = black_box(&text);
            model.embed_text(text_ref).unwrap()
        })
    });

    // Batch embedding (10 texts)
    group.bench_function("batch_10_texts", |b| {
        let texts: Vec<String> = (0..10)
            .map(|i| format!("This is test text number {}", i))
            .collect();
        b.iter(|| {
            let texts_ref = black_box(&texts);
            model.embed_batch(texts_ref).unwrap()
        })
    });

    group.finish();
}

// Benchmark: Vector search with different database sizes
fn benchmark_vector_search(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("vector_search");

    for db_size in [10, 50, 100, 500].iter() {
        let db = rt.block_on(create_benchmark_db(*db_size));
        let model = Arc::new(EmbeddingModel::new(None).unwrap());

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}_memories", db_size)),
            db_size,
            |b, _| {
                b.to_async(&rt).iter(|| async {
                    let query_embedding = model.embed_text("benchmark query").unwrap();
                    db.search_memories(
                        black_box("com.apple.Notes"),
                        black_box("Benchmark.txt"),
                        black_box(&query_embedding),
                        5,
                    )
                    .await
                    .unwrap()
                })
            },
        );
    }

    group.finish();
}

// Benchmark: End-to-end memory retrieval
fn benchmark_end_to_end_retrieval(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("end_to_end_retrieval");

    for db_size in [10, 50, 100].iter() {
        let db = rt.block_on(create_benchmark_db(*db_size));
        let model = Arc::new(EmbeddingModel::new(None).unwrap());
        let config = Arc::new(MemoryConfig::default());

        let retrieval = MemoryRetrieval::new(db, model, config);
        let context =
            ContextAnchor::now("com.apple.Notes".to_string(), "Benchmark.txt".to_string());

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}_memories", db_size)),
            db_size,
            |b, _| {
                b.to_async(&rt).iter(|| async {
                    retrieval
                        .retrieve_memories(black_box(&context), black_box("benchmark query"))
                        .await
                        .unwrap()
                })
            },
        );
    }

    group.finish();
}

// Benchmark: Memory ingestion (store)
fn benchmark_memory_ingestion(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("memory_ingestion");

    let temp_dir = std::env::temp_dir();
    let db_path = temp_dir.join(format!("bench_ingestion_{}.db", Uuid::new_v4()));
    let db = Arc::new(VectorDatabase::new(db_path).unwrap());

    let model = Arc::new(EmbeddingModel::new(None).unwrap());
    let config = Arc::new(MemoryConfig::default());

    let ingestion = MemoryIngestion::new(db, model, config);
    let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

    group.bench_function("store_single_memory", |b| {
        let mut counter = 0;
        b.to_async(&rt).iter(|| {
            counter += 1;
            let context_clone = context.clone();
            let ingestion_clone = ingestion.clone();
            async move {
                ingestion_clone
                    .store_memory(
                        black_box(context_clone),
                        black_box(&format!("input {}", counter)),
                        black_box(&format!("output {}", counter)),
                    )
                    .await
                    .unwrap()
            }
        })
    });

    group.finish();
}

// Benchmark: Database statistics
fn benchmark_database_stats(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("database_stats");

    for db_size in [10, 100, 1000].iter() {
        let db = rt.block_on(create_benchmark_db(*db_size));

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}_memories", db_size)),
            db_size,
            |b, _| {
                b.to_async(&rt)
                    .iter(|| async { black_box(db.get_stats().await.unwrap()) })
            },
        );
    }

    group.finish();
}

// Benchmark: Concurrent operations
fn benchmark_concurrent_operations(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("concurrent_operations");

    let db = rt.block_on(create_benchmark_db(50));
    let model = Arc::new(EmbeddingModel::new(None).unwrap());
    let config = Arc::new(MemoryConfig::default());

    let retrieval = MemoryRetrieval::new(db, model, config);
    let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

    group.bench_function("10_concurrent_retrievals", |b| {
        b.to_async(&rt).iter(|| {
            let retrieval = retrieval.clone();
            let context = context.clone();
            async move {
                let mut handles = Vec::new();
                for i in 0..10 {
                    let retrieval = retrieval.clone();
                    let context = context.clone();
                    let handle = tokio::spawn(async move {
                        retrieval
                            .retrieve_memories(&context, &format!("query {}", i))
                            .await
                            .unwrap()
                    });
                    handles.push(handle);
                }
                for handle in handles {
                    let _ = black_box(handle.await.unwrap());
                }
            }
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    benchmark_embedding_inference,
    benchmark_vector_search,
    benchmark_end_to_end_retrieval,
    benchmark_memory_ingestion,
    benchmark_database_stats,
    benchmark_concurrent_operations
);

criterion_main!(benches);
