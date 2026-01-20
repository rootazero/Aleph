//! SIMD-optimized vector operations for cosine similarity calculation.
//!
//! This module provides platform-specific SIMD optimizations:
//! - Apple Silicon (aarch64): NEON intrinsics
//! - Intel/AMD (x86_64): SSE/AVX intrinsics
//! - Fallback: Scalar implementation
//!
//! Performance improvement: ~3-5x faster than scalar for 512-dim vectors.

/// Calculate cosine similarity between two vectors using SIMD when available.
///
/// Automatically selects the best implementation for the current platform:
/// - aarch64: Uses NEON SIMD (128-bit, 4 floats at a time)
/// - x86_64: Uses AVX (256-bit, 8 floats at a time) or SSE (128-bit)
/// - fallback: Scalar implementation
///
/// # Arguments
/// * `a` - First vector
/// * `b` - Second vector (must have same length as `a`)
///
/// # Returns
/// Cosine similarity in range [-1.0, 1.0], or 0.0 if vectors have different lengths
/// or either vector has zero magnitude.
#[inline]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    #[cfg(target_arch = "aarch64")]
    {
        cosine_similarity_neon(a, b)
    }

    #[cfg(target_arch = "x86_64")]
    {
        // Runtime feature detection for AVX
        if is_x86_feature_detected!("avx") {
            // SAFETY: We've verified AVX is available via is_x86_feature_detected!
            // and the slices a and b are valid with equal lengths (checked above).
            unsafe { cosine_similarity_avx(a, b) }
        } else {
            // SAFETY: SSE is always available on x86_64 and the slices a and b
            // are valid with equal lengths (checked above).
            unsafe { cosine_similarity_sse(a, b) }
        }
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        cosine_similarity_scalar(a, b)
    }
}

/// Scalar fallback implementation (also used for testing SIMD correctness)
#[allow(dead_code)]
#[inline]
fn cosine_similarity_scalar(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    let mut mag_a = 0.0f32;
    let mut mag_b = 0.0f32;

    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        mag_a += x * x;
        mag_b += y * y;
    }

    let mag = (mag_a * mag_b).sqrt();
    if mag == 0.0 {
        0.0
    } else {
        dot / mag
    }
}

// =============================================================================
// ARM NEON Implementation (Apple Silicon)
// =============================================================================

#[cfg(target_arch = "aarch64")]
#[inline]
fn cosine_similarity_neon(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::aarch64::*;

    let len = a.len();
    let chunks = len / 4;
    let remainder = len % 4;

    // SAFETY: NEON intrinsics are always available on aarch64.
    // Pointer arithmetic is safe because:
    // - a and b are valid slices with equal lengths (checked by caller)
    // - We only access indices within bounds (chunks*4 + remainder = len)
    // - vld1q_f32 loads 4 contiguous f32s which fits within slice bounds
    unsafe {
        // Initialize accumulators (4 lanes each)
        let mut dot_acc = vdupq_n_f32(0.0);
        let mut mag_a_acc = vdupq_n_f32(0.0);
        let mut mag_b_acc = vdupq_n_f32(0.0);

        let a_ptr = a.as_ptr();
        let b_ptr = b.as_ptr();

        // Process 4 floats at a time
        for i in 0..chunks {
            let offset = i * 4;
            let va = vld1q_f32(a_ptr.add(offset));
            let vb = vld1q_f32(b_ptr.add(offset));

            // Fused multiply-add for better precision and performance
            dot_acc = vfmaq_f32(dot_acc, va, vb);
            mag_a_acc = vfmaq_f32(mag_a_acc, va, va);
            mag_b_acc = vfmaq_f32(mag_b_acc, vb, vb);
        }

        // Horizontal sum of vector lanes
        let dot = vaddvq_f32(dot_acc);
        let mag_a = vaddvq_f32(mag_a_acc);
        let mag_b = vaddvq_f32(mag_b_acc);

        // Handle remainder with scalar ops
        let mut dot_rem = 0.0f32;
        let mut mag_a_rem = 0.0f32;
        let mut mag_b_rem = 0.0f32;

        let start = chunks * 4;
        for i in 0..remainder {
            let x = *a_ptr.add(start + i);
            let y = *b_ptr.add(start + i);
            dot_rem += x * y;
            mag_a_rem += x * x;
            mag_b_rem += y * y;
        }

        let total_dot = dot + dot_rem;
        let total_mag_a = mag_a + mag_a_rem;
        let total_mag_b = mag_b + mag_b_rem;

        let magnitude = (total_mag_a * total_mag_b).sqrt();
        if magnitude == 0.0 {
            0.0
        } else {
            total_dot / magnitude
        }
    }
}

