//! Transcript presentation logic shared by the Panel (Leptos) and the TUI
//! (ratatui). Everything here is `data → data`: no rendering, no signals, no
//! terminal. Each surface paints what these functions decide, so the two
//! can never disagree about a fold, a summary, a hunk or a colour role.
//!
//! Compiles under `default-features = false` (the TUI build) — nothing in
//! this module may be feature-gated or reach for leptos / web-sys.

mod theme_tokens;

pub use theme_tokens::{
    mix_rgb, SemanticColor, ALL_SEMANTIC_COLORS, DIFF_EMPHASIS_MIX, DIFF_ROW_MIX,
};

mod fold;
pub use fold::{
    fold, wrap_physical, FoldAnchor, FoldBody, FoldPolicy, Folded, DEFAULT_COLLAPSED_ROWS,
    FALLBACK_WIDTH,
};

mod summarize;
pub use summarize::{
    clip_one_line, display_name, humanize, summarize, CallSummary, ARGS_CLIP, DISPLAY_NAMES,
    PREFERRED_ARG_KEYS,
};

mod group;
mod view_model;
pub use group::{group_entries, MAX_GAP_TEXT, MIN_GROUP};
pub use view_model::{
    RowBody, RowStatus, ToolGroup, ToolRow, TranscriptEntry, TurnSummaryEntry,
    READ_ONLY_DISPLAY_NAMES,
};

mod diff_view;
pub use diff_view::{
    diff_rows, stats_label, word_spans, DiffRow, DiffRows, DiffView, Span, COLLAPSED_DIFF_ROWS,
    EXPANDED_DIFF_ROWS, LCS_CELL_BUDGET_COLLAPSED, LCS_CELL_BUDGET_EXPANDED,
    MAX_INLINE_LINE_CHARS,
};

mod md_enhance;
pub use md_enhance::{
    enhance, find_path_refs, linkify_bare_urls, trim_url, AdmonitionKind, Block, Enhanced,
    PathRef, KNOWN_EXTENSIONS, MERMAID_PLACEHOLDER_PREFIX,
};

mod affordance;
mod context;
mod turn_summary;
pub use affordance::{
    expand_hint, fmt_duration_ms, spinner_frame, verb, worked_for, Locale, Modality,
    SPINNER_FRAMES, SPINNER_PERIOD_MS, VERBS_EN, VERBS_ZH, VERB_REROLL_MS,
};
pub use context::{reconcile, ContextRow, ContextRows, PROVIDER_TOLERANCE};
pub use turn_summary::{summarize_turn, turn_summary_text, MIN_TOOLS_FOR_SUMMARY};
