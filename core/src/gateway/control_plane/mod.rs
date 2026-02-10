//! Control Plane Module
//!
//! Provides embedded web UI for configuration management.

#[cfg(feature = "control-plane")]
pub mod assets;

#[cfg(feature = "control-plane")]
pub mod server;

#[cfg(feature = "control-plane")]
pub use server::create_control_plane_router;
