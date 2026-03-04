/// Prompt rendering mode — controls which layers participate in assembly.
///
/// Orthogonal to `AssemblyPath`: path selects *which variant* (Basic/Soul/Context),
/// mode selects *how verbose* (Full/Compact/Minimal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PromptMode {
    /// All layers included (primary agent).
    #[default]
    Full,
    /// Essential layers only (sub-agent, saves ~60% tokens).
    Compact,
    /// Identity + tools + response format only (ultra-lightweight).
    Minimal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_full() {
        assert_eq!(PromptMode::default(), PromptMode::Full);
    }

    #[test]
    fn modes_are_distinct() {
        assert_ne!(PromptMode::Full, PromptMode::Compact);
        assert_ne!(PromptMode::Compact, PromptMode::Minimal);
        assert_ne!(PromptMode::Full, PromptMode::Minimal);
    }
}
