//! Spec B — filter for restricting HybridAssembler results by `FactSource`.
//!
//! Default behaviour (`Any`) is byte-for-byte identical to pre-Spec-B
//! assembler output. `Only(_)` and `Excluding(_)` are non-default values
//! used by `session_search` (and only `session_search`) to physically
//! separate session summaries from wiki/note retrieval.

use crate::memory::context::FactSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactSourceFilter {
    Any,
    Only(FactSource),
    Excluding(FactSource),
}

impl Default for FactSourceFilter {
    fn default() -> Self {
        Self::Any
    }
}

impl FactSourceFilter {
    /// Predicate evaluated row-by-row when filtering candidate facts.
    pub fn matches(&self, source: FactSource) -> bool {
        match self {
            Self::Any => true,
            Self::Only(want) => *want == source,
            Self::Excluding(skip) => *skip != source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_matches_everything() {
        let f = FactSourceFilter::Any;
        assert!(f.matches(FactSource::SessionCompressed));
        assert!(f.matches(FactSource::Extracted));
    }

    #[test]
    fn only_matches_target_only() {
        let f = FactSourceFilter::Only(FactSource::SessionCompressed);
        assert!(f.matches(FactSource::SessionCompressed));
        assert!(!f.matches(FactSource::Extracted));
    }

    #[test]
    fn excluding_skips_target_only() {
        let f = FactSourceFilter::Excluding(FactSource::SessionCompressed);
        assert!(!f.matches(FactSource::SessionCompressed));
        assert!(f.matches(FactSource::Extracted));
    }

    #[test]
    fn default_is_any() {
        assert_eq!(FactSourceFilter::default(), FactSourceFilter::Any);
    }
}
