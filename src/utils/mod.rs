/// Utility modules shared across the codebase
pub mod atomic_io;
pub mod atomic_write;
pub(crate) mod fifo_cache;
pub mod instance_lock;
pub mod json_extract;
pub mod no_window;
pub mod one_or_many;
pub mod path_within;
pub mod paths;
pub mod process_alive;
pub mod sqlite_open;
pub mod text_format;
pub mod vault_io;

pub use one_or_many::OneOrMany;
