//! Bundled official skills and plugins, embedded at compile time.
//!
//! On startup, these are extracted to `~/.aleph/` if the bundled version
//! differs from what's already installed.

mod extractor;
pub mod manifest;
mod sync;

use include_dir::{include_dir, Dir};

/// Official skills directory tree, embedded at compile time via `include_dir!`.
///
/// At build time, the contents of `skills/` are read and embedded into the binary.
/// On startup, `extract_bundled_content()` extracts these to `~/.aleph/skills/`.
pub static BUNDLED_SKILLS: Dir = include_dir!("$CARGO_MANIFEST_DIR/skills");

/// Official plugins (marketplace), embedded at compile time via `include_dir!`.
///
/// At build time, the contents of `plugins/` are read and embedded into the binary.
/// On startup, `extract_bundled_content()` extracts these to `~/.aleph/plugins/cache/aleph-official/`.
pub static BUNDLED_PLUGINS: Dir = include_dir!("$CARGO_MANIFEST_DIR/plugins");

/// Version of the bundled content, tied to the server release.
///
/// Set at build time from the `ALEPH_VERSION` env var (see build.rs).
/// This version is compared against `~/.aleph/skills/manifest.json` to determine
/// whether re-extraction is needed on startup.
pub const BUNDLED_VERSION: &str = env!("ALEPH_VERSION");

/// Upstream repos for the official content (runtime clone source; the embedded
/// snapshot above is the offline fallback, sourced from the same repos via
/// git submodules at build time).
pub const OFFICIAL_SKILLS_REPO: &str = "https://github.com/rootazero/Aleph-skills";
pub const OFFICIAL_PLUGINS_REPO: &str = "https://github.com/rootazero/Aleph-plugins";

pub(crate) use extractor::copy_skill_leaf;
pub(crate) use sync::{clone_or_update, clone_or_update_at};

pub use extractor::{extract_bundled_content, sync_official_now, SyncKind, SyncReport};
