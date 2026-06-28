//! Fetch (URL→markdown) provider category — parallel to `crate::search`.
//!
//! - [`FetchProvider`]: capability contract (URL → markdown)
//! - [`FetchProviderFactory`] / [`FetchProviderFactoryRegistry`]: construction (Task 4)
//! - [`FetchRegistry`]: active providers + selection/fallback (Task 5)
//! - `providers/`: crawl4ai, firecrawl (Task 6)

pub mod factory;
pub mod provider;
pub mod providers;
pub mod registry;

pub use factory::{FetchProviderFactory, FetchProviderFactoryRegistry};
pub use provider::FetchProvider;
pub use registry::FetchRegistry;
