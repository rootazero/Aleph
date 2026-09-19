// Browser open tool — opens a URL in a managed browser profile.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `browser_open` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserOpenArgs {
    /// URL to open.
    pub url: String,
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Which browser engine to start for this profile, when nothing is running
    /// yet: "obscura" (default, fast) or "chromium" (the escape hatch).
    /// Refused — not silently honoured — if this profile already has a browser
    /// running: its cookies live in that process, so moving is
    /// `browser_session{action:"switch_engine"}`, not a flag on open.
    /// The profile then STAYS on that engine until the server restarts or you
    /// switch back; the config file is unchanged.
    #[serde(default)]
    pub engine: Option<crate::browser::engine::Engine>,
}

/// Output from the `browser_open` tool.
#[derive(Debug, Serialize)]
pub struct BrowserOpenOutput {
    pub success: bool,
    pub tab_id: Option<String>,
    pub message: Option<String>,
    /// Which engine served this call. Present so the model never has to infer
    /// which browser it is driving from the absence of an error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
}

/// Opens a URL in a managed browser profile with SSRF protection.
#[derive(Clone)]
pub struct BrowserOpenTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserOpenTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate `browser_open` behind the same approval policy as
    /// `browser_navigate` — opening a fresh tab is the same trust surface as
    /// navigating an existing one, so a deny on a host must deny both. With
    /// no policy wired the tool behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

#[async_trait]
impl AlephTool for BrowserOpenTool {
    const NAME: &'static str = "browser_open";
    const DESCRIPTION: &'static str = "Open a URL in a headless browser. Default profile is fast and invisible. Only use profile=\"user\" when the user explicitly asks to use their Chrome browser with logged-in sessions.";
    type Args = BrowserOpenArgs;
    type Output = BrowserOpenOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // SSRF + secret-exfiltration check before creating backend
        if let Err(violation) = self.manager.check_navigation(&args.url).await {
            return Ok(BrowserOpenOutput {
                success: false,
                tab_id: None,
                message: Some(format!("Blocked: {violation}")),
                // Nothing has been resolved at this point, and `None` says
                // "unknown" — never "the default one" (判据 §8).
                engine: None,
            });
        }