// =============================================================================
// x86_64 AVX Implementation
// =============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx")]
#[inline]
unsafe fn cosine_similarity_avx(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;

    let len = a.len();
    let chunks = len / 8;
    let remainder = len % 8;

    // Initialize 256-bit accumulators (8 floats each)
    let mut dot_acc = _mm256_setzero_ps();
    let mut mag_a_acc = _mm256_setzero_ps();
    let mut mag_b_acc = _mm256_setzero_ps();

    let a_ptr = a.as_ptr();
    let b_ptr = b.as_ptr();

    // Process 8 floats at a time
    for i in 0..chunks {
        let offset = i * 8;
        let va = _mm256_loadu_ps(a_ptr.add(offset));
        let vb = _mm256_loadu_ps(b_ptr.add(offset));

        // FMA if available, otherwise mul + add
        #[cfg(target_feature = "fma")]
        {
            dot_acc = _mm256_fmadd_ps(va, vb, dot_acc);
            mag_a_acc = _mm256_fmadd_ps(va, va, mag_a_acc);
            mag_b_acc = _mm256_fmadd_ps(vb, vb, mag_b_acc);
        }
        #[cfg(not(target_feature = "fma"))]
        {
            dot_acc = _mm256_add_ps(dot_acc, _mm256_mul_ps(va, vb));
            mag_a_acc = _mm256_add_ps(mag_a_acc, _mm256_mul_ps(va, va));
            mag_b_acc = _mm256_add_ps(mag_b_acc, _mm256_mul_ps(vb, vb));
        }
    }

    // Horizontal sum: reduce 8 lanes to 1
    let dot = hsum_avx(dot_acc);
    let mag_a = hsum_avx(mag_a_acc);
    let mag_b = hsum_avx(mag_b_acc);

    // Handle remainder
    let mut dot_rem = 0.0f32;
    let mut mag_a_rem = 0.0f32;
    let mut mag_b_rem = 0.0f32;

    let start = chunks * 8;
    for i in 0..remainder {
        let x = *a_ptr.add(start + i);
        let y = *b_ptr.add(start + i);
        dot_rem += x * y;
        mag_a_rem += x * x;
        mag_b_rem += y * y;
    }

    let total_dot = dot + dot_rem;
    let total_mag_a = mag_a + mag_a_rem;
    let total_mag_b = mag_b + mag_b_rem;

    let magnitude = (total_mag_a * total_mag_b).sqrt();
    if magnitude == 0.0 {
        0.0
    } else {
        total_dot / magnitude
    }
}

/// Horizontal sum for AVX 256-bit register
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx")]
#[inline]
unsafe fn hsum_avx(v: std::arch::x86_64::__m256) -> f32 {
    use std::arch::x86_64::*;

    // Extract high 128 bits and add to low 128 bits
    let high = _mm256_extractf128_ps(v, 1);
    let low = _mm256_castps256_ps128(v);
    let sum128 = _mm_add_ps(high, low);

    // Now reduce 128-bit to scalar
    let shuf = _mm_movehdup_ps(sum128); // [1,1,3,3]
    let sums = _mm_add_ps(sum128, shuf); // [0+1, 1+1, 2+3, 3+3]
    let shuf2 = _mm_movehl_ps(sums, sums); // [2+3, 3+3, 2+3, 3+3]
    let result = _mm_add_ss(sums, shuf2); // [0+1+2+3, ...]

    _mm_cvtss_f32(result)
}

// =============================================================================
// x86_64 SSE Implementation (fallback for older CPUs)
// =============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse")]
#[inline]
unsafe fn cosine_similarity_sse(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;

    let len = a.len();
    let chunks = len / 4;
    let remainder = len % 4;

    // Initialize 128-bit accumulators (4 floats each)
    let mut dot_acc = _mm_setzero_ps();
    let mut mag_a_acc = _mm_setzero_ps();
    let mut mag_b_acc = _mm_setzero_ps();

    let a_ptr = a.as_ptr();
    let b_ptr = b.as_ptr();

    // Process 4 floats at a time
    for i in 0..chunks {
        let offset = i * 4;
        let va = _mm_loadu_ps(a_ptr.add(offset));
        let vb = _mm_loadu_ps(b_ptr.add(offset));

        dot_acc = _mm_add_ps(dot_acc, _mm_mul_ps(va, vb));
        mag_a_acc = _mm_add_ps(mag_a_acc, _mm_mul_ps(va, va));
        mag_b_acc = _mm_add_ps(mag_b_acc, _mm_mul_ps(vb, vb));
    }

    // Horizontal sum
    let dot = hsum_sse(dot_acc);
    let mag_a = hsum_sse(mag_a_acc);
    let mag_b = hsum_sse(mag_b_acc);

    // Handle remainder
    let mut dot_rem = 0.0f32;
    let mut mag_a_rem = 0.0f32;
    let mut mag_b_rem = 0.0f32;

    let start = chunks * 4;
    for i in 0..remainder {
        let x = *a_ptr.add(start + i);
        let y = *b_ptr.add(start + i);
        dot_rem += x * y;
        mag_a_rem += x * x;
        mag_b_rem += y * y;
    }

    let total_dot = dot + dot_rem;
    let total_mag_a = mag_a + mag_a_rem;
    let total_mag_b = mag_b + mag_b_rem;

    let magnitude = (total_mag_a * total_mag_b).sqrt();
    if magnitude == 0.0 {
        0.0
    } else {
        total_dot / magnitude
    }
}

