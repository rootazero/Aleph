//! Tiny FNV-1a 32-bit hash for deterministic node-position jitter.
//! Standard reference: <http://www.isthe.com/chongo/tech/comp/fnv>/.

const FNV_OFFSET: u32 = 2_166_136_261;
const FNV_PRIME: u32 = 16_777_619;

/// 32-bit FNV-1a hash of `bytes`. Identical on every machine and every run.
pub(crate) fn fnv1a_32(bytes: &[u8]) -> u32 {
    let mut h = FNV_OFFSET;
    for b in bytes {
        h ^= *b as u32;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// Normalised jitter in `[-1.0, 1.0]` keyed by an arbitrary string id.
/// Used as the deterministic substitute for `random()` in layout code.
pub(crate) fn hash_jitter(id: &str) -> f32 {
    // 1024-bucket so adjacent ids do NOT cluster
    let h = (fnv1a_32(id.as_bytes()) % 1024) as i32 - 512;
    h as f32 / 512.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_matches_canonical_vectors() {
        // Reference values from http://www.isthe.com/chongo/tech/comp/fnv/
        assert_eq!(fnv1a_32(b""), 0x811c9dc5);
        assert_eq!(fnv1a_32(b"a"), 0xe40c292c);
        assert_eq!(fnv1a_32(b"foobar"), 0xbf9cf968);
    }

    #[test]
    fn hash_jitter_in_range() {
        for id in [
            "",
            "a",
            "foo",
            "very-long-identifier-that-keeps-going-and-going",
        ] {
            let j = hash_jitter(id);
            assert!((-1.0..=1.0).contains(&j), "out of range: {}", j);
        }
    }

    #[test]
    fn hash_jitter_is_deterministic() {
        assert_eq!(hash_jitter("node-x"), hash_jitter("node-x"));
    }

    #[test]
    fn hash_jitter_varies_with_id() {
        let a = hash_jitter("node-a");
        let b = hash_jitter("node-b");
        assert_ne!(a, b);
    }
}
