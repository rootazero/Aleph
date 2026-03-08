//! DefaultsResolver for smart parameter resolution.
//!
//! Resolves default parameters. Currently returns defaults directly;
//! preset matching was removed as part of the language-agnostic redesign.

use super::types::{ParameterSource, TaskParameters};

/// Resolves default parameters for tasks.
pub struct DefaultsResolver;

impl DefaultsResolver {
    /// Create a new defaults resolver.
    pub fn new() -> Self {
        Self
    }

    /// Resolve parameters (currently returns inference defaults).
    pub fn resolve(&self) -> TaskParameters {
        TaskParameters::default().with_source(ParameterSource::Inference)
    }
}

impl Default for DefaultsResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults_resolver() {
        let resolver = DefaultsResolver::new();
        let params = resolver.resolve();
        assert_eq!(params.source, ParameterSource::Inference);
    }
}
