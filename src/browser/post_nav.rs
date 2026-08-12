//! Post-navigation URL re-check with quarantine (openclaw
//! `assertBrowserNavigationResultAllowed` parity).
//!
//! The navigation-time guard (`BrowserSsrfGuard::check_navigation`) only vets
//! the URL being navigated *to*. An HTTP redirect chain can still land the tab
//! on a blocked private/loopback origin afterwards — and without a re-check the
//! tab would stay open there, readable by every content tool until the
//! read-time guard happens to fire. This module closes that gap: after a
//! successful `open_tab` / `navigate`, the landed URL is re-checked against the
//! policy and the offending tab is best-effort closed on a violation.
//!
//! **The quarantine (the `close_tab`) lives here exactly once.** Both backends
//! used to hand-copy the "verify, then close, then return the policy error"
//! sequence into their `open_tab`, which is how the copies drifted; every entry
//! point below funnels into [`audit_landed_url`].
//!
//! Only `open_tab` / `navigate` are wired. History (`back`/`forward`) and
//! interaction ops are deliberately NOT audited here — the read-time guard in
//! the tool layer (`make_backend_and_tab_guarded`) already covers anything
//! those ops can expose, and the agent must always be able to navigate *away*
//! from a blocked page.
//!
//! The audit is defense-in-depth, never a hard gate: if the post-navigation
//! `list_tabs` itself fails, the successful navigation stands (and the skip is
//! logged — a silently skipped security audit is indistinguishable from one
//! that passed).

use super::backend::BrowserBackend;
use super::error::BrowserError;
use super::network_policy::BrowserSsrfGuard;
use super::tab_registry;

/// Resolve the URL a navigation landed on from a `list_tabs` snapshot.
///
/// `tab_id`'s line when given and found, else the listing's active tab
/// ([`tab_registry::active_tab`] — the driver's `[selected]` marker first,
/// last-listed only as fallback). `None` for an empty / unparseable listing.
fn landed_url(tabs_text: &str, tab_id: Option<&str>) -> Option<String> {
    tab_id
        .and_then(|id| tab_registry::tab_url_for(tabs_text, id))
        .or_else(|| tab_registry::active_tab_url(tabs_text))
}

/// Re-validate one landed URL against the SSRF policy (no browser I/O).
///
/// Non-http(s) landed URLs (`about:blank`, `chrome://`, …) carry no network
/// target and are skipped.
///
/// Returns `Ok(())` when the landed URL is policy-clean. The error message
/// states that the tab was closed, since every caller that surfaces it has
/// already run the quarantine below.
async fn check_landed_url(guard: &BrowserSsrfGuard, url: &str) -> Result<(), BrowserError> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Ok(());
    }
    guard.check_url(url).await.map_err(|v| {
        BrowserError::NavigationFailed(format!(
            "tab closed after landing on a policy-blocked origin ({v}): \
             an HTTP redirect moved the tab onto a blocked URL after the \
             navigation-time guard approved the target"
        ))
    })
}

/// Check one authoritative landed URL and quarantine `offender` on a violation.
///
/// The single quarantine site. `offender` is the tab to close; `None` means the
/// caller could not name a tab (an unparseable listing), in which case the
/// policy error is still surfaced — losing the close is strictly better than
/// losing the verdict.
pub(crate) async fn audit_landed_url<B: BrowserBackend + ?Sized>(
    backend: &B,
    guard: &BrowserSsrfGuard,
    url: &str,
    offender: Option<&str>,
) -> Result<(), BrowserError> {
    let Err(err) = check_landed_url(guard, url).await else {
        return Ok(());
    };
    // Quarantine: close the tab so it does not stay open on the blocked
    // origin. Best-effort — a close failure is logged, not propagated, so
    // the model still sees the policy violation itself.
    if let Some(id) = offender {
        if let Err(e) = backend.close_tab(id).await {
            tracing::warn!(tab = %id, error = %e, "post-navigation quarantine: failed to close blocked tab");
        }
    } else {
        tracing::warn!(
            url = %url,
            "post-navigation quarantine: blocked landing has no resolvable tab id to close"
        );
    }
    Err(err)
}

/// Audit an already-fetched `list_tabs` snapshot (used by `open_tab`, which
/// needs the listing anyway to learn the new tab's id — no extra round trip).
pub(crate) async fn audit_listing<B: BrowserBackend + ?Sized>(
    backend: &B,
    guard: &BrowserSsrfGuard,
    tabs_text: &str,
    tab_id: Option<&str>,
) -> Result<(), BrowserError> {
    let Some(url) = landed_url(tabs_text, tab_id) else {
        return Ok(());
    };
    let offender = tab_id
        .map(str::to_string)
        .or_else(|| tab_registry::active_tab_id(tabs_text));
    audit_landed_url(backend, guard, &url, offender.as_deref()).await
}

