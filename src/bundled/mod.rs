//! Bundled official skills and plugins, embedded at compile time.
//!
//! On startup, these are extracted to `~/.aleph/` if the bundled version
//! is newer than what's already installed.

mod extractor;
pub mod manifest;

use include_dir::{include_dir, Dir};

/// Official skills directory tree, embedded at compile time.
pub static BUNDLED_SKILLS: Dir = include_dir!("$CARGO_MANIFEST_DIR/skills");

/// Official plugins (marketplace), embedded at compile time.
pub static BUNDLED_PLUGINS: Dir = include_dir!("$CARGO_MANIFEST_DIR/plugins");

/// Version of the bundled content, tied to the server release.
pub const BUNDLED_VERSION: &str = env!("ALEPH_VERSION");

pub use extractor::extract_bundled_content;
