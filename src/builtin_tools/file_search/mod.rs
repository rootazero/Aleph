//! Repository-aware search: the `grep` and `find` tools.
//!
//! # The gap these close
//!
//! `file_ops` answers questions about *a named path*; this module answers
//! questions about *a tree*. Until it existed the model had no builtin that
//! could search file contents at all, so every "where is X defined" became a
//! `bash` call — and a `bash` grep is a context bomb: it does not read
//! `.gitignore`, so one recursive run pours every hit under `node_modules/`,
//! `target/` and `dist/` into the window, unbounded and unpageable.
//!
//! # Shape
//!
//! - [`walk`] — the one answer to "which files does this repository consider
//!   its own", plus the denylist floor a byte-reading face has to bind.
//! - [`scan`] — pure line matching and rendering, testable against strings.
//! - [`notes`] — the clauses that name what a result withheld, written once so
//!   the two tools cannot spell the same omission two ways.
//! - [`grep`] / [`find`] — the two tools, which differ only in what they do
//!   with the file list [`walk`] hands them.
//!
//! # Deliberately not a third tool
//!
//! There is no `multi_grep`. `pattern` is a regex, so several terms are one
//! call (`foo|bar|baz`) and the tool's description says so outright; a second
//! verb would buy per-pattern grouping at the price of a second registration
//! surface, ~700 B of description billed on every request, and one action
//! answering to two names.

mod find;
mod grep;
mod notes;
mod scan;
mod walk;

pub use find::{FindArgs, FindOutput, FindTool};
pub use grep::{GrepArgs, GrepOutput, GrepTool};
