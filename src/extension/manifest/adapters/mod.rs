//! Platform-specific manifest adapter implementations

pub mod aleph_toml;
pub mod auto_discover;
pub mod codex;
pub mod cursor;
mod dsh;
mod foreign;
pub(crate) mod pi;
pub(super) use dsh::DshAdapter;
