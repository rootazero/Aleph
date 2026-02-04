pub mod config;
pub mod registry;
pub mod watcher;

#[cfg(test)]
mod tests;

pub use config::{
    FSWatcherConfig, PerceptionConfig, ProcessWatcherConfig, SystemWatcherConfig,
    TimeWatcherConfig,
};
pub use registry::WatcherRegistry;
pub use watcher::{Watcher, WatcherControl, WatcherHealth};
