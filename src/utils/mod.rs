/// Utility modules shared across the codebase
pub mod atomic_io;
pub mod atomic_write;
pub mod instance_lock;
pub mod json_extract;
pub mod one_or_many;
pub mod paths;
pub mod pii;
pub mod text_format;

#[cfg(debug_assertions)]
pub mod spec_c_audit; // Removed in Task 26

pub use one_or_many::OneOrMany;
