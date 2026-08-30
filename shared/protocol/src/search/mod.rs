//! Search backend identity, shared by the core, the Panel and the CLI.

mod providers;

pub use providers::{preset, SearchProviderPreset, CONFIGURABLE_SEARCH_PROVIDERS};
