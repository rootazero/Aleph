//! Platform-specific sandbox implementations

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub use macos::MacOSSandbox;
