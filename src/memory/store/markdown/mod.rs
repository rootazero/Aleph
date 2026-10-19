//! Markdown-based session storage implementation.
//!
//! Stores sessions as Markdown files with content-addressed attachments.
//! SQLite is used only for metadata, indexes, and search.

mod store_impl;
mod utils;

pub use store_impl::MarkdownSessionStore;
