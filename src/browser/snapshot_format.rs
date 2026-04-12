//! Text formatter for browser snapshots.
//!
//! Accepts the text-first [`SnapshotOutput`] returned by the new BrowserBackend
//! and applies truncation and ref-counting. The old AriaSnapshot-based tree
//! formatter has been replaced as part of the text-first refactor.

use super::types::SnapshotOutput;

/// Options controlling how the snapshot is formatted.
#[derive(Debug, Clone)]
pub struct SnapshotFormatOptions {
    /// Only include interactive elements (buttons, inputs, links, etc.).
    /// Currently a hint only — filtering happens inside the backend (playwright-cli).
    pub interactive_only: bool,
    /// Skip unnamed structural/container elements (default: true).
    pub compact: bool,
    /// Maximum output characters before truncation.
    pub max_chars: Option<usize>,
    /// Maximum tree depth to include (hint, not yet enforced for text snapshots).
    pub max_depth: Option<usize>,
}

impl Default for SnapshotFormatOptions {
    fn default() -> Self {
        Self {
            interactive_only: false,
            compact: true,
            max_chars: Some(30_000),
            max_depth: None,
        }
    }
}

/// Result of formatting a snapshot.
pub struct SnapshotFormatResult {
    /// The formatted snapshot text (possibly truncated).
    pub text: String,
    /// Whether the output was truncated due to max_chars.
    pub truncated: bool,
    /// Number of ref= tokens found in the snapshot text.
    pub ref_count: usize,
}

/// Format a [`SnapshotOutput`] for LLM consumption.
///
/// Applies character truncation and counts ref tokens.
pub fn format_snapshot(snapshot: &SnapshotOutput, opts: &SnapshotFormatOptions) -> SnapshotFormatResult {
    let raw = &snapshot.snapshot_text;

    // Count ref= occurrences as a proxy for interactive element count.
    let ref_count = raw.matches("[ref=").count();

    let (text, truncated) = if let Some(max) = opts.max_chars {
        if raw.len() > max {
            // Truncate at a char boundary.
            let truncated_text = raw
                .char_indices()
                .take_while(|(i, _)| *i < max)
                .last()
                .map(|(i, c)| &raw[..i + c.len_utf8()])
                .unwrap_or("")
                .to_string();
            (truncated_text, true)
        } else {
            (raw.clone(), false)
        }
    } else {
        (raw.clone(), false)
    };

    SnapshotFormatResult {
        text,
        truncated,
        ref_count,
    }
}
