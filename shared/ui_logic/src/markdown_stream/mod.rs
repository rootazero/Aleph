//! Streaming markdown boundary detection — see [`boundary`] for the
//! algorithm and its safety rationale. Shared between Panel (HTML renderer)
//! and TUI (ratatui `Line` renderer): only the "how far is it safe to
//! freeze" decision is shared, not the rendering.

mod boundary;

pub use boundary::safe_freeze_offset;
