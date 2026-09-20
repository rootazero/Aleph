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
/// headers, and user-agent on the active tab.
///
/// **Which profile serves what is stated exactly once**, in [`Self::DESCRIPTION`],
/// and pinned there by
/// `tests::the_description_excepts_exactly_the_axes_the_default_engine_refuses`.
/// A second sentence stood here saying it too ("Full support requires an
/// existing-session profile; the managed profile supports network state only")
/// and went false the day the default driver flipped, with nothing able to
/// notice — 判据 §1 in its expensive form, because a doc comment is what the
/// next reader checks and a `DESCRIPTION` is what the model obeys. Deleted
/// rather than corrected: two copies is the defect.
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
    /// request-level auth/identity writes, the same surface a cookie write is
    /// ("a cookie value is a credential by design"), and they were classified
    /// as [`ActionType::BrowserCookiesWrite`] for that reason.
    ///
    /// They now have their own [`ActionType::BrowserIdentityOverride`]: the
    /// trust surface was right, but a policy file is read by a person, and
    /// `browser_cookies_write` does not tell that person that it also governs
    /// header injection. The old key still governs this one unless the policy
    /// names it — see [`ActionType::inherited_from`] — so the rename cannot
    /// loosen an existing deployment.
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
        "Emulate environment overrides on the active tab. On a cdp profile (the default) \
         every override reaches the engine except network_condition: obscura has no such \
         method, so that one needs chromium or an existing-session profile, which serve \
         all six";
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
                ActionType::BrowserIdentityOverride,
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
        defaults.insert(ActionType::BrowserIdentityOverride, DefaultDecision::Deny);
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
        defaults.insert(ActionType::BrowserIdentityOverride, DefaultDecision::Allow);
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

    /// The DESCRIPTION's engine claim, derived from Task 0's measurements
    /// rather than from the sentence it is checking.
    ///
    /// **What stood here before did not merely miss the bug — it PINNED it.**
    /// The retired guard asserted the DESCRIPTION contained
    /// `"existing-session profile"`, i.e. it *required* the sentence that sends
    /// the model away from the default profile for five axes the default
    /// profile serves, and *required* `network_condition` — the one axis the
    /// default ENGINE refuses — to be named as the one that works. It was
    /// written when the default profile was `managed`, where every clause of it
    /// was true; `71d973920` flipped the default to `cdp` / `obscura` and
    /// nothing here could notice (B17 — a fact whose derivation is coarser than
    /// the world a change just created). It is also why `fa0c9c5e8`'s
    /// DESCRIPTION sweep walked past this file twice over: a needle on
    /// `"managed profiles only"` is fail-GREEN on this file's paraphrase (B7),
    /// and a needle that HAD hit would have met a test demanding the old words.
    ///
    /// **What this one is keyed to**, hardest-to-move first:
    ///
    /// 1. [`Engine::default`] — the exact thing whose change caused the bug. A
    ///    third flip re-reads the other half of the matrix, and the sentence has
    ///    to follow or this goes red.
    /// 2. `EMULATE_AXIS_METHODS`, itself tied to `EmulateOptions`' fields by
    ///    `cdp_backend::screenshot::tests::the_axis_map_covers_every_field_of_emulate_options`,
    ///    and to the wire by `…::emulate_sends_every_axis_to_its_own_cdp_method`.
    /// 3. `t0-support-matrix.json` — an on-disk measurement against real
    ///    binaries, not a claim in a comment.
    ///
    /// The one phrase-keyed assertion is `contains("cdp")`, and it buys exactly
    /// one thing: that the sentence still names a driver. It cannot see a
    /// rewording that keeps the word and changes the claim — the axis loops can,
    /// which is why they carry the weight.
    ///
    /// Neither loop is vacuous today: at HEAD the split is five answered
    /// (`Emulation.setEmulatedMedia` / `setGeolocationOverride` /
    /// `setCPUThrottlingRate` / `setUserAgentOverride`, `Network.setExtraHTTPHeaders`)
    /// against one refused (`Network.emulateNetworkConditions` —
    /// *"Unknown Network method"*). Both being empty is impossible; one of them
    /// becoming empty is a real change in the world, and the fixture is where it
    /// shows up.
    #[test]
    fn the_description_excepts_exactly_the_axes_the_default_engine_refuses() {
        use crate::browser::cdp_backend::EMULATE_AXIS_METHODS;
        use crate::browser::engine::capability::{t0_key, T0_SUPPORT_MATRIX};
        use crate::browser::engine::Engine;

        let matrix: serde_json::Value =
            serde_json::from_str(T0_SUPPORT_MATRIX).expect("t0-support-matrix.json is valid JSON");
        let key = t0_key(Engine::default());
        let half = matrix.get(key).unwrap_or_else(|| {
            panic!("t0-support-matrix.json has no `{key}` half, so the default engine is unprobed")
        });

        let mut answered: Vec<&str> = Vec::new();
        let mut refused: Vec<&str> = Vec::new();
        for (axis, method) in EMULATE_AXIS_METHODS {
            let protocol = half
                .get(method)
                .and_then(|entry| entry.get("protocol"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| {
                    panic!("t0-support-matrix.json[{key}] has no string `protocol` for {method}")
                });
            // `ok` is the fixture's own word for "the engine answered". Anything
            // else is a refusal text, and an unknown may only say "I don't
            // know" — which here means fail-closed onto the refused side
            // (判据 §8).
            if protocol == "ok" {
                answered.push(axis);
            } else {
                refused.push(axis);
            }
        }

        // Every way this sentence could spell an axis, DERIVED from the axis
        // rather than listed. Found by writing the mutation prediction for this
        // guard before running it: the first draft matched the snake_case key
        // only, and the retired sentence spells four of its five axes as prose
        // ("color scheme", "CPU throttle", "extra HTTP headers", "user-agent").
        // So the draft would have reddened on `geolocation` alone — one of five
        // — and would have gone GREEN on a rewording that kept the claim and
        // dropped the underscore. That is B7 on the guard's own negative half,
        // and the cure is derivation, not a longer list.
        fn spellings(axis: &str) -> Vec<String> {
            vec![
                axis.to_string(),
                axis.replace('_', " "),
                axis.replace('_', "-"),
            ]
        }

        let d = BrowserEmulateTool::DESCRIPTION;
        let lower = d.to_lowercase();
        for axis in &refused {
            assert!(
                spellings(axis).iter().any(|s| lower.contains(s)),
                "`{axis}` is refused by the default engine ({key}) and the DESCRIPTION \
                 does not name it in any spelling, so the model will try it and get a \
                 raw protocol error: {d}"
            );
        }
        for axis in &answered {
            for spelling in spellings(axis) {
                assert!(
                    !lower.contains(&spelling),
                    "`{axis}` REACHES the default engine ({key}), and the DESCRIPTION \
                     names it as {spelling:?}; that is how the retired sentence routed \
                     the model to profile='user', which needs the user's own Chrome plus \
                     npx: {d}"
                );
            }
        }
        assert!(
            d.contains("cdp"),
            "the sentence must still name the driver these reach: {d}"
        );
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
