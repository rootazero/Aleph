//! URL fetch cache for the web fetch tool.

use super::types::{ExtractMode, WebFetchResult};
use crate::sync_primitives::Mutex;
use lru::LruCache;
use once_cell::sync::Lazy;
use regex::Regex;
use std::borrow::Cow;
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// URL fetch cache (inspired by claude-code's WebFetchTool LRU)
// ---------------------------------------------------------------------------
//
// Aleph-server is a long-running daemon; the same URL is frequently re-asked
// across a single agent loop (e.g. an LLM that re-reads a doc page in 3
// different sub-steps). Caching the parsed result avoids hammering the
// upstream + paying repeat extract cost. Sized by entry count (not bytes)
// for simplicity — each entry's body is already capped at ~10 KB after
// markdown extraction, so 256 entries is < 3 MB worst case.
//
// Key is (canonical-URL, extract_mode) because the same URL fetched as
// Markdown vs Text yields different content.
//
// Invalidation is purely TTL-based (15 min); we don't honour HTTP
// Cache-Control because most LLM-driven re-fetches are within seconds of
// each other and a 15-min ceiling is the right blast radius.

const CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const CACHE_CAPACITY: usize = 256;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct CacheKey {
    /// Canonical URL: lowercased scheme+host, default port stripped,
    /// fragment removed. Path and query preserved verbatim.
    pub(crate) url: String,
    pub(crate) extract_mode: ExtractModeKey,
}

/// Discriminant-only copy of `ExtractMode` so it can be cheaply used as
/// part of the cache key without coupling to its serde-aware definition.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum ExtractModeKey {
    Markdown,
    Text,
}

impl From<&ExtractMode> for ExtractModeKey {
    fn from(m: &ExtractMode) -> Self {
        match m {
            ExtractMode::Markdown => Self::Markdown,
            ExtractMode::Text => Self::Text,
        }
    }
}

pub(crate) struct CacheEntry {
    pub(crate) result: WebFetchResult,
    pub(crate) inserted_at: Instant,
}

static URL_CACHE: Lazy<Mutex<LruCache<CacheKey, CacheEntry>>> = Lazy::new(|| {
    Mutex::new(LruCache::new(
        NonZeroUsize::new(CACHE_CAPACITY).unwrap_or_else(|| unreachable!("CACHE_CAPACITY > 0")),
    ))
});

/// Best-effort URL canonicalisation. Falls back to the raw URL if `url`
/// can't parse it (e.g. caller already sent something the SSRF layer
/// will reject anyway). Lowercasing the host + dropping fragment +
/// default-port-stripping covers >95% of "same URL different string"
/// cases without inviting more aggressive normalisation bugs.
pub(crate) fn canonicalize_url(raw: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(raw) else {
        return raw.to_string();
    };
    parsed.set_fragment(None);
    // `url` lowercases scheme and host on parse already; default ports
    // are normalised by calling set_port(None) when the port equals
    // the scheme's default.
    if matches!(
        (parsed.scheme(), parsed.port()),
        ("http", Some(80)) | ("https", Some(443)) | ("ws", Some(80)) | ("wss", Some(443))
    ) {
        let _ = parsed.set_port(None);
    }
    parsed.to_string()
}

// Match `href="…"` / `src="…"` attribute values for base-URL resolution so
// extracted Markdown links point at usable absolute "original article" URLs
// rather than the relative paths an index page ships.
static RE_HREF_SRC: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)(\s(?:href|src)\s*=\s*["'])([^"']*)(["'])"#)
        .unwrap_or_else(|e| unreachable!("invalid regex RE_HREF_SRC: {e}"))
});

/// Resolve relative `href`/`src` URLs in an HTML fragment against `base_url`
/// so extracted Markdown links point at usable absolute URLs. Falls back to
/// the original fragment when `base_url` can't be parsed; already-absolute
/// URLs and non-HTTP schemes (`mailto:`, `data:`, …) are left unchanged by
/// the RFC-3986 `join`. Protocol-relative (`//host/x`) and root-relative
/// (`/x`) forms resolve against the page's scheme/host.
pub(crate) fn resolve_relative_urls<'a>(html: &'a str, base_url: &str) -> Cow<'a, str> {
    let Ok(base) = url::Url::parse(base_url) else {
        return Cow::Borrowed(html);
    };
    RE_HREF_SRC.replace_all(html, |caps: &regex::Captures| {
        let prefix = &caps[1];
        let value = &caps[2];
        let suffix = &caps[3];
        match base.join(value) {
            Ok(abs) => format!("{prefix}{abs}{suffix}"),
            Err(_) => format!("{prefix}{value}{suffix}"),
        }
    })
}

pub(crate) fn cache_key(url: &str, mode: &ExtractMode) -> CacheKey {
    CacheKey {
        url: canonicalize_url(url),
        extract_mode: ExtractModeKey::from(mode),
    }
}

pub(crate) fn cache_lookup(key: &CacheKey) -> Option<WebFetchResult> {
    let mut guard = URL_CACHE.lock().unwrap_or_else(|e| { tracing::error!(reason = %e, "web_fetch URL_CACHE poisoned: a previous holder panicked; recovering"); e.into_inner() });
    // `LruCache::get` mutates recency, so we need &mut.
    let entry = guard.get(key)?;
    if entry.inserted_at.elapsed() > CACHE_TTL {
        guard.pop(key);
        return None;
    }
    Some(entry.result.clone())
}

pub(crate) fn cache_store(key: CacheKey, result: WebFetchResult) {
    let mut guard = URL_CACHE.lock().unwrap_or_else(|e| { tracing::error!(reason = %e, "web_fetch URL_CACHE poisoned: a previous holder panicked; recovering"); e.into_inner() });
    guard.put(
        key,
        CacheEntry {
            result,
            inserted_at: Instant::now(),
        },
    );
}