/// Re-list the tabs, then audit the landed URL (used by `navigate`).
///
/// A failed `list_tabs` is logged and the navigation is left alone — the audit
/// must never turn a successful navigation into a failure.
pub(crate) async fn audit_landed_tab<B: BrowserBackend + ?Sized>(
    backend: &B,
    guard: &BrowserSsrfGuard,
    tab_id: Option<&str>,
) -> Result<(), BrowserError> {
    let tabs_text = match backend.list_tabs().await {
        Ok(text) => text,
        Err(e) => {
            tracing::warn!(error = %e, "post-navigation audit skipped: list_tabs failed");
            return Ok(());
        }
    };
    audit_listing(backend, guard, &tabs_text, tab_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::network_policy::SsrfConfig;
    use crate::browser::testkit::FakeBackend;

    /// Audit a listing through the real entry point, with a backend that
    /// records whatever quarantine it triggers.
    async fn audit(
        guard: &BrowserSsrfGuard,
        tabs: &str,
        tab_id: Option<&str>,
    ) -> (Result<(), BrowserError>, Vec<String>) {
        let backend = FakeBackend::new(None);
        let out = audit_listing(&backend, guard, tabs, tab_id).await;
        (out, backend.calls())
    }

    #[tokio::test]
    async fn blocks_loopback_landed_url() {
        // Default policy blocks loopback; an IP literal needs no DNS.
        let guard = BrowserSsrfGuard::default();
        let tabs = "1: https://example.com\n2: http://127.0.0.1:9000/admin [selected]";
        // By tab id…
        let (out, calls) = audit(&guard, tabs, Some("2")).await;
        let err = out.expect_err("loopback landed URL must be rejected");
        assert!(
            err.to_string().contains("policy-blocked origin"),
            "unexpected message: {err}"
        );
        assert_eq!(calls, vec!["close_tab:2"]);
        // …and via the active tab when no id is given.
        assert!(audit(&guard, tabs, None).await.0.is_err());
    }

    #[tokio::test]
    async fn passes_public_landed_url() {
        // Resolver hook pins the hostname to a public IP so the async DNS
        // validation in check_url succeeds offline.
        let _scope = crate::security::ssrf::dns::test_hook::ResolverScope::install({
            let mut m = std::collections::HashMap::new();
            m.insert("example.com".to_string(), vec!["8.8.8.8".parse().unwrap()]);
            m
        });
        let guard = BrowserSsrfGuard::default();
        let tabs = "1: https://example.com/docs [selected]";
        assert_eq!(audit(&guard, tabs, Some("1")).await.1.len(), 0);
        assert!(audit(&guard, tabs, Some("1")).await.0.is_ok());
        // Public IP literal passes without any DNS at all.
        assert!(audit(&guard, "1: https://8.8.8.8/", None).await.0.is_ok());
    }

    #[tokio::test]
    async fn skips_non_http_and_empty_listings() {
        let guard = BrowserSsrfGuard::default();
        // Non-http(s) schemes carry no network target.
        assert!(audit(&guard, "1: about:blank", None).await.0.is_ok());
        assert!(audit(&guard, "1: chrome://extensions", Some("1"))
            .await
            .0
            .is_ok());
        // Empty / unparseable listing → nothing to check.
        assert!(audit(&guard, "", None).await.0.is_ok());
        assert!(audit(&guard, "noise", Some("1")).await.0.is_ok());
        // Unknown tab id falls back to the active tab's URL.
        assert!(audit(&guard, "1: about:blank", Some("9")).await.0.is_ok());
    }

    #[tokio::test]
    async fn disabled_policy_passes_loopback() {
        let guard = BrowserSsrfGuard::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec![],
            block_secrets_in_url: false,
            block_secrets_in_input: false,
            redact_secrets_in_content: false,
        });
        let (out, calls) = audit(&guard, "1: http://127.0.0.1/", None).await;
        assert!(out.is_ok());
        assert!(calls.is_empty(), "nothing may be quarantined: {calls:?}");
    }

    // --- the quarantine half: the audit must actually CLOSE the blocked tab ---

    #[tokio::test]
    async fn audit_landed_tab_closes_the_blocked_tab() {
        let guard = BrowserSsrfGuard::default();
        let backend =
            FakeBackend::new(None).with_tabs_text("1: https://ok.example\n2: http://127.0.0.1/x");
        let err = audit_landed_tab(&backend, &guard, Some("2"))
            .await
            .expect_err("loopback landing must be refused");
        assert!(err.to_string().contains("policy-blocked origin"));
        assert_eq!(
            backend.calls(),
            vec!["list_tabs".to_string(), "close_tab:2".to_string()],
            "the blocked tab must be closed, not merely reported"
        );
    }

    #[tokio::test]
    async fn audit_closes_the_active_tab_when_no_id_is_given() {
        // No tab id → the offender is the listing's active tab (marker first).
        let guard = BrowserSsrfGuard::default();
        let backend = FakeBackend::new(None)
            .with_tabs_text("1: http://127.0.0.1/x [selected]\n2: https://ok.example");
        assert!(audit_landed_tab(&backend, &guard, None).await.is_err());
        assert_eq!(backend.calls(), vec!["list_tabs", "close_tab:1"]);
    }

    #[tokio::test]
    async fn audit_leaves_a_clean_landing_untouched() {
        let guard = BrowserSsrfGuard::default();
        let backend = FakeBackend::new(None).with_tabs_text("1: about:blank [selected]");
        assert!(audit_landed_tab(&backend, &guard, None).await.is_ok());
        assert_eq!(backend.calls(), vec!["list_tabs"], "no tab may be closed");
    }

    #[tokio::test]
    async fn a_failed_listing_skips_the_audit_without_failing_the_navigation() {
        // fail_at = 1 makes the first recorded call (list_tabs) fail.
        let guard = BrowserSsrfGuard::default();
        let backend = FakeBackend::new(Some(1));
        assert!(
            audit_landed_tab(&backend, &guard, None).await.is_ok(),
            "the audit must never turn a successful navigation into a failure"
        );
        assert_eq!(backend.calls(), vec!["list_tabs"]);
    }

    #[tokio::test]
    async fn audit_landed_url_quarantines_an_authoritative_url() {
        // The `goto` path knows the landed URL without re-listing.
        let guard = BrowserSsrfGuard::default();
        let backend = FakeBackend::new(None);
        assert!(
            audit_landed_url(&backend, &guard, "http://127.0.0.1/admin", Some("7"))
                .await
                .is_err()
        );
        assert_eq!(backend.calls(), vec!["close_tab:7"]);
    }
}
