use crate::error::Result;
use async_trait::async_trait;

/// A backend that turns a URL into clean markdown. Parallel to
/// [`crate::search::SearchProvider`]. Implementations are thin HTTP clients.
#[async_trait]
pub trait FetchProvider: Send + Sync {
    /// Fetch `url` and return extracted markdown. Errors bubble up so the
    /// caller can fall through to another backend / the built-in fetch.
    ///
    /// **SSRF contract**: callers MUST SSRF-validate `url` against the
    /// operator-configured [`crate::security::ssrf::SsrfPolicy`] BEFORE
    /// invoking `fetch`. Providers do not re-validate — they trust the
    /// caller's gate. Note that caller-side validation cannot pin the DNS
    /// resolution a provider performs on its own network; that gap
    /// (BT-D-R4-22) is exactly why the agent-facing `web_fetch` path no
    /// longer routes through providers at all.
    ///
    /// **Production caller**: the user-invoked connection-test RPC
    /// `gateway/handlers/fetch_config.rs::handle_test`, which passes a
    /// hardcoded `https://example.com` and cannot leak. `WebFetchTool`
    /// carries no provider wiring anymore; reviving it requires a provider
    /// API that honors a caller-supplied DNS pin (or enforces an equivalent
    /// SSRF policy server-side) before arbitrary-URL delegation is safe.
    async fn fetch(&self, url: &str) -> Result<String>;

    /// Stable provider name (matches the `[fetch].backends` key).
    fn name(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct Dummy;
    #[async_trait]
    impl FetchProvider for Dummy {
        async fn fetch(&self, _url: &str) -> crate::error::Result<String> {
            Ok("# md".into())
        }
        fn name(&self) -> &str {
            "dummy"
        }
    }

    #[tokio::test]
    async fn trait_object_is_usable() {
        let p: std::sync::Arc<dyn FetchProvider> = std::sync::Arc::new(Dummy);
        assert_eq!(p.name(), "dummy");
        assert_eq!(p.fetch("http://x").await.unwrap(), "# md");
    }
}
