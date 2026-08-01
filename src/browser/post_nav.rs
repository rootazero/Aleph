//! Post-navigation URL re-check with quarantine (openclaw
//! `assertBrowserNavigationResultAllowed` parity).
//!
//! The navigation-time guard (`BrowserSsrfGuard::check_navigation`) only vets
//! the URL being navigated *to*. An HTTP redirect chain can still land the tab
//! on a blocked private/loopback origin afterwards — and without a re-check the
//! tab would stay open there, readable by every content tool until the
//! read-time guard happens to fire. This module closes that gap: after a
//! successful `open_tab` / `navigate`, the backend re-lists the tabs, re-runs
//! `check_url` on the landed URL, and best-effort closes the offending tab on
//! a violation.
//!
//! Only `open_tab` / `navigate` are wired. History (`back`/`forward`) and
//! interaction ops are deliberately NOT audited here — the read-time guard in
//! the tool layer (`make_backend_and_tab_guarded`) already covers anything
//! those ops can expose, and the agent must always be able to navigate *away*
//! from a blocked page.
//!
//! The audit is defense-in-depth, never a hard gate: if the post-navigation
//! `list_tabs` itself fails, the successful navigation stands.

use super::backend::BrowserBackend;
use super::error::BrowserError;
use super::network_policy::BrowserSsrfGuard;
use super::tab_registry;

/// Re-validate the landed URL from a `list_tabs` snapshot against the SSRF
/// policy.
///
/// URL resolution: `tab_id`'s line when given and found, else the active
/// (last-listed) tab — newly opened tabs append to the list, and a `navigate`
/// targets the session's current tab. Non-http(s) landed URLs (`about:blank`,
/// `chrome://`, …) carry no network target and are skipped, as is an empty /
/// unparseable listing.
///
/// Returns `Ok(())` when the landed URL is policy-clean. On a violation the
/// caller is expected to quarantine (close) the tab — the error message says
/// so, since by the time the model reads it the close has already run.
pub(crate) async fn verify_landed_url(
    guard: &BrowserSsrfGuard,
    tabs_text: &str,
    tab_id: Option<&str>,
) -> Result<(), BrowserError> {
    let url = tab_id
        .and_then(|id| tab_registry::tab_url_for(tabs_text, id))
        .or_else(|| tab_registry::parse_active_tab_url(tabs_text));
    let Some(url) = url else { return Ok(()) };
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Ok(());
    }
    guard.check_url(&url).await.map_err(|v| {
        BrowserError::NavigationFailed(format!(
            "tab closed after landing on a policy-blocked origin ({v}): \
             an HTTP redirect moved the tab onto a blocked URL after the \
             navigation-time guard approved the target"
        ))
    })
}

/// Shared post-navigation audit for both backends: re-list the tabs, re-check
/// the landed URL, and on a violation best-effort close the offending tab
/// before returning the error.
///
/// A failed `list_tabs` is logged at debug and the navigation is left alone —
/// the audit must never turn a successful navigation into a failure.
pub(crate) async fn audit_landed_tab<B: BrowserBackend + ?Sized>(
    backend: &B,
    guard: &BrowserSsrfGuard,
    tab_id: Option<&str>,
) -> Result<(), BrowserError> {
    let tabs_text = match backend.list_tabs().await {
        Ok(text) => text,
        Err(e) => {
            tracing::debug!(error = %e, "post-navigation audit skipped: list_tabs failed");
            return Ok(());
        }
    };
    if let Err(err) = verify_landed_url(guard, &tabs_text, tab_id).await {
        // Quarantine: close the tab so it does not stay open on the blocked
        // origin. Best-effort — a close failure is logged, not propagated, so
        // the model still sees the policy violation itself.
        let offender = tab_id
            .map(str::to_string)
            .or_else(|| tab_registry::parse_tab_ids(&tabs_text).last().cloned());
        if let Some(id) = offender {
            if let Err(e) = backend.close_tab(&id).await {
                tracing::warn!(tab = %id, error = %e, "post-navigation quarantine: failed to close blocked tab");
            }
        }
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::network_policy::SsrfConfig;

    #[tokio::test]
    async fn blocks_loopback_landed_url() {
        // Default policy blocks loopback; an IP literal needs no DNS.
        let guard = BrowserSsrfGuard::default();
        let tabs = "1: https://example.com\n2: http://127.0.0.1:9000/admin [selected]";
        // By tab id…
        let err = verify_landed_url(&guard, tabs, Some("2"))
            .await
            .expect_err("loopback landed URL must be rejected");
        assert!(
            err.to_string().contains("policy-blocked origin"),
            "unexpected message: {err}"
        );
        // …and via the active (last) tab when no id is given.
        assert!(verify_landed_url(&guard, tabs, None).await.is_err());
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
        assert!(verify_landed_url(&guard, tabs, Some("1")).await.is_ok());
        // Public IP literal passes without any DNS at all.
        assert!(verify_landed_url(&guard, "1: https://8.8.8.8/", None)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn skips_non_http_and_empty_listings() {
        let guard = BrowserSsrfGuard::default();
        // Non-http(s) schemes carry no network target.
        assert!(verify_landed_url(&guard, "1: about:blank", None)
            .await
            .is_ok());
        assert!(
            verify_landed_url(&guard, "1: chrome://extensions", Some("1"))
                .await
                .is_ok()
        );
        // Empty / unparseable listing → nothing to check.
        assert!(verify_landed_url(&guard, "", None).await.is_ok());
        assert!(verify_landed_url(&guard, "noise", Some("1")).await.is_ok());
        // Unknown tab id falls back to the active tab's URL.
        assert!(verify_landed_url(&guard, "1: about:blank", Some("9"))
            .await
            .is_ok());
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
        assert!(verify_landed_url(&guard, "1: http://127.0.0.1/", None)
            .await
            .is_ok());
    }
}
