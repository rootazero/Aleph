//! Bounded regex compilation for untrusted (config- or user-supplied) patterns.
//!
//! Rust's `regex` crate guarantees linear-time matching with no catastrophic
//! backtracking, so it is **immune to classic `ReDoS`** by construction. The
//! residual risk from an untrusted pattern is therefore not match-time blowup
//! but *compile-time memory* blowup: a tiny source such as `(a{1000}){1000}`
//! can expand into an enormous compiled automaton.
//!
//! [`bounded_builder`] caps the size of the compiled program (and its lazy DFA
//! cache). `RegexBuilder::build()` returns the same
//! `Result<Regex, regex::Error>` as `Regex::new`, so this is a drop-in
//! replacement at every untrusted compile site: pathological patterns fail with
//! a normal `regex::Error` ("Compiled regex exceeds size limit ...") instead of
//! exhausting memory.
//!
//! Reserve this for patterns that arrive from config or user input (secret leak
//! patterns, custom PII rules, shell risk patterns, hook matchers). Hardcoded,
//! in-crate patterns are trusted and need no bound. Mirrors the limit already
//! applied to routing rules in `crate::routing::rules`.

use regex::RegexBuilder;

/// Maximum size, in bytes, of a compiled regex program built from an untrusted
/// pattern. The `regex` crate default is 10 MiB; 1 MiB is ample for operator
/// patterns while rejecting expansion bombs. Matches `routing::rules`.
pub const MAX_COMPILED_SIZE: usize = 1 << 20;

/// A [`RegexBuilder`] pre-configured with conservative size limits, suitable for
/// compiling untrusted patterns. Callers finish with `.build()` (and may set
/// extra options such as `.case_insensitive(true)` beforehand).
#[must_use]
pub fn bounded_builder(pattern: &str) -> RegexBuilder {
    let mut builder = RegexBuilder::new(pattern);
    builder
        .size_limit(MAX_COMPILED_SIZE)
        .dfa_size_limit(MAX_COMPILED_SIZE);
    builder
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_ordinary_pattern() {
        assert!(bounded_builder(r"sk-[A-Za-z0-9]{20}").build().is_ok());
    }

    #[test]
    fn rejects_expansion_bomb() {
        // Tiny source, astronomically large compiled program — must be rejected
        // rather than allocated.
        let bomb = "(a{1000}){1000}{1000}";
        assert!(bounded_builder(bomb).build().is_err());
    }

    #[test]
    fn invalid_syntax_still_errors() {
        assert!(bounded_builder("(unclosed").build().is_err());
    }
}