#[cfg(test)]
pub(crate) fn cache_clear() {
    URL_CACHE.lock().unwrap_or_else(|e| { tracing::error!(reason = %e, "web_fetch URL_CACHE poisoned: a previous holder panicked; recovering"); e.into_inner() }).clear();
}

#[cfg(test)]
mod tests {
    use super::super::types::Extractor;
    use super::*;

    /// Serialises tests that touch the process-global `URL_CACHE`. They each
    /// call `cache_clear()`, so without this guard a parallel sweep lets one
    /// test wipe another's just-stored entry, producing intermittent failures.
    /// Uses `std::sync::Mutex` (not the crate alias) so the `const` initialiser
    /// holds regardless of the `loom` feature.
    static CACHE_TEST_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn dummy_result(url: &str, content: &str) -> WebFetchResult {
        WebFetchResult {
            url: url.to_string(),
            title: None,
            content: content.to_string(),
            extractor: Extractor::Selector,
        }
    }

    #[test]
    fn canonicalize_url_strips_default_port_and_fragment() {
        assert_eq!(
            canonicalize_url("HTTPS://Example.COM:443/path?q=1#frag"),
            "https://example.com/path?q=1"
        );
        assert_eq!(
            canonicalize_url("http://example.com:80/"),
            "http://example.com/"
        );
        assert_eq!(
            canonicalize_url("https://example.com:8443/"),
            "https://example.com:8443/"
        );
        // Junk URLs pass through unchanged so the SSRF layer can reject them.
        assert_eq!(canonicalize_url("not-a-url"), "not-a-url");
    }

    #[test]
    fn cache_key_distinguishes_extract_modes() {
        let k1 = cache_key("https://example.com/", &ExtractMode::Markdown);
        let k2 = cache_key("https://example.com/", &ExtractMode::Text);
        assert_ne!(
            k1, k2,
            "same URL with different extract modes must be separate cache entries"
        );
    }

    #[test]
    fn cache_lookup_returns_stored_entry() {
        let _guard = CACHE_TEST_GUARD.lock().unwrap_or_else(|e| { tracing::error!(reason = %e, "web_fetch test guard poisoned; recovering"); e.into_inner() });
        cache_clear();
        let key = cache_key("https://cache-test.invalid/a", &ExtractMode::Markdown);
        assert!(cache_lookup(&key).is_none(), "fresh cache should miss");

        cache_store(
            key.clone(),
            dummy_result("https://cache-test.invalid/a", "hi"),
        );
        let got = cache_lookup(&key).expect("should hit");
        assert_eq!(got.content, "hi");
    }

    #[test]
    fn cache_lookup_returns_none_for_expired_entry() {
        let _guard = CACHE_TEST_GUARD.lock().unwrap_or_else(|e| { tracing::error!(reason = %e, "web_fetch test guard poisoned; recovering"); e.into_inner() });
        cache_clear();
        let key = cache_key("https://cache-test.invalid/b", &ExtractMode::Markdown);
        // Direct insert with an `inserted_at` in the past — bypass
        // `cache_store` so the test doesn't have to actually wait 15
        // minutes for the TTL to elapse.
        {
            let mut guard = URL_CACHE.lock().unwrap_or_else(|e| { tracing::error!(reason = %e, "web_fetch URL_CACHE poisoned: a previous holder panicked; recovering"); e.into_inner() });
            guard.put(
                key.clone(),
                CacheEntry {
                    result: dummy_result("https://cache-test.invalid/b", "stale"),
                    inserted_at: Instant::now()
                        .checked_sub(CACHE_TTL + Duration::from_secs(1))
                        .expect("Instant arithmetic"),
                },
            );
        }
        assert!(
            cache_lookup(&key).is_none(),
            "expired entry must be reported as a miss"
        );
        // And evicted.
        let guard = URL_CACHE.lock().unwrap_or_else(|e| { tracing::error!(reason = %e, "web_fetch URL_CACHE poisoned: a previous holder panicked; recovering"); e.into_inner() });
        assert!(guard.peek(&key).is_none(), "expired entry must be evicted");
    }

    #[test]
    fn cache_key_normalises_url_for_hit() {
        let _guard = CACHE_TEST_GUARD.lock().unwrap_or_else(|e| { tracing::error!(reason = %e, "web_fetch test guard poisoned; recovering"); e.into_inner() });
        cache_clear();
        let stored = cache_key("HTTPS://Example.com:443/path", &ExtractMode::Markdown);
        cache_store(stored, dummy_result("https://example.com/path", "ok"));

        // Caller requests the same URL with a different surface form —
        // canonicalisation should bring them to the same cache slot.
        let looked = cache_key("https://example.com/path#frag", &ExtractMode::Markdown);
        assert!(
            cache_lookup(&looked).is_some(),
            "URLs differing only in case/port/fragment must share a cache entry"
        );
    }

    #[test]
    fn resolve_relative_urls_handles_common_cases() {
        let base = "https://news.test/world/index.html";
        let html = r#"<a href="/a">x</a><a href="b">y</a><a href="https://z.test/c">z</a><a href="mailto:t@x.test">m</a><img src="//cdn.test/i.png">"#;
        let out = resolve_relative_urls(html, base);
        assert!(out.contains("https://news.test/a"), "root-relative: {out}");
        assert!(
            out.contains("https://news.test/world/b"),
            "path-relative: {out}"
        );
        assert!(out.contains("https://z.test/c"), "absolute kept: {out}");
        assert!(out.contains("mailto:t@x.test"), "scheme kept: {out}");
        assert!(
            out.contains("https://cdn.test/i.png"),
            "protocol-relative: {out}"
        );
    }
}
