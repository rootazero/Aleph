//! Wide layout — desktop app + browser (≥ wide breakpoint).
//!
//! Hosts the full canonical screen set. This is the existing panel UI,
//! unchanged: the form-factor router falls back here for any non-phone /
//! non-tablet viewport. The crate-root re-export `pub use
//! platform::wide::views` (in `lib.rs`) keeps every existing
//! `crate::views::…` path resolving after the physical move.

pub mod views;
