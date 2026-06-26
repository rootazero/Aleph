//! MinHash + LSH similarity edges over note bodies. Zero-embedding, zero new
//! deps (R3). Word-level 3-shingles, K=64 MinHash, LSH banding for O(n)
//! candidate generation, exact Jaccard estimate gating. Concurrency via
//! std::thread::scope (matching relevance::all_related). Deterministic.

use std::collections::{HashMap, HashSet};

pub const K: usize = 64;
/// Scale jaccard (≤1) into the 4-signal magnitude range so similarity edges are
/// competitive with a single direct link when merged into notes_graph_related.
pub const SIMILARITY_EDGE_WEIGHT: f32 = 3.0;
const BANDS: usize = 32;
const ROWS: usize = K / BANDS; // 2

/// FNV-1a 64-bit of a string.
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Mix a base hash with a seed (splitmix64-style) for K independent hashes.
fn mix(x: u64, seed: u64) -> u64 {
    let mut z = x.wrapping_add(seed).wrapping_add(0x9e3779b97f4a7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

/// 3-word shingle hashes of a lowercased body. Whole-token set if < 3 words.
#[must_use]
pub fn shingles(body: &str) -> HashSet<u64> {
    let toks: Vec<&str> = body.split_whitespace().collect();
    let mut out = HashSet::new();
    if toks.len() < 3 {
        for t in &toks {
            out.insert(fnv1a(&t.to_lowercase()));
        }
        return out;
    }
    for w in toks.windows(3) {
        let s = format!("{} {} {}", w[0].to_lowercase(), w[1].to_lowercase(), w[2].to_lowercase());
        out.insert(fnv1a(&s));
    }
    out
}

/// K-length MinHash signature. Empty input → all u64::MAX (estimates 0 vs any).
#[must_use]
pub fn signature(shingles: &HashSet<u64>) -> [u64; K] {
    let mut sig = [u64::MAX; K];
    for &sh in shingles {
        for (k, slot) in sig.iter_mut().enumerate() {
            let h = mix(sh, k as u64);
            if h < *slot {
                *slot = h;
            }
        }
    }
    sig
}

#[must_use]
pub fn jaccard_estimate(a: &[u64; K], b: &[u64; K]) -> f32 {
    let agree = a.iter().zip(b.iter()).filter(|(x, y)| x == y).count();
    agree as f32 / K as f32
}

/// Similarity edges among docs (path, body). LSH candidate generation, exact
/// Jaccard gating ≥ threshold, ≤ cap edges per node. Deterministic output
/// (sorted). Edge score = jaccard * SIMILARITY_EDGE_WEIGHT.
#[must_use]
pub fn similarity_edges(
    docs: &[(String, String)],
    threshold: f32,
    cap: usize,
    threads: usize,
) -> Vec<(String, String, f32)> {
    let n = docs.len();
    if n < 2 {
        return Vec::new();
    }
    // Signatures in parallel (CPU-bound; std::thread::scope, no rayon).
    let threads = threads.clamp(1, n);
    let chunk = n.div_ceil(threads);
    let mut sigs: Vec<[u64; K]> = vec![[u64::MAX; K]; n];
    {
        let docs_ref = &docs;
        let slices: Vec<&mut [[u64; K]]> = sigs.chunks_mut(chunk).collect();
        std::thread::scope(|scope| {
            for (t, slice) in slices.into_iter().enumerate() {
                let start = t * chunk;
                scope.spawn(move || {
                    for (j, slot) in slice.iter_mut().enumerate() {
                        *slot = signature(&shingles(&docs_ref[start + j].1));
                    }
                });
            }
        });
    }

    // LSH: bucket by band; candidate pairs collide in ≥1 band.
    let mut buckets: HashMap<(usize, u64), Vec<usize>> = HashMap::new();
    for (i, sig) in sigs.iter().enumerate() {
        for band in 0..BANDS {
            let mut h: u64 = 0xcbf29ce484222325;
            for r in 0..ROWS {
                h ^= sig[band * ROWS + r];
                h = h.wrapping_mul(0x100000001b3);
            }
            buckets.entry((band, h)).or_default().push(i);
        }
    }
    let mut cand: HashSet<(usize, usize)> = HashSet::new();
    for members in buckets.values() {
        for a in 0..members.len() {
            for b in (a + 1)..members.len() {
                let (x, y) = (members[a], members[b]);
                cand.insert(if x < y { (x, y) } else { (y, x) });
            }
        }
    }

    // Exact-estimate gate + per-node cap.
    let mut per_node: HashMap<usize, Vec<(usize, f32)>> = HashMap::new();
    for (x, y) in cand {
        let j = jaccard_estimate(&sigs[x], &sigs[y]);
        if j >= threshold {
            per_node.entry(x).or_default().push((y, j));
            per_node.entry(y).or_default().push((x, j));
        }
    }
    let mut edges: Vec<(String, String, f32)> = Vec::new();
    let mut keys: Vec<usize> = per_node.keys().copied().collect();
    keys.sort_unstable();
    for k in keys {
        let mut peers = per_node.remove(&k).unwrap();
        peers.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        peers.truncate(cap);
        for (peer, j) in peers {
            edges.push((docs[k].0.clone(), docs[peer].0.clone(), j * SIMILARITY_EDGE_WEIGHT));
        }
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_bodies_estimate_near_one() {
        let a = signature(&shingles("the quick brown fox jumps over the lazy dog"));
        let b = signature(&shingles("the quick brown fox jumps over the lazy dog"));
        assert!((jaccard_estimate(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn disjoint_bodies_estimate_near_zero() {
        let a = signature(&shingles("alpha beta gamma delta epsilon zeta"));
        let b = signature(&shingles("one two three four five six seven"));
        assert!(jaccard_estimate(&a, &b) < 0.2);
    }

    #[test]
    fn similarity_edges_link_near_duplicates_only() {
        let docs = vec![
            ("g/a".to_string(), "rust ownership borrowing lifetimes prevent data races".to_string()),
            ("g/b".to_string(), "rust ownership borrowing lifetimes prevent data races today".to_string()),
            ("g/c".to_string(), "completely unrelated text about cooking pasta sauce".to_string()),
        ];
        let edges = similarity_edges(&docs, 0.5, 8, 1);
        assert!(edges.iter().any(|(f, t, _)| (f == "g/a" && t == "g/b") || (f == "g/b" && t == "g/a")));
        assert!(!edges.iter().any(|(f, t, _)| *f == "g/c" || *t == "g/c"));
        assert!(edges.iter().all(|(_, _, s)| *s > 0.0));
    }

    #[test]
    fn short_body_falls_back_to_token_set() {
        // <3 words: shingle set = the individual token hashes, still comparable.
        let a = shingles("hello world");
        assert!(!a.is_empty());
    }
}
