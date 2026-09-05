/// Utility modules shared across the codebase
pub mod atomic_io;
pub mod atomic_write;
pub(crate) mod fifo_cache;
pub(crate) mod filename;
pub mod host;
pub mod instance_lock;
pub mod json_extract;
pub mod no_window;
pub(crate) mod panic_payload;
pub mod path_within;
pub mod paths;
pub mod process_alive;
pub mod reqwest_limit;
#[cfg(any(test, feature = "test-helpers"))]
pub mod scratch;
pub mod shell;
pub mod source_scan;
pub mod sqlite_open;
pub mod text_format;
pub mod vault_io;