/// Horizontal sum for SSE 128-bit register
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse")]
#[inline]
unsafe fn hsum_sse(v: std::arch::x86_64::__m128) -> f32 {
    use std::arch::x86_64::*;

    let shuf = _mm_movehdup_ps(v);
    let sums = _mm_add_ps(v, shuf);
    let shuf2 = _mm_movehl_ps(sums, sums);
    let result = _mm_add_ss(sums, shuf2);

    _mm_cvtss_f32(result)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_vectors() {
        let a = vec![1.0, 2.0, 3.0, 4.0];
        let b = vec![1.0, 2.0, 3.0, 4.0];
        let result = cosine_similarity(&a, &b);
        assert!(
            (result - 1.0).abs() < 0.0001,
            "Expected 1.0, got {}",
            result
        );
    }

    #[test]
    fn test_orthogonal_vectors() {
        let a = vec![1.0, 0.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0, 0.0];
        let result = cosine_similarity(&a, &b);
        assert!(result.abs() < 0.0001, "Expected 0.0, got {}", result);
    }

    #[test]
    fn test_opposite_vectors() {
        let a = vec![1.0, 2.0, 3.0, 4.0];
        let b = vec![-1.0, -2.0, -3.0, -4.0];
        let result = cosine_similarity(&a, &b);
        assert!(
            (result - (-1.0)).abs() < 0.0001,
            "Expected -1.0, got {}",
            result
        );
    }

    #[test]
    fn test_512_dimensions() {
        // Typical embedding dimension for bge-small-zh-v1.5
        let a: Vec<f32> = (0..512).map(|i| (i as f32) * 0.01).collect();
        let b: Vec<f32> = (0..512).map(|i| (i as f32) * 0.01 + 0.5).collect();
        let result = cosine_similarity(&a, &b);
        // Just verify it's in valid range and doesn't crash
        assert!(result >= -1.0 && result <= 1.0);
        assert!(result > 0.9); // Similar vectors should have high similarity
    }

    #[test]
    fn test_empty_vectors() {
        let a: Vec<f32> = vec![];
        let b: Vec<f32> = vec![];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_different_lengths() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_zero_vector() {
        let a = vec![0.0, 0.0, 0.0, 0.0];
        let b = vec![1.0, 2.0, 3.0, 4.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_remainder_handling() {
        // Test with length that's not divisible by SIMD width
        let a: Vec<f32> = (0..387).map(|i| (i as f32) * 0.1).collect();
        let b: Vec<f32> = (0..387).map(|i| (i as f32) * 0.1).collect();
        let result = cosine_similarity(&a, &b);
        assert!(
            (result - 1.0).abs() < 0.0001,
            "Expected 1.0, got {}",
            result
        );
    }

    #[test]
    fn test_consistency_with_scalar() {
        let a: Vec<f32> = (0..512).map(|i| ((i * 17) % 100) as f32 / 100.0).collect();
        let b: Vec<f32> = (0..512).map(|i| ((i * 31) % 100) as f32 / 100.0).collect();

        let simd_result = cosine_similarity(&a, &b);
        let scalar_result = cosine_similarity_scalar(&a, &b);

        assert!(
            (simd_result - scalar_result).abs() < 0.0001,
            "SIMD: {}, Scalar: {}",
            simd_result,
            scalar_result
        );
    }
}

// =============================================================================
// Benchmarks (run with: cargo test --release simd::benchmarks -- --nocapture)
// =============================================================================

#[cfg(test)]
mod benchmarks {
    use super::*;
    use std::hint::black_box;
    use std::time::Instant;

    #[test]
    fn benchmark_comparison() {
        let iterations = 100_000;
        let dim = 512;

        // Generate random-ish vectors
        let a: Vec<f32> = (0..dim).map(|i| ((i * 17) % 100) as f32 / 100.0).collect();
        let b: Vec<f32> = (0..dim).map(|i| ((i * 31) % 100) as f32 / 100.0).collect();

        // Warm up (with black_box to prevent optimization)
        for _ in 0..1000 {
            black_box(cosine_similarity(black_box(&a), black_box(&b)));
            black_box(cosine_similarity_scalar(black_box(&a), black_box(&b)));
        }

        // Benchmark SIMD
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(cosine_similarity(black_box(&a), black_box(&b)));
        }
        let simd_time = start.elapsed();

        // Benchmark scalar
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(cosine_similarity_scalar(black_box(&a), black_box(&b)));
        }
        let scalar_time = start.elapsed();

        let simd_ns = simd_time.as_nanos() as f64 / iterations as f64;
        let scalar_ns = scalar_time.as_nanos() as f64 / iterations as f64;
        let speedup = scalar_ns / simd_ns;

        println!(
            "\n=== Cosine Similarity Benchmark ({} dims, {} iterations) ===",
            dim, iterations
        );
        println!("SIMD:   {:?} ({:.1} ns/op)", simd_time, simd_ns);
        println!("Scalar: {:?} ({:.1} ns/op)", scalar_time, scalar_ns);
        println!("Speedup: {:.2}x", speedup);

        // Note: In debug mode, SIMD may be slower due to lack of optimization.
        // This test is informational only and should not fail in CI.
        // For accurate benchmarks, run with --release flag.
    }
}