        // Approval gate mirrors `browser_navigate`: a host the user has
        // already denied should not be reachable via a fresh tab.
        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserOpen,
            "open",
            &args.url,
        )
        .await
        {
            return Ok(BrowserOpenOutput {
                success: false,
                tab_id: None,
                message: Some(message),
                engine: None,
            });
        }

        // Resolve (and, for a cold profile, launch) the engine BEFORE the
        // backend: `get_backend` is synchronous and cannot start a process, and
        // an `engine` override against a live handle of the other engine is a
        // refusal, not a relaunch — the profile's cookies are in that process.
        // The refusal itself is `EngineRegistry`'s `EngineMismatch`, whose text
        // already names `switch_engine`; this call site adds no second wording.
        //
        // `None` back from it is not a failure: it is a driver that has no
        // engine at all (`managed`, `existing_session`), whose backend is built
        // by `make_backend` just below.
        let engine = match self
            .manager
            .prepare_engine(&args.profile, args.engine)
            .await
        {
            Ok(e) => e,
            Err(e) => {
                return Ok(BrowserOpenOutput {
                    success: false,
                    tab_id: None,
                    message: Some(super::backend_error_text(&self.manager, &e)),
                    engine: None,
                });
            }
        };
        let backend = match super::make_backend(&self.manager, &args.profile) {
            Ok(b) => b,
            Err(e) => {
                return Ok(BrowserOpenOutput {
                    success: false,
                    tab_id: None,
                    message: Some(super::backend_error_text(&self.manager, &e)),
                    engine: engine.map(|e| e.as_str().to_string()),
                });
            }
        };
        match backend.open_tab(&args.url).await {
            Ok(tab_id) => {
                // Register the tab we just created. Every OTHER browser verb
                // reaches the registry through `make_backend_and_tab`, but this
                // one resolves no existing tab and so registered nothing — and
                // both reapers take their candidate list from the registry
                // (`TabRegistry::has_tabs`). A session that only ever called
                // `browser_open` was therefore invisible to the LRU cap and to
                // the idle sweep alike: the registry did not know about the one
                // thing it exists to bound.
                self.manager.touch_tab(&args.profile, &tab_id);
                Ok(BrowserOpenOutput {
                    success: true,
                    tab_id: Some(tab_id),
                    message: Some(format!(
                        "Opened {} in profile '{}'{}",
                        args.url,
                        args.profile,
                        // Said only when an override actually chose the engine.
                        // The adoption is sticky for the life of the server
                        // process — the same boundary `switch_engine` states on
                        // its own face — but on every open that never asked for
                        // an engine the sentence would be noise about a setting
                        // the caller did not touch.
                        match (args.engine, engine) {
                            (Some(_), Some(e)) => format!(
                                ". This profile runs {e} until the server restarts or you \
                                 switch back — the config file is unchanged"
                            ),
                            _ => String::new(),
                        }
                    )),
                    engine: engine.map(|e| e.as_str().to_string()),
                })
            }
            Err(e) => Ok(BrowserOpenOutput {
                success: false,
                tab_id: None,
                message: Some(format!(
                    "Failed to open tab: {}",
                    super::backend_error_text(&self.manager, &e)
                )),
                engine: engine.map(|e| e.as_str().to_string()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_browser_open_ssrf_blocks_private() {
        let mut config = BrowserSystemConfig::default();
        config.policy.block_private = true;
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserOpenTool::new(manager);

        let result = tool
            .call(BrowserOpenArgs {
                url: "http://localhost:3000/admin".into(),
                profile: "default".into(),
                engine: None,
            })
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.message.unwrap().contains("Blocked"));
    }

    #[tokio::test]
    async fn test_browser_open_blocks_ssrf_private_ip() {
        let mut config = BrowserSystemConfig::default();
        config.policy.block_private = true;
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserOpenTool::new(manager);

        for url in &[
            "http://10.0.0.1/secret",
            "http://172.16.0.1/internal",
            "http://192.168.1.1/router",
            "http://[::1]/",
        ] {
            let result = tool
                .call(BrowserOpenArgs {
                    url: url.to_string(),
                    profile: "default".into(),
                    engine: None,
                })
                .await
                .unwrap();

            assert!(!result.success, "Should block {}", url);
            assert!(
                result.message.as_ref().unwrap().contains("Blocked"),
                "Should have Blocked message for {}",
                url
            );
        }
    }

    #[tokio::test]
    async fn test_browser_open_blocked_domain_list() {
        use crate::browser::network_policy::SsrfConfig;

        let config = BrowserSystemConfig {
            policy: SsrfConfig {
                block_private: false,
                blocked_domains: vec!["*.evil.com".to_string(), "malware.org".to_string()],
                allowed_domains: vec![],
                block_secrets_in_url: false,
                block_secrets_in_input: false,
                redact_secrets_in_content: false,
            },
            ..Default::default()
        };

        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserOpenTool::new(manager);

        // Should block evil.com subdomain
        let result = tool
            .call(BrowserOpenArgs {
                url: "http://sub.evil.com/payload".into(),
                profile: "default".into(),
                engine: None,
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.as_ref().unwrap().contains("Blocked"));

        // Should block malware.org
        let result = tool
            .call(BrowserOpenArgs {
                url: "http://malware.org/payload".into(),
                profile: "default".into(),
                engine: None,
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.as_ref().unwrap().contains("Blocked"));

        // Should allow normal domains (passes policy, but fails without a running browser)
        let result = tool
            .call(BrowserOpenArgs {
                url: "https://safe.com".into(),
                profile: "default".into(),
                engine: None,
            })
            .await
            .unwrap();
        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    #[tokio::test]
    async fn test_browser_open_allowlist_mode() {
        use crate::browser::network_policy::SsrfConfig;

        let config = BrowserSystemConfig {
            policy: SsrfConfig {
                block_private: false,
                blocked_domains: vec![],
                allowed_domains: vec!["*.allowed.com".to_string()],
                block_secrets_in_url: false,
                block_secrets_in_input: false,
                redact_secrets_in_content: false,
            },
            ..Default::default()
        };

        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserOpenTool::new(manager);

        // Should allow allowed.com subdomain (passes policy, but fails without a running browser)
        let result = tool
            .call(BrowserOpenArgs {
                url: "http://app.allowed.com/page".into(),
                profile: "default".into(),
                engine: None,
            })
            .await
            .unwrap();
        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());

        // Should block non-allowed domain
        let result = tool
            .call(BrowserOpenArgs {
                url: "http://other.com/page".into(),
                profile: "default".into(),
                engine: None,
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.as_ref().unwrap().contains("Blocked"));
    }

    /// A URL the guard admits degrades on the BROWSER, not on the network.
    ///
    /// Three things were wrong with the older form of this test, and between
    /// them they hid a hazard:
    ///
    /// * it asserted only `!success` + `message.is_some()`, which stays green
    ///   under any failure reason at all (判据 §2) — including the one
    ///   `prepare_engine` introduced;
    /// * it held **no `$ALEPH_HOME` guard** while reaching a path that now
    ///   resolves one. With a real obscura in the developer's own runtime
    ///   ledger, a unit test that reaches `prepare_engine` **launches a
    ///   browser** and writes into `~/.aleph`;
    /// * it depended on the host's resolver. `example.com` is a public name and
    ///   a benchmark-range address on this machine (measured: the call is
    ///   refused with `198.18.0.138 resolves to a private network`), so "a
    ///   public URL is admitted" is a claim about DNS rather than about this
    ///   tool. The guard is opened explicitly instead — it is not this test's
    ///   subject, and the allowlist tests above own it.
    #[tokio::test]
    async fn test_browser_open_allows_public() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        manager.apply_policy(crate::browser::network_policy::SsrfConfig {
            block_private: false,
            ..Default::default()
        });
        let tool = BrowserOpenTool::new(manager);

        let result = tool
            .call(BrowserOpenArgs {
                url: "https://example.com".into(),
                profile: "default".into(),
                engine: None,
            })
            .await
            .unwrap();

        // Without a browser to run the call degrades — and the REASON is a
        // missing engine runtime, never a network refusal. Under an empty
        // `$ALEPH_HOME` the ledger holds no obscura, so this is deterministic.
        assert!(!result.success);
        let message = result.message.expect("a refusal says why");
        assert!(
            message.contains("obscura"),
            "the refusal must name the engine that could not start: {message}"
        );
        assert!(
            !message.contains("Blocked"),
            "the guard is open in this test; a refusal from it would mean the \
             call never reached the browser at all: {message}"
        );
    }
    /// The one-shot override cannot silently relaunch. A profile already
    /// serving one engine refuses the other by name and points at the verb that
    /// can do it (判据 §14). The message is `EngineMismatch`'s, from the
    /// registry — this asserts it reaches the model, not that it was written
    /// twice.
    #[tokio::test]
    async fn an_engine_override_against_a_live_other_engine_names_switch_engine() {
        use crate::browser::engine::Engine;
        use crate::browser::testkit::{switch_fixture, SwitchFixture};
        // This test resolves `$ALEPH_HOME` through
        // `prepare_engine -> engine_handle_for -> launch_request_for_engine ->
        // browser_state_dir`, exactly the path `browser::home_guard_census`
        // exists for. It escapes that census only because the census scans
        // `src/browser/` and this file is under `src/builtin_tools/` — a gap
        // in the guard, not an exemption (判据 §3), recorded for Task 20.
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());
        let SwitchFixture { manager, .. } =
            switch_fixture(Engine::Obscura, Engine::Chromium, false).await;
        // `browser_open` runs the SSRF pre-check before it resolves an engine,
        // and on a host whose resolver hands back a carrier-grade / benchmark
        // address for `example.com` the call is blocked there — a refusal about
        // the NETWORK, which would make this test pass or fail on a fact it is
        // not about. The subject here is the engine refusal, so the guard is
        // opened for it explicitly rather than the URL being chosen to sneak
        // past whatever this machine's DNS happens to answer.
        manager.apply_policy(crate::browser::network_policy::SsrfConfig {
            block_private: false,
            ..Default::default()
        });
        let tool = BrowserOpenTool::new(Arc::clone(&manager));
        let result = tool
            .call(BrowserOpenArgs {
                url: "https://example.com".into(),
                profile: "default".into(),
                engine: Some(Engine::Chromium),
            })
            .await
            .unwrap();
        assert!(!result.success);
        let message = result.message.unwrap();
        assert!(message.contains("switch_engine"), "got: {message}");
        assert!(message.contains("obscura"), "got: {message}");
        // And nothing was resolved, so the engine field says "unknown" rather
        // than naming the one it failed to reach.
        assert!(result.engine.is_none(), "got: {:?}", result.engine);
    }
}
