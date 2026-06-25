//! Per-form-factor UI layers.
//!
//! Each platform owns its own screens / navigation / layout. Shared data
//! (`api`, `state`, `context`), design tokens, and leaf components live at the
//! crate root and are reused by every platform — only the presentation layer
//! diverges per device. This keeps iPhone/iPad work physically isolated from
//! the desktop/browser UI: code in `phone`/`tablet` cannot reach into `wide`.
//!
//! - [`wide`]   — desktop app + browser (one wide UI; only the shell differs)
//! - [`phone`]  — iPhone, iOS-native, panel-only (remote core)
//! - [`tablet`] — iPad (future), panel-only

pub mod phone;
pub mod tablet;
pub mod wide;
