// Browser emulate tool — apply environment/device overrides to a tab
// (color scheme, geolocation, network/CPU throttling, HTTP headers, user-agent).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::browser::types::EmulateOptions;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `browser_emulate` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserEmulateArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Emulation overrides to apply (only the set fields take effect).
    #[serde(flatten)]
    pub options: EmulateOptions,
}

/// Output from the `browser_emulate` tool.
#[derive(Debug, Serialize)]
pub struct BrowserEmulateOutput {
    pub success: bool,
    pub message: Option<String>,
}

/// Emulates color scheme, geolocation, network/CPU throttling, extra HTTP
/// headers, and user-agent on the active tab. Full support requires an
/// existing-session profile; the managed profile supports network state only.
#[derive(Clone)]
pub struct BrowserEmulateTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserEmulateTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate the two identity-bearing overrides behind the approval policy.
    /// With no policy wired the tool behaves exactly as before.
    ///
    /// `extra_http_headers` attaches a caller-chosen header — canonically
    /// `Authorization: Bearer …` — to EVERY request the page makes from then
    /// on, and `user_agent` rewrites how the page identifies itself. Both are
    /// request-level auth/identity writes, which is the surface
    /// [`ActionType::BrowserCookiesWrite`] already names ("a cookie value is a
    /// credential by design"); reusing it means one policy knob covers every
    /// way an agent can attach a credential to page traffic. A dedicated
    /// `BrowserEmulate` variant would read better in a policy file but lives in
    /// `src/approval/types.rs`.
    ///
    /// The presentation-only overrides (color scheme, geolocation, network
    /// condition, CPU throttle) deliberately stay ungated: they carry no
    /// credential and gating them would train the user to click through.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

/// Whether these options write request-level identity/auth state — the subset
/// that earns the approval gate and the input-secret scan.
const fn carries_request_identity(options: &EmulateOptions) -> bool {
    options.extra_http_headers.is_some() || options.user_agent.is_some()
}

