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

use crate::error::Result;
use crate::hub::trust::{scan_for_injection, InjectionFinding};
use crate::security::ssrf::{safe_fetch, SafeFetchRequest, SsrfPolicy};
use crate::tools::AlephTool;

/// Maximum number of bytes handed back to the model. Bodies above this are
/// clipped and flagged via `truncated`, never rejected.
const DOC_BYTE_BUDGET: usize = 64 * 1024; // 64 KiB

/// Hard ceiling passed to `safe_fetch` purely as a memory bound.
///
/// These are deliberately two different numbers. `safe_fetch`'s cap **aborts**
/// the read with an error — it does not truncate — so wiring it directly to
/// `DOC_BYTE_BUDGET` (as `ef9282462` did) turns every doc over 64 KiB into a
/// hard `fetch failed: response body too large` and leaves `truncated` able to
/// fire only on an exact one-byte overshoot. Graceful truncation is the tool's
/// contract; the streaming cap is only here so a hostile upstream cannot OOM
/// the process. Clipping to the budget then happens locally, below.
const DOC_FETCH_CEILING: usize = 4 * 1024 * 1024; // 4 MiB

// Compile-time, not a test: if the abort ceiling ever slips to or below the clip
// budget, truncation becomes unreachable and over-budget docs fail outright —
// which is exactly the regression this pair of constants exists to prevent.
const _: () = assert!(DOC_FETCH_CEILING > DOC_BYTE_BUDGET);

/// Clip a fetched body to `DOC_BYTE_BUDGET`, reporting whether it was cut.
///
/// Lossy UTF-8 decode so a cut landing mid-codepoint (or binary content) can
/// never panic.
fn clip_to_budget(body: &[u8]) -> (String, bool) {
    // Exactly at the budget is NOT truncated — only a body strictly over it is.
    match body.get(..DOC_BYTE_BUDGET) {
        Some(clipped) if body.len() > DOC_BYTE_BUDGET => {
            (String::from_utf8_lossy(clipped).into_owned(), true)
        }
        _ => (String::from_utf8_lossy(body).into_owned(), false),
    }
}

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
        let ssrf_policy = SsrfPolicy::default();
        let fetch_request = SafeFetchRequest::get(std::time::Duration::from_secs(10))
            .with_max_body_bytes(DOC_FETCH_CEILING);

        let resp = safe_fetch(&args.url, &ssrf_policy, fetch_request)
            .await
            .map_err(|e| crate::error::AlephError::network(format!("fetch failed: {e}")))?;

        if !resp.status.is_success() {
            return Err(crate::error::AlephError::network(format!(
                "HTTP {} for {}",
                resp.status, args.url
            )));
        }

        let (text, truncated) = clip_to_budget(&resp.body);

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

    /// The previous version of this test built two `Vec`s and asserted their
    /// lengths against the constant they were built from — a tautology that
    /// never called the clipping logic, and stayed green while `ef9282462` made
    /// an over-budget doc fail outright instead of truncating. Exercise the
    /// real function.
    #[test]
    fn clip_to_budget_truncates_instead_of_failing() {
        let (text, truncated) = clip_to_budget(b"short doc");
        assert_eq!(text, "short doc");
        assert!(!truncated);

        // Exactly at the budget is not a truncation.
        let at_limit = vec![b'a'; DOC_BYTE_BUDGET];
        let (text, truncated) = clip_to_budget(&at_limit);
        assert_eq!(text.len(), DOC_BYTE_BUDGET);
        assert!(!truncated, "a body exactly at the budget was not cut");

        // Over the budget: clipped to the budget and flagged — NOT an error.
        let over = vec![b'b'; DOC_BYTE_BUDGET * 3];
        let (text, truncated) = clip_to_budget(&over);
        assert_eq!(text.len(), DOC_BYTE_BUDGET);
        assert!(truncated, "an over-budget body must report truncated");
    }

    /// A cut landing mid-codepoint must not panic — multi-byte content is the
    /// normal case for non-English docs.
    ///
    /// Note the result may be a couple of bytes OVER the budget: the trailing
    /// partial codepoint is replaced by U+FFFD, which is 3 bytes. That is the
    /// long-standing behaviour of the lossy decode and is immaterial at 64 KiB;
    /// what matters is that it neither panics nor emits invalid UTF-8.
    #[test]
    fn clip_to_budget_survives_a_cut_inside_a_codepoint() {
        // 3 bytes per char, so the budget boundary cannot land on a char edge.
        let doc: String = "中".repeat(DOC_BYTE_BUDGET);
        let (text, truncated) = clip_to_budget(doc.as_bytes());
        assert!(truncated);
        assert!(std::str::from_utf8(text.as_bytes()).is_ok());
        assert!(
            text.len() <= DOC_BYTE_BUDGET + 3,
            "clip overshot by more than one replacement char: {}",
            text.len()
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
