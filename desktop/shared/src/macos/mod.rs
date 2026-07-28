//! macOS-only implementations shared by both sides of the desktop stack.
//!
//! The counterpart to [`crate::linux`], and it exists for the same structural
//! reason. `aleph-desktop-macos` depends on `aleph-desktop`, never the other way
//! round, so any behaviour both crates need has to live here — on the side that
//! is depended upon. When it does not, it gets written twice, and the two copies
//! drift: see the module docs in [`app`] and [`clipboard`] for what that cost.
//!
//! Compiled only on macOS: every function here is `NSWorkspace` / `NSPasteboard`
//! and has no host-testable half worth keeping behind a `cfg` on other targets.

pub mod app;
pub mod clipboard;
