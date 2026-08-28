use crate::error::Result;
use async_trait::async_trait;

/// A backend that turns a URL into clean markdown. Parallel to
/// [`crate::search::SearchProvider`]. Implementations are thin HTTP clients.
#[async_trait]
pub trait FetchProvider: Send + Sync {
    /// Fetch `url` and return extracted markdown. Errors bubble up so the
    /// registry can fall through to the next provider / built-in fetch.
    ///
    /// **SSRF contract**: callers MUST SSRF-validate `url` against the
    /// operator-configured [`crate::security::ssrf::SsrfPolicy`] BEFORE
    /// invoking `fetch`. Providers do not re-validate — they trust the
    /// caller's gate. This keeps the operator's policy authoritative and
    /// avoids redundant DNS resolutions (which would otherwise widen the
    /// DNS-rebinding TOCTOU window). The production caller is
    /// [`crate::builtin_tools::web_fetch`], which performs this check at
    /// the entry point; the gateway health-check caller uses a hardcoded
    /// `https://example.com` so it cannot leak.
    async fn fetch(&self, url: &str) -> Result<String>;

    /// Stable provider name (matches the `[fetch].backends` key).
    fn name(&self) -> &str;

    /// Whether this provider is configured enough to be used.
    fn is_available(&self) -> bool;
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
        fn is_available(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn trait_object_is_usable() {
        let p: std::sync::Arc<dyn FetchProvider> = std::sync::Arc::new(Dummy);
        assert_eq!(p.name(), "dummy");
        assert!(p.is_available());
        assert_eq!(p.fetch("http://x").await.unwrap(), "# md");
    }
}
