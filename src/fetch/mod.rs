//! Fetch (URL→markdown) provider category — parallel to `crate::search`.
//!
//! - [`FetchProvider`]: capability contract (URL → markdown)
//! - `providers/`: crawl4ai, firecrawl
//!
//! # Runtime status (BT-D-R4-22)
//!
//! These providers are **not** wired into `web_fetch`. A provider receives the
//! target URL as a string and resolves/follows it from its own network
//! position, so the SSRF DNS pin from
//! [`crate::security::ssrf::validate_url_async`] cannot be enforced on the
//! fetch that actually happens — neither provider API accepts a pre-resolved
//! address, and validate-then-delegate reopens the DNS-rebinding /
//! redirect-to-internal gap the SSRF gate exists to close. The registry and
//! factory that used to assemble a provider chain for `WebFetchTool` were
//! removed with the wiring.
//!
//! What remains is the connection-test surface: `fetch_config.test`
//! (`gateway/handlers/fetch_config.rs::handle_test`) builds a provider
//! directly and fetches a hardcoded `https://example.com` so operators can
//! validate credentials and reachability from the Panel. If provider
//! delegation is ever revived, the provider API must first learn to honor a
//! caller-supplied resolution/pin (or enforce an equivalent SSRF policy
//! itself) — see the `FetchProvider::fetch` contract.

pub mod provider;
pub mod providers;

pub use provider::FetchProvider;
