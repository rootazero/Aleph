//! Enhanced token usage tracking with cache awareness

/// Enhanced token usage tracking with cache awareness
///
/// This struct provides detailed token tracking that matches OpenCode's approach,
/// including support for reasoning tokens and cache-aware billing calculations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnhancedTokenUsage {
    /// Input tokens consumed
    pub input: u64,
    /// Output tokens generated
    pub output: u64,
    /// Reasoning tokens (for models that support it)
    pub reasoning: u64,
    /// Tokens read from cache (reduces cost)
    pub cache_read: u64,
    /// Tokens written to cache
    pub cache_write: u64,
}

impl EnhancedTokenUsage {
    /// Create a new EnhancedTokenUsage with all fields set
    pub fn new(input: u64, output: u64, reasoning: u64, cache_read: u64, cache_write: u64) -> Self {
        Self {
            input,
            output,
            reasoning,
            cache_read,
            cache_write,
        }
    }

    /// Calculate total tokens for overflow detection
    ///
    /// OpenCode formula: input + cache.read + output
    /// This represents the actual context window usage
    pub fn total_for_overflow(&self) -> u64 {
        self.input + self.cache_read + self.output
    }

    /// Calculate billable tokens (cache reads are cheaper, often excluded)
    ///
    /// Returns input + output + reasoning tokens (excludes cache reads)
    pub fn total_billable(&self) -> u64 {
        self.input + self.output + self.reasoning
    }

    /// Add another usage to this one
    pub fn add(&mut self, other: &EnhancedTokenUsage) {
        self.input += other.input;
        self.output += other.output;
        self.reasoning += other.reasoning;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    /// Check if this usage is empty (all fields are zero)
    pub fn is_empty(&self) -> bool {
        self.input == 0
            && self.output == 0
            && self.reasoning == 0
            && self.cache_read == 0
            && self.cache_write == 0
    }

    /// Calculate total tokens (all fields combined)
    pub fn total(&self) -> u64 {
        self.input + self.output + self.reasoning + self.cache_read + self.cache_write
    }
}
