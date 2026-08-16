//! Memory-graph support shared by `api/{graph,memory}.rs`, the memory table
//! views, and the galaxy renderer. Split out of the old `canvas_engine/` when
//! the `canvas` name was handed to the whiteboard (see views/canvas/).
//!
//! `fnv1a` lives here (not in `views/memory/galaxy/`) because
//! `category_color` — a member of this shared half — hashes category names
//! with it; the shared side must not reach into a view's private modules.
pub mod adapter;
pub mod category_color;
pub mod fnv1a;
pub mod markdown_excerpt;