/// Audit target for the approval record: which identity surfaces are being
/// written, by name only. Header values are the credential and must not be
/// recorded — the gate exists because they can be one.
fn emulate_approval_target(options: &EmulateOptions) -> String {
    let mut parts = Vec::new();
    if let Some(headers) = &options.extra_http_headers {
        parts.push(format!(
            "headers: {}",
            headers.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    if options.user_agent.is_some() {
        parts.push("user_agent override".to_string());
    }
    parts.join("; ")
}

#[async_trait]
impl AlephTool for BrowserEmulateTool {
    const NAME: &'static str = "browser_emulate";
    const DESCRIPTION: &'static str =
        "Emulate environment overrides on the active tab. The managed (default) profile \
         supports only network_condition offline/online; color scheme, geolocation, CPU \
         throttle, extra HTTP headers and user-agent need an existing-session profile \
         (e.g. profile='user')";
    type Args = BrowserEmulateArgs;
    type Output = BrowserEmulateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Validate at the boundary before touching the browser.
        if let Err(reason) = args.options.validate() {
            return Ok(BrowserEmulateOutput {
                success: false,
                message: Some(reason),
            });
        }

        // Input-side secret scan runs BEFORE the approval check: deterministic
        // policy beats interactive approval (and is cheaper).
        //
        // This is NOT the carve-out `browser_cookies set` documents. A cookie
        // value legitimately IS the credential the caller means to install, so
        // scanning it would false-positive on the tool's core use. An extra
        // request header or a user-agent string is an environment override —
        // routing a secret out of the model's context into every request the
        // page makes is exfiltration, not the feature.
        if let Some(headers) = &args.options.extra_http_headers {
            for (name, value) in headers {
                if let Some(message) = super::check_input_secret_block(&self.manager, value) {
                    return Ok(BrowserEmulateOutput {
                        success: false,
                        message: Some(format!("header '{name}': {message}")),
                    });
                }
            }
        }
        if let Some(ua) = &args.options.user_agent {
            if let Some(message) = super::check_input_secret_block(&self.manager, ua) {
                return Ok(BrowserEmulateOutput {
                    success: false,
                    message: Some(message),
                });
            }
        }

        if carries_request_identity(&args.options) {
            if let Some(message) = super::check_browser_approval(
                self.approval_policy.as_ref(),
                ActionType::BrowserCookiesWrite,
                "emulate",
                // Header NAMES and the presence of a UA override are enough for
                // the audit trail; the values are the credential and never
                // reach the prompt (`approval_display_target` narrows the
                // prompt to "browser emulate" regardless, but the recorded
                // target must not carry them either).
                &emulate_approval_target(&args.options),
            )
            .await
            {
                return Ok(BrowserEmulateOutput {
                    success: false,
                    message: Some(message),
                });
            }
        }

        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.emulate(&tab_id, &args.options).await {
                Ok(()) => Ok(BrowserEmulateOutput {
                    success: true,
                    message: Some(format!("Emulation applied in profile '{}'", args.profile)),
                }),
                Err(e) => Ok(BrowserEmulateOutput {
                    success: false,
                    message: Some(format!(
                        "Emulate failed: {}",
                        super::backend_error_text(&self.manager, &e)
                    )),
                }),
            },
            Err(e) => Ok(BrowserEmulateOutput {
                success: false,
                message: Some(super::backend_error_text(&self.manager, &e)),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;
    use crate::browser::types::{ColorScheme, NetworkCondition};

    fn tool() -> BrowserEmulateTool {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        BrowserEmulateTool::new(manager)
    }

    fn headers(name: &str, value: &str) -> EmulateOptions {
        EmulateOptions {
            extra_http_headers: Some(
                [(name.to_string(), value.to_string())]
                    .into_iter()
                    .collect::<std::collections::BTreeMap<_, _>>(),
            ),
            ..Default::default()
        }
    }

    fn deny_policy() -> Arc<crate::approval::ConfigApprovalPolicy> {
        use crate::approval::{ConfigApprovalPolicy, DefaultDecision, PolicyConfig};
        let mut defaults = std::collections::HashMap::new();
        defaults.insert(ActionType::BrowserCookiesWrite, DefaultDecision::Deny);
        Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        }))
    }

    #[tokio::test]
    async fn test_extra_http_headers_are_gated_before_the_backend() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserEmulateTool::new(manager).with_approval_policy(deny_policy());
        let result = tool
            .call(BrowserEmulateArgs {
                profile: "default".into(),
                options: headers("Authorization", "Bearer abc"),
            })
            .await
            .unwrap();
        assert!(!result.success);
        let message = result.message.unwrap();
        // The denial — not a "no browser running" error — proves the gate ran
        // before the backend was constructed.
        assert!(
            message.contains("denied by approval policy"),
            "got: {message}"
        );
        // And the header value never reaches the model through the refusal.
        assert!(!message.contains("Bearer abc"), "got: {message}");
    }

    #[tokio::test]
    async fn test_user_agent_override_is_gated() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserEmulateTool::new(manager).with_approval_policy(deny_policy());
        let result = tool
            .call(BrowserEmulateArgs {
                profile: "default".into(),
                options: EmulateOptions {
                    user_agent: Some("Mozilla/5.0 (spoofed)".into()),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("denied by approval policy")),
            "got: {:?}",
            result.message
        );
    }

    #[tokio::test]
    async fn test_presentation_only_overrides_skip_the_gate() {
        // A dark-mode toggle carries no credential; gating it would train the
        // user to click through. It reaches the backend and fails there.
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserEmulateTool::new(manager).with_approval_policy(deny_policy());
        let result = tool
            .call(BrowserEmulateArgs {
                profile: "default".into(),
                options: EmulateOptions {
                    color_scheme: Some(ColorScheme::Dark),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| !m.contains("denied by approval policy")),
            "got: {:?}",
            result.message
        );
    }

    #[tokio::test]
    async fn test_secret_bearing_header_is_blocked_before_approval() {
        // Allow the action outright: the deterministic scan must still refuse,
        // proving it runs first — and before any backend lookup.
        use crate::approval::{ConfigApprovalPolicy, DefaultDecision, PolicyConfig};
        let mut defaults = std::collections::HashMap::new();
        defaults.insert(ActionType::BrowserCookiesWrite, DefaultDecision::Allow);
        let policy = Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        }));
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserEmulateTool::new(manager).with_approval_policy(policy);
        let result = tool
            .call(BrowserEmulateArgs {
                profile: "default".into(),
                options: headers(
                    "X-Api-Key",
                    "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789",
                ),
            })
            .await
            .unwrap();
        assert!(!result.success);
        let message = result.message.unwrap();
        assert!(message.contains("Blocked"), "expected refusal: {message}");
        assert!(message.contains("X-Api-Key"), "got: {message}");
        // The refusal names the rule but never echoes the secret value.
        assert!(!message.contains("sk-ant-api03"), "got: {message}");
    }

    #[test]
    fn approval_target_names_headers_but_never_their_values() {
        let target = emulate_approval_target(&headers("Authorization", "Bearer topsecret"));
        assert!(target.contains("Authorization"), "got: {target}");
        assert!(!target.contains("topsecret"), "got: {target}");
    }

    #[test]
    fn description_does_not_promise_what_the_default_profile_refuses() {
        // The managed (default) profile rejects every override except
        // network_condition — the description must say so rather than advertise
        // six overrides, five of which always fail.
        let d = BrowserEmulateTool::DESCRIPTION;
        assert!(d.contains("network_condition"), "got: {d}");
        assert!(d.contains("existing-session profile"), "got: {d}");
    }

    #[tokio::test]
    async fn test_empty_options_is_rejected_without_browser() {
        let result = tool()
            .call(BrowserEmulateArgs {
                profile: "default".into(),
                options: EmulateOptions::default(),
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.unwrap().contains("at least one option"));
    }

    #[tokio::test]
    async fn test_out_of_range_cpu_throttle_is_rejected() {
        let result = tool()
            .call(BrowserEmulateArgs {
                profile: "default".into(),
                options: EmulateOptions {
                    cpu_throttle: Some(99.0),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.unwrap().contains("out of range"));
    }

    #[tokio::test]
    async fn test_valid_options_degrade_without_browser() {
        // Valid request, but no browser is running → graceful failure, not panic.
        let result = tool()
            .call(BrowserEmulateArgs {
                profile: "default".into(),
                options: EmulateOptions {
                    color_scheme: Some(ColorScheme::Dark),
                    network_condition: Some(NetworkCondition::Offline),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    #[test]
    fn test_args_flatten_deserialization() {
        // Options flatten alongside `profile` in the JSON-RPC payload.
        let json = serde_json::json!({
            "profile": "user",
            "color_scheme": "dark",
            "geolocation": { "latitude": 37.77, "longitude": -122.41 },
            "network_condition": "fast3g"
        });
        let args: BrowserEmulateArgs = serde_json::from_value(json).unwrap();
        assert_eq!(args.profile, "user");
        assert!(matches!(args.options.color_scheme, Some(ColorScheme::Dark)));
        assert!(matches!(
            args.options.network_condition,
            Some(NetworkCondition::Fast3g)
        ));
        let geo = args.options.geolocation.unwrap();
        assert!((geo.latitude - 37.77).abs() < 1e-9);
    }
}
