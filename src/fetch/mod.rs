//! Fetch (URL→markdown) provider category — parallel to `crate::search`.
//!
//! - [`FetchProvider`]: capability contract (URL → markdown)
//! - [`FetchRegistry`]: active providers + selection/fallback
//! - `factory`: `FetchProviderFactory` / `FetchProviderFactoryRegistry` for construction
//! - `providers/`: concrete `Crawl4aiFetchProvider` + `FirecrawlFetchProvider`

pub mod factory;
pub mod provider;
pub mod providers;
pub mod registry;

pub use provider::FetchProvider;
pub use registry::FetchRegistry;
