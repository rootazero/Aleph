pub mod config;
pub mod registry;
pub mod watcher;
pub mod watchers;

#[cfg(test)]
mod tests;

pub use config::{
    FSWatcherConfig, PerceptionConfig, ProcessWatcherConfig, SystemWatcherConfig,
    TimeWatcherConfig,
};
pub use registry::WatcherRegistry;
pub use watcher::{Watcher, WatcherControl, WatcherHealth};
pub use watchers::*;
