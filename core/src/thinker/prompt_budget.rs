//! Token budget management for system prompt assembly.
//!
//! Prevents system prompt bloat by enforcing character limits
//! and providing truncation statistics.

use super::prompt_mode::PromptMode;

/// Budget configuration for system prompt assembly.
#[derive(Debug, Clone)]
pub struct TokenBudget {
    /// Maximum total characters for assembled system prompt.
    /// Default: 80_000 (~20K tokens).
    pub max_total_chars: usize,
    /// Bootstrap section total budget.
    /// Default: 100_000.
    pub max_bootstrap_chars: usize,
    /// Per-bootstrap-file character limit.
    /// Default: 20_000.
    pub max_per_file_chars: usize,
    /// Warning mode for truncation events.
    pub truncation_warning: TruncationWarning,
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self {
            max_total_chars: 80_000,
            max_bootstrap_chars: 100_000,
            max_per_file_chars: 20_000,
            truncation_warning: TruncationWarning::default(),
        }
    }
}

/// Warning mode for truncation events.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TruncationWarning {
    /// Never warn.
    Off,
    /// Warn once per session per unique truncation state.
    #[default]
    Once,
    /// Warn every time.
    Always,
}

/// Result of prompt assembly with truncation metadata.
#[derive(Debug, Clone)]
pub struct PromptResult {
    /// The assembled system prompt string.
    pub prompt: String,
    /// Truncation statistics (empty if nothing was truncated).
    pub truncation_stats: Vec<TruncationStat>,
    /// Which mode was used.
    pub mode: PromptMode,
}

/// Per-section truncation statistics.
#[derive(Debug, Clone)]
pub struct TruncationStat {
    /// Layer name that was truncated or removed.
    pub layer_name: String,
    /// Original character count before truncation.
    pub original_chars: usize,
    /// Final character count (0 if fully removed).
    pub final_chars: usize,
    /// Whether the section was fully removed.
    pub fully_removed: bool,
}

/// Truncate content preserving head and tail, UTF-8 safe.
///
/// Keeps `head_ratio` of chars from the start and `tail_ratio` from the end,
/// inserting a truncation marker in between.
pub fn truncate_with_head_tail(
    content: &str,
    max_chars: usize,
    head_ratio: f64,
    tail_ratio: f64,
) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }

    let marker_template = "\n\n[... truncated ...]\n\n";
    let marker_overhead = marker_template.len() + 10; // extra for digit count

    // If max_chars is too small for head+tail+marker, just take head
    if max_chars <= marker_overhead {
        let end = content
            .char_indices()
            .take_while(|(i, _)| *i < max_chars)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        return content[..end].to_string();
    }

    let usable = max_chars - marker_overhead;
    let head_chars = (usable as f64 * head_ratio / (head_ratio + tail_ratio)) as usize;
    let tail_chars = usable.saturating_sub(head_chars);

    let truncated_count = content
        .len()
        .saturating_sub(head_chars)
        .saturating_sub(tail_chars);
    let marker = format!("\n\n[... {} chars truncated ...]\n\n", truncated_count);

    // UTF-8 safe boundary finding
    let head_end = content
        .char_indices()
        .take_while(|(i, _)| *i < head_chars)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);

    let tail_start = content
        .char_indices()
        .rev()
        .take_while(|(i, _)| content.len() - *i <= tail_chars)
        .last()
        .map(|(i, _)| i)
        .unwrap_or(content.len());

    let result = format!("{}{}{}", &content[..head_end], marker, &content[tail_start..]);

    // Final safety check — if still over budget, hard truncate
    if result.len() > max_chars {
        let end = result
            .char_indices()
            .take_while(|(i, _)| *i < max_chars)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        return result[..end].to_string();
    }

    result
}

