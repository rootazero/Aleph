//! Compatibility shim. The launch mechanics moved to [`crate::browser::engine`]
//! when the second engine arrived:
//!
//! * process records, the orphan sweep and the launch contract →
//!   `engine::process`
//! * the Chromium argv, the `DevToolsActivePort` file and `ChromiumChild` →
//!   `engine::chromium`
//!
//! Kept for exactly one stage so the move and the rewiring are separate
//! commits: `playwright_cli.rs`, `playwright_launch.rs` and `manager.rs` still
//! name this path, and `manager.rs`'s source census
//! (`the_boot_hook_still_calls_the_orphan_sweep`, `manager.rs:1030`) asserts
//! the string `chromium_launch::reap_orphans_now`. **Task 14 deletes this file
//! and updates that census in the same commit** — deleting it without the
//! census edit turns a green test into one that reads its own absence.

pub(crate) use super::engine::chromium::*;
pub(crate) use super::engine::process::*;
