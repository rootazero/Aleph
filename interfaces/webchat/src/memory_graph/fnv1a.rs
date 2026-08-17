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
}
