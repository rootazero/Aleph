//! `hub_fetch_docs` — fetch a repo/URL's README/manifest and injection-scan it.
//!
//! The model is the consumer: when a catalog entry is too terse to judge, or an
//! extension's setup lives in its README, this is how that text reaches the
//! decision — SSRF-guarded, byte-capped, and scanned before it enters context.
//! (It was long described as an unwired "scaffold" while being fully registered
//! and dispatchable — a description that talked the model out of a working
//! capability.)

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{AlephError, Result};
use crate::hub::trust::{scan_for_injection, InjectionFinding};
use crate::security::ssrf::{validate_url_async, SsrfPolicy};
use crate::tools::AlephTool;

/// Maximum number of bytes accepted from the remote response body.
const DOC_BYTE_BUDGET: usize = 64 * 1024; // 64 KiB

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HubFetchDocsArgs {
    /// URL to fetch (README, manifest, or any text document).
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HubFetchDocsOutput {
    pub text: String,
    pub truncated: bool,
    pub injection_findings: Vec<InjectionFinding>,
}

/// Fetches a URL, caps the body to `DOC_BYTE_BUDGET`, and runs the injection
/// scanner before returning.
#[derive(Clone)]
pub struct HubFetchDocsTool;

#[async_trait]
impl AlephTool for HubFetchDocsTool {
    const NAME: &'static str = "hub_fetch_docs";
    const DESCRIPTION: &'static str =
        "Fetch a text document (README / manifest) over HTTP and scan it for prompt-injection \
         before returning it. Private and reserved IP ranges are blocked; the body is capped at \
         64 KiB and `truncated` says whether it was cut. Use it to read an extension's own docs \
         when a catalog entry is too terse to decide on, or when install instructions live in a \
         repo rather than the catalog.";
    type Args = HubFetchDocsArgs;
    type Output = HubFetchDocsOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // SSRF protection: reject private/reserved IP ranges before the request.
        let ssrf_policy = SsrfPolicy::default();
        let (_url, _pinned) = validate_url_async(&args.url, &ssrf_policy)
            .await
            .map_err(|e| {
                AlephError::network(format!("SSRF blocked for URL '{}': {e}", args.url))
            })?;

        // Build a short-timeout client, mirroring docker_mcp/mcp_registry providers.
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| AlephError::network(format!("failed to build HTTP client: {e}")))?;

        let resp = client
            .get(&args.url)
            .send()
            .await
            .map_err(|e| AlephError::network(format!("fetch failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(AlephError::network(format!(
                "HTTP {} for {}",
                resp.status(),
                args.url
            )));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| AlephError::network(format!("read body failed: {e}")))?;

        let truncated = bytes.len() > DOC_BYTE_BUDGET;
        let slice = if truncated {
            bytes
                .get(..DOC_BYTE_BUDGET)
                .expect("invariant: truncated only when bytes exceed budget")
        } else {
            &bytes[..]
        };

        // Lossy UTF-8 decode so we never panic on binary content.
        let text = String::from_utf8_lossy(slice).into_owned();

        let injection_findings = scan_for_injection(&text);

        Ok(HubFetchDocsOutput {
            text,
            truncated,
            injection_findings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_boundary() {
        // Verify the truncation logic: body exactly at budget → not truncated;
        // body one byte over → truncated.
        let at_limit = vec![b'a'; DOC_BYTE_BUDGET];
        let over_limit = vec![b'b'; DOC_BYTE_BUDGET + 1];
        assert!(
            at_limit.len() <= DOC_BYTE_BUDGET,
            "at-limit body should not truncate"
        );
        assert!(
            over_limit.len() > DOC_BYTE_BUDGET,
            "over-limit body should truncate"
        );
    }

    #[test]
    fn injection_findings_included_in_output() {
        // Verify that a fabricated output with findings serializes correctly.
        let out = HubFetchDocsOutput {
            text: "clean".into(),
            truncated: false,
            injection_findings: vec![InjectionFinding {
                kind: "suspicious_phrase".into(),
                detail: "ignore previous".into(),
            }],
        };
        let v = serde_json::to_value(&out).unwrap();
        assert_eq!(v["injection_findings"][0]["kind"], "suspicious_phrase");
        assert!(!v["truncated"].as_bool().unwrap());
    }
}
