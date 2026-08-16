//! Whiteboard canvas core — file-backed document store with per-canvas write
//! locks and optimistic-concurrency `revision` checks.
//!
//! Layering: wire types live in `aleph_protocol::canvas` (the single contract
//! shared by the gateway handlers, the Panel and the `canvas` builtin tool —
//! NOT `json_canvas_io`, which is Obsidian interchange). This module owns
//! persistence (`<data_dir>/canvas/<id>/doc.json`), the lock discipline
//! (`doc_io`, the `MetaGuard` module-boundary pattern), op application and
//! validation. RPC handlers and the tool face both consume [`CanvasStore`];
//! neither reaches the disk directly.

mod assets;
mod doc_io;
pub mod selection;
mod store;
mod validate;

pub use store::{CanvasError, CanvasStore};
