/// Simplified performance benchmarks for memory module

use criterion::{black_box, criterion_group, criterion_main, Criterion};

// Simple microbenchmarks

fn benchmark_string_operations(c: &mut Criterion) {
    c.bench_function("hash_text_short", |bencher| {
        bencher.iter(|| {
            let text = black_box("This is a short test");
            let hash = text.chars().fold(0u64, |acc, ch| acc.wrapping_mul(31).wrapping_add(ch as u64));
            hash
        })
    });

    c.bench_function("hash_text_long", |bencher| {
        let long_text = "word ".repeat(200);
        bencher.iter(|| {
            let text = black_box(&long_text);
            let hash = text.chars().fold(0u64, |acc, ch| acc.wrapping_mul(31).wrapping_add(ch as u64));
            hash
        })
    });
}

fn benchmark_vector_operations(c: &mut Criterion) {
    c.bench_function("normalize_vector_small", |bencher| {
        let vec = vec![1.0f32, 2.0, 3.0, 4.0];
        bencher.iter(|| {
            let v = black_box(&vec);
            let magnitude: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            let normalized: Vec<f32> = v.iter().map(|x| x / magnitude).collect();
            normalized
        })
    });

    c.bench_function("cosine_similarity", |bencher| {
        let a = vec![1.0f32; 384];
        let b_vec = vec![0.5f32; 384];
        bencher.iter(|| {
            let va = black_box(&a);
            let vb = black_box(&b_vec);
            let dot: f32 = va.iter().zip(vb.iter()).map(|(x, y)| x * y).sum();
            let mag_a: f32 = va.iter().map(|x| x * x).sum::<f32>().sqrt();
            let mag_b: f32 = vb.iter().map(|x| x * x).sum::<f32>().sqrt();
            if mag_a == 0.0 || mag_b == 0.0 {
                0.0
            } else {
                dot / (mag_a * mag_b)
            }
        });
    });
}

criterion_group!(benches, benchmark_string_operations, benchmark_vector_operations);
criterion_main!(benches);
