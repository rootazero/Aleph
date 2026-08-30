pub mod base;
pub mod bing;
pub mod brave;
pub mod duckduckgo;
pub mod exa;
pub mod firecrawl;
pub mod google;
pub mod jina;
pub mod searxng;
pub mod tavily;

// Provider re-exports
pub use bing::BingProvider;
pub use brave::BraveProvider;
pub use duckduckgo::DuckDuckGoProvider;
pub use exa::ExaProvider;
pub use firecrawl::FirecrawlProvider;
pub use google::GoogleProvider;
pub use jina::JinaProvider;
pub use searxng::SearxngProvider;
pub use tavily::TavilyProvider;

// Factory re-exports — keep in sync with ProviderFactoryRegistry::with_defaults
pub use bing::BingFactory;
pub use brave::BraveFactory;
pub use duckduckgo::DuckDuckGoFactory;
pub use exa::ExaFactory;
pub use firecrawl::FirecrawlFactory;
pub use google::GoogleFactory;
pub use jina::JinaFactory;
pub use searxng::SearxngFactory;
pub use tavily::TavilyFactory;

#[cfg(test)]
mod capability_census;