/// Enforce total budget by removing sections from lowest priority.
///
/// Returns (trimmed prompt, truncation stats).
/// Sections with priority in `protected_priorities` are never removed.
pub fn enforce_budget(
    sections: &[(u32, &str, &str)], // (priority, layer_name, content)
    max_total: usize,
    protected_priorities: &[u32],
) -> (String, Vec<TruncationStat>) {
    let total: usize = sections.iter().map(|(_, _, c)| c.len()).sum();
    if total <= max_total {
        let prompt = sections
            .iter()
            .map(|(_, _, c)| *c)
            .collect::<Vec<_>>()
            .join("");
        return (prompt, vec![]);
    }

    let mut stats = Vec::new();
    let mut excess = total - max_total;

    // Sort by priority descending (lowest priority = highest number = removed first)
    let mut removal_order: Vec<usize> = (0..sections.len()).collect();
    removal_order.sort_by(|a, b| sections[*b].0.cmp(&sections[*a].0));

    let mut included = vec![true; sections.len()];

    for idx in removal_order {
        if excess == 0 {
            break;
        }
        let (priority, name, content) = &sections[idx];
        if protected_priorities.contains(priority) {
            continue;
        }
        let saved = content.len();
        included[idx] = false;
        stats.push(TruncationStat {
            layer_name: name.to_string(),
            original_chars: saved,
            final_chars: 0,
            fully_removed: true,
        });
        excess = excess.saturating_sub(saved);
    }

    let prompt = sections
        .iter()
        .enumerate()
        .filter(|(i, _)| included[*i])
        .map(|(_, (_, _, c))| *c)
        .collect::<Vec<_>>()
        .join("");

    (prompt, stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_budget_values() {
        let b = TokenBudget::default();
        assert_eq!(b.max_total_chars, 80_000);
        assert_eq!(b.max_bootstrap_chars, 100_000);
        assert_eq!(b.max_per_file_chars, 20_000);
        assert_eq!(b.truncation_warning, TruncationWarning::Once);
    }

    #[test]
    fn truncate_short_content_unchanged() {
        let content = "Hello, world!";
        let result = truncate_with_head_tail(content, 100, 0.7, 0.2);
        assert_eq!(result, content);
    }

    #[test]
    fn truncate_long_content_preserves_head_tail() {
        let content = "A".repeat(1000);
        let result = truncate_with_head_tail(&content, 100, 0.7, 0.2);
        assert!(result.len() < 1000);
        assert!(result.contains("[..."));
        assert!(result.contains("truncated ...]"));
        assert!(result.starts_with("AAAA"));
        assert!(result.ends_with("AAAA"));
    }

    #[test]
    fn truncate_multibyte_utf8_safe() {
        let content = "你好世界".repeat(100);
        let result = truncate_with_head_tail(&content, 50, 0.7, 0.2);
        assert!(result.contains("[..."));
        // Should not panic
    }

    #[test]
    fn enforce_budget_under_limit_no_stats() {
        let sections = vec![
            (100u32, "role", "You are an AI."),
            (500, "tools", "Available tools: none"),
        ];
        let (prompt, stats) = enforce_budget(&sections, 1000, &[]);
        assert!(stats.is_empty());
        assert!(prompt.contains("You are an AI."));
        assert!(prompt.contains("Available tools"));
    }

    #[test]
    fn enforce_budget_removes_lowest_priority_first() {
        let long_a = "A".repeat(30);
        let long_b = "B".repeat(30);
        let long_c = "C".repeat(30);
        let long_d = "D".repeat(30);
        let sections = vec![
            (100u32, "role", long_a.as_str()),
            (500, "tools", long_b.as_str()),
            (1600, "language", long_c.as_str()),
            (1500, "custom", long_d.as_str()),
        ];
        // Total = 120 chars, limit to 70 — must remove ~50
        let (prompt, stats) = enforce_budget(&sections, 70, &[100, 500]);
        assert!(!stats.is_empty());
        assert!(prompt.contains(&long_a));
        assert!(prompt.contains(&long_b));
        let removed: Vec<_> = stats.iter().map(|s| s.layer_name.as_str()).collect();
        assert!(removed.contains(&"language"));
    }

    #[test]
    fn enforce_budget_protects_layers() {
        let long = "A".repeat(100);
        let sections = vec![
            (100u32, "role", long.as_str()),
            (500, "tools", long.as_str()),
        ];
        // Both protected — nothing can be removed
        let (_, stats) = enforce_budget(&sections, 50, &[100, 500]);
        assert!(stats.is_empty());
    }
}
