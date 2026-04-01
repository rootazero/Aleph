//! Control Plane Module
//!
//! Provides embedded web UI for configuration management.

pub mod assets;

pub mod server;

pub use server::create_control_plane_router;
