//! Deterministic pseudo-embedding provider for probe tests.
//!
//! Uses a simple hash of the content to generate a fixed-dimension
//! vector. Texts sharing keywords will have higher cosine similarity
//! because shared words contribute identical components.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub const DEFAULT_DIM: usize = 128;

/// Generate a deterministic pseudo-embedding from text content.
pub fn embed(text: &str, dim: usize) -> Vec<f32> {
    let mut vec = vec![0.0_f32; dim];

    for token in text.split_whitespace() {
        let mut hasher = DefaultHasher::new();
        token.to_lowercase().hash(&mut hasher);
        let h = hasher.finish();

        for i in 0..4 {
            let idx = ((h >> (i * 16)) as usize) % dim;
            let sign = if (h >> (i * 8)) & 1 == 0 { 1.0 } else { -1.0 };
            vec[idx] += sign * 0.25;
        }
    }

    // L2-normalise
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut vec {
            *v /= norm;
        }
    } else {
        vec[0] = 1.0;
    }

    vec
}

/// Cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a > 0.0 && norm_b > 0.0 {
        dot / (norm_a * norm_b)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_produces_identical_vectors() {
        let a = embed("user prefers Rust", DEFAULT_DIM);
        let b = embed("user prefers Rust", DEFAULT_DIM);
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn similar_text_has_higher_similarity_than_unrelated() {
        let a = embed("user prefers Rust programming", DEFAULT_DIM);
        let b = embed("user likes Rust coding", DEFAULT_DIM);
        let c = embed("the weather is sunny today", DEFAULT_DIM);
        let sim_ab = cosine_similarity(&a, &b);
        let sim_ac = cosine_similarity(&a, &c);
        assert!(
            sim_ab > sim_ac,
            "similar texts should score higher: {sim_ab} vs {sim_ac}"
        );
    }

    #[test]
    fn vectors_are_normalised() {
        let v = embed("hello world", DEFAULT_DIM);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }
}
