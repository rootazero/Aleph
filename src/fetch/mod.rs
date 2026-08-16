//! Fetch (URL→markdown) provider category — parallel to `crate::search`.
//!
//! - [`FetchProvider`]: capability contract (URL → markdown)
//! - [`FetchRegistry`]: active providers + selection/fallback (Task 5)
//! - `factory`: `FetchProviderFactory` / `FetchProviderFactoryRegistry` for construction (Task 4)
//! - `providers/`: crawl4ai, firecrawl (Task 6)

pub mod factory;
pub mod provider;
pub mod providers;
pub mod registry;

pub use provider::FetchProvider;
pub use registry::FetchRegistry;
