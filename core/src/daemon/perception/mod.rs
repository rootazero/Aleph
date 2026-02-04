pub mod config;

#[cfg(test)]
mod tests;

pub use config::{
    FSWatcherConfig, PerceptionConfig, ProcessWatcherConfig, SystemWatcherConfig,
    TimeWatcherConfig,
};
