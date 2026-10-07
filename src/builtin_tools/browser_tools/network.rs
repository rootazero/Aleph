// Browser network tool — reads the network request log of the current page,
// and manages the mock routes that intercept its requests (C1; spec:
// docs/superpowers/specs/2026-09-24-browser-network-mock-design.md §4).

use std::collections::BTreeMap;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::cdp_backend::routes::{NewRouteRule, RouteKind, RouteRuleInfo, RouteScope};
use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// spec §4's mock-body ceiling: 256 KiB, refused at registration so an
/// oversized body never enters the interception loop.
const MAX_MOCK_BODY_BYTES: usize = 256 * 1024;

/// Which `browser_network` operation to perform.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkAction {
    /// Read the network request log (the default — the pre-mock behaviour,
    /// so existing calls are unchanged byte-for-byte).
    #[default]
    Log,
    /// Register a mock route. Requires `url_contains`; `kind` (mock|abort,
    /// default mock), `status`/`headers`/`body` for a mock, `scope`
    /// (tab|profile, default tab), `note?`.
    MockAdd,
    /// List the profile's rules with hit counts and liveness.
    MockList,
    /// Remove one rule by `rule_id`.
    MockRemove,
    /// Clear rules by `scope` — REQUIRED, no default, so a clear is never
    /// ambiguous.
    MockClear,
}

/// What a `mock_add`ed rule does with a matched request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MockKind {
    /// Answer with `status` / `headers` / `body` (the default).
    Mock,
    /// Fail the request (ad-blocker shape). Carries no response material.
    Abort,
}

/// Who a rule applies to. `tab` (the default) binds the active tab and dies
/// with it; `profile` serves every tab of the profile, including ones opened
/// later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MockScope {
    Tab,
    Profile,
}

impl From<MockScope> for RouteScope {
    fn from(scope: MockScope) -> Self {
        match scope {
            MockScope::Tab => RouteScope::Tab,
            MockScope::Profile => RouteScope::Profile,
        }
    }
}

/// Arguments for the `browser_network` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserNetworkArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Which operation to perform (default: `log`).
    #[serde(default)]
    pub action: NetworkAction,
    /// `mock_add`: substring the request URL must contain (required,
    /// non-empty — an empty needle would intercept EVERY request).
    #[serde(default)]
    pub url_contains: Option<String>,
    /// `mock_add`: optional HTTP method filter (case-insensitive).
    #[serde(default)]
    pub method: Option<String>,
    /// `mock_add`: `mock` (default) answers with status/headers/body;
    /// `abort` fails the request.
    #[serde(default)]
    pub kind: Option<MockKind>,
    /// `mock_add` with kind=mock: response status (default 200).
    #[serde(default)]
    pub status: Option<u16>,
    /// `mock_add` with kind=mock: response headers.
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    /// `mock_add` with kind=mock: response body (string or JSON; max 256 KiB).
    #[serde(default)]
    pub body: Option<String>,
    /// `mock_add` (default tab) / `mock_clear` (required) / `mock_list`
    /// (optional filter): `tab` or `profile`.
    #[serde(default)]
    pub scope: Option<MockScope>,
    /// `mock_remove`: the rule id (`r1`, `r2`, …) as `mock_list` shows it.
    #[serde(default)]
    pub rule_id: Option<String>,
    /// `mock_add`: a free-text annotation `mock_list` carries back verbatim.
    #[serde(default)]
    pub note: Option<String>,
}

/// One validated call, with the model's raw optionality resolved away.
enum NetworkRequest {
    Log,
    MockAdd {
        rule: NewRouteRule,
        /// The approval gate's target — the needle, never the body.
        display_target: String,
    },
    MockList {
        scope: Option<MockScope>,
    },
    MockRemove {
        rule_id: String,
    },
    MockClear {
        scope: MockScope,
    },
}

impl BrowserNetworkArgs {
    /// Validate BEFORE the approval gate: a malformed call is a model mistake
    /// and must not consume a user approval (the ordering `browser_click`
    /// already states). Errors are messages, not `Err` — the family
    /// convention for a malformed call.
    fn validate(&self) -> std::result::Result<NetworkRequest, String> {
        match self.action {
            NetworkAction::Log => Ok(NetworkRequest::Log),
            NetworkAction::MockAdd => {
                // Review Focus #4: an empty needle matches EVERYTHING. The
                // registry refuses it too — this is the same refusal one
                // layer up, where the message reaches the model directly.
                let url_contains = self
                    .url_contains
                    .clone()
                    .filter(|u| !u.is_empty())
                    .ok_or("mock_add requires a non-empty url_contains — an empty needle would intercept EVERY request")?;
                let kind = match self.kind.unwrap_or(MockKind::Mock) {
                    MockKind::Abort => {
                        if self.status.is_some() || self.headers.is_some() || self.body.is_some() {
                            return Err("an abort rule carries no response material — drop status/headers/body".to_string());
                        }
                        RouteKind::Abort
                    }
                    MockKind::Mock => {
                        let body = self.body.clone().unwrap_or_default();
                        if body.len() > MAX_MOCK_BODY_BYTES {
                            return Err(format!(
                                "mock body is {} bytes, over the 256 KiB cap",
                                body.len()
                            ));
                        }
                        let headers: Vec<(String, String)> = self
                            .headers
                            .clone()
                            .unwrap_or_default()
                            .into_iter()
                            .collect();
                        RouteKind::Mock {
                            status: self.status.unwrap_or(200),
                            headers,
                            body: body.into_bytes(),
                        }
                    }
                };
                Ok(NetworkRequest::MockAdd {
                    rule: NewRouteRule {
                        url_contains: url_contains.clone(),
                        method: self.method.clone(),
                        kind,
                        scope: self.scope.unwrap_or(MockScope::Tab).into(),
                        note: self.note.clone(),
                    },
                    display_target: url_contains,
                })
            }
            NetworkAction::MockList => Ok(NetworkRequest::MockList { scope: self.scope }),
            NetworkAction::MockRemove => {
                let rule_id = self
                    .rule_id
                    .clone()
                    .filter(|r| !r.is_empty())
                    .ok_or("mock_remove requires rule_id — mock_list shows the live ids")?;
                Ok(NetworkRequest::MockRemove { rule_id })
            }
            NetworkAction::MockClear => {
                let scope = self.scope.ok_or(
                    "mock_clear requires an explicit scope (\"tab\" or \"profile\") — there is no default, so a clear is never ambiguous",
                )?;
                Ok(NetworkRequest::MockClear { scope })
            }
        }
    }
}

/// Output from the `browser_network` tool.
#[derive(Debug, Serialize)]
pub struct BrowserNetworkOutput {
    pub success: bool,
    /// The request log (action=log); empty for the mock actions.
    pub requests: String,
    /// The rule(s) the call produced or reports: the one registered
    /// (mock_add), the table (mock_list), the one removed (mock_remove).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<RouteRuleInfo>>,
    pub message: Option<String>,
}

impl BrowserNetworkOutput {
    fn failed(message: String) -> Self {
        Self {
            success: false,
            requests: String::new(),
            rules: None,
            message: Some(message),
        }
    }
}

/// Reads the network request log of the current browser page for debugging,
/// and manages the mock routes that intercept its requests.
#[derive(Clone)]
pub struct BrowserNetworkTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserNetworkTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate the mutating mock actions (mock_add / mock_remove / mock_clear)
    /// behind the approval policy — a mock route decides what a page's
    /// requests RECEIVE, the same family of page-visible change as
    /// `browser_evaluate`. `log` and `mock_list` are reads and skip the
    /// gate. With no policy wired the tool behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

#[async_trait]
impl AlephTool for BrowserNetworkTool {
    const NAME: &'static str = "browser_network";
    const DESCRIPTION: &'static str =
        "Read the current page's network request log (action=log, default), or manage mock routes \
         intercepting its requests: mock_add (url_contains; kind=mock|abort; scope=tab|profile), \
         mock_list, mock_remove (rule_id), mock_clear (scope required). Mock routes need a \
         driver=\"cdp\" profile.";
    type Args = BrowserNetworkArgs;
    type Output = BrowserNetworkOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Validate before the approval gate: a malformed call must not
        // consume a user approval (see `validate`).
        let request = match args.validate() {
            Ok(request) => request,
            Err(message) => return Ok(BrowserNetworkOutput::failed(message)),
        };
        let gated = match &request {
            NetworkRequest::MockAdd { display_target, .. } => {
                Some(("mock_add", display_target.as_str()))
            }
            NetworkRequest::MockRemove { rule_id } => Some(("mock_remove", rule_id.as_str())),
            NetworkRequest::MockClear { scope } => Some((
                "mock_clear",
                match scope {
                    MockScope::Tab => "tab",
                    MockScope::Profile => "profile",
                },
            )),
            NetworkRequest::Log | NetworkRequest::MockList { .. } => None,
        };
        if let Some((action, target)) = gated {
            if let Some(message) = super::check_browser_approval(
                self.approval_policy.as_ref(),
                ActionType::BrowserNetworkMock,
                action,
                target,
            )
            .await
            {
                return Ok(BrowserNetworkOutput::failed(message));
            }
        }
        let (backend, tab_id) =
            match super::make_backend_and_tab_guarded(&self.manager, &args.profile).await {
                Ok(pair) => pair,
                Err(e) => {
                    return Ok(BrowserNetworkOutput::failed(super::backend_error_text(
                        &self.manager,
                        &e,
                    )));
                }
            };
        match request {
            NetworkRequest::Log => match backend.network_log(&tab_id).await {
                Ok(requests) => {
                    let line_count = requests.lines().count();
                    // Network log entries include URLs/headers from the page;
                    // untrusted (redact credentials, then wrap). Append-ordered
                    // log: head+tail truncation keeps the newest requests.
                    let wrapped = super::redact_and_wrap_log(&self.manager, &requests);
                    Ok(BrowserNetworkOutput {
                        success: true,
                        requests: wrapped,
                        rules: None,
                        message: Some(format!("{line_count} network log line(s)")),
                    })
                }
                Err(e) => Ok(BrowserNetworkOutput::failed(format!(
                    "Network log read failed: {}",
                    super::backend_error_text(&self.manager, &e)
                ))),
            },
            NetworkRequest::MockAdd { rule, .. } => match backend.route_add(&tab_id, rule).await {
                Ok(info) => {
                    let scope = match info.scope {
                        RouteScope::Tab => "tab",
                        RouteScope::Profile => "profile",
                    };
                    Ok(BrowserNetworkOutput {
                        success: true,
                        requests: String::new(),
                        rules: Some(vec![info.clone()]),
                        message: Some(format!(
                            "mock route {} registered ({scope} scope); interception is armed",
                            info.id
                        )),
                    })
                }
                Err(e) => Ok(BrowserNetworkOutput::failed(format!(
                    "mock_add failed: {}",
                    super::backend_error_text(&self.manager, &e)
                ))),
            },
            NetworkRequest::MockList { scope } => match backend.route_list().await {
                Ok(mut rules) => {
                    if let Some(scope) = scope {
                        let scope: RouteScope = scope.into();
                        rules.retain(|r| r.scope == scope);
                    }
                    Ok(BrowserNetworkOutput {
                        success: true,
                        requests: String::new(),
                        message: Some(format!("{} mock route(s)", rules.len())),
                        rules: Some(rules),
                    })
                }
                Err(e) => Ok(BrowserNetworkOutput::failed(format!(
                    "mock_list failed: {}",
                    super::backend_error_text(&self.manager, &e)
                ))),
            },
            NetworkRequest::MockRemove { rule_id } => match backend.route_remove(&rule_id).await {
                Ok(info) => Ok(BrowserNetworkOutput {
                    success: true,
                    requests: String::new(),
                    message: Some(format!(
                        "mock route {} removed (served {} hit(s))",
                        info.id, info.hits
                    )),
                    rules: Some(vec![info]),
                }),
                Err(e) => Ok(BrowserNetworkOutput::failed(format!(
                    "mock_remove failed: {}",
                    super::backend_error_text(&self.manager, &e)
                ))),
            },
            NetworkRequest::MockClear { scope } => {
                let label = match scope {
                    MockScope::Tab => "tab",
                    MockScope::Profile => "profile",
                };
                match backend.route_clear(&tab_id, scope.into()).await {
                    Ok(cleared) => Ok(BrowserNetworkOutput {
                        success: true,
                        requests: String::new(),
                        rules: None,
                        message: Some(format!("cleared {cleared} {label}-scope mock route(s)")),
                    }),
                    Err(e) => Ok(BrowserNetworkOutput::failed(format!(
                        "mock_clear failed: {}",
                        super::backend_error_text(&self.manager, &e)
                    ))),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    fn tool() -> BrowserNetworkTool {
        BrowserNetworkTool::new(Arc::new(
            ProfileManager::new(BrowserSystemConfig::default()),
        ))
    }

    fn mock_add_args() -> BrowserNetworkArgs {
        BrowserNetworkArgs {
            profile: "default".into(),
            action: NetworkAction::MockAdd,
            url_contains: Some("/api/".into()),
            method: None,
            kind: None,
            status: None,
            headers: None,
            body: Some("{}".into()),
            scope: None,
            rule_id: None,
            note: None,
        }
    }

    #[tokio::test]
    async fn test_network_read_degrades_without_browser() {
        let tool = tool();
        let result = tool
            .call(BrowserNetworkArgs {
                profile: "default".into(),
                ..log_args()
            })
            .await
            .unwrap();
        assert!(!result.success); // No browser running
    }

    /// Backward compatibility is the whole reason `action` has a default: a
    /// call shaped exactly like pre-mock `browser_network` (no `action` key)
    /// must parse to `log` and keep its old behaviour byte-for-byte.
    #[test]
    fn the_default_action_is_log_so_existing_calls_are_unchanged() {
        let args: BrowserNetworkArgs = serde_json::from_str("{}").expect("parses");
        assert_eq!(args.action, NetworkAction::Log);
        assert_eq!(args.profile, "default");
    }

    fn log_args() -> BrowserNetworkArgs {
        BrowserNetworkArgs {
            profile: "default".into(),
            action: NetworkAction::Log,
            url_contains: None,
            method: None,
            kind: None,
            status: None,
            headers: None,
            body: None,
            scope: None,
            rule_id: None,
            note: None,
        }
    }

    /// Review Focus #4: an empty needle matches EVERY request — almost always
    /// a model typo. Refused at the input side, before any backend is built.
    #[tokio::test]
    async fn mock_add_rejects_an_empty_url_contains() {
        let tool = tool();
        let mut args = mock_add_args();
        args.url_contains = Some(String::new());
        let result = tool.call(args).await.unwrap();
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("url_contains")),
            "got: {:?}",
            result.message
        );
    }

    /// spec §4: the mock body is capped at 256 KiB, refused at registration
    /// so an oversized body never enters the interception loop.
    #[tokio::test]
    async fn mock_add_rejects_a_body_over_256_kib() {
        let tool = tool();
        let mut args = mock_add_args();
        args.body = Some("x".repeat(256 * 1024 + 1));
        let result = tool.call(args).await.unwrap();
        assert!(!result.success);
        assert!(
            result.message.as_deref().is_some_and(|m| m.contains("256")),
            "got: {:?}",
            result.message
        );
        // The boundary value itself is admitted to validation (it then fails
        // later, on the missing browser — validation did not refuse it).
        let mut args = mock_add_args();
        args.body = Some("x".repeat(256 * 1024));
        let result = tool.call(args).await.unwrap();
        assert!(
            result
                .message
                .as_deref()
                .is_none_or(|m| !m.contains("256 KiB")),
            "the exact cap must pass the size gate: {:?}",
            result.message
        );
    }

    /// 防误清: `mock_clear` with no scope would be one ambiguous call away
    /// from wiping rules the model did not mean — the scope is REQUIRED, no
    /// default.
    #[tokio::test]
    async fn mock_clear_requires_an_explicit_scope() {
        let tool = tool();
        let result = tool
            .call(BrowserNetworkArgs {
                action: NetworkAction::MockClear,
                ..log_args()
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("scope")),
            "got: {:?}",
            result.message
        );
    }

    /// An abort rule has no response to carry: accepting `status`/`body` for
    /// one would drop model-supplied bytes silently (判据 §8's quiet-success
    /// shape, inverted).
    #[tokio::test]
    async fn an_abort_rule_carries_no_response_material() {
        let tool = tool();
        let mut args = mock_add_args();
        args.kind = Some(MockKind::Abort);
        let result = tool.call(args).await.unwrap();
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("abort")),
            "got: {:?}",
            result.message
        );
    }

    #[tokio::test]
    async fn mock_remove_requires_a_rule_id() {
        let tool = tool();
        let result = tool
            .call(BrowserNetworkArgs {
                action: NetworkAction::MockRemove,
                ..log_args()
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("rule_id")),
            "got: {:?}",
            result.message
        );
    }

    /// click.rs's ordering contract, applied to the mock gate: a malformed
    /// call is a model mistake and must not consume a user approval.
    #[tokio::test]
    async fn mock_add_validates_before_the_approval_gate() {
        use crate::approval::{ConfigApprovalPolicy, DefaultDecision, PolicyConfig};
        use std::collections::HashMap;
        let mut defaults = HashMap::new();
        defaults.insert(ActionType::BrowserNetworkMock, DefaultDecision::Deny);
        let policy = Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        }));
        let tool = BrowserNetworkTool::new(Arc::new(ProfileManager::new(
            BrowserSystemConfig::default(),
        )))
        .with_approval_policy(policy);

        let mut args = mock_add_args();
        args.url_contains = Some(String::new());
        let result = tool.call(args).await.unwrap();
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("url_contains") && !m.contains("denied")),
            "validation speaks before the gate: {:?}",
            result.message
        );
    }

    /// A policy that denies mock routes stops the call BEFORE any backend
    /// exists: no browser is contacted to be told no.
    #[tokio::test]
    async fn a_denied_mock_add_never_reaches_the_backend() {
        use crate::approval::{ConfigApprovalPolicy, DefaultDecision, PolicyConfig};
        use std::collections::HashMap;
        let mut defaults = HashMap::new();
        defaults.insert(ActionType::BrowserNetworkMock, DefaultDecision::Deny);
        let policy = Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        }));
        let tool = BrowserNetworkTool::new(Arc::new(ProfileManager::new(
            BrowserSystemConfig::default(),
        )))
        .with_approval_policy(policy);

        let result = tool.call(mock_add_args()).await.unwrap();
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("denied")),
            "got: {:?}",
            result.message
        );
    }

    /// `mock_list` is a READ: it stays outside the approval gate, and with no
    /// browser running it degrades exactly like `log` does.
    #[tokio::test]
    async fn mock_list_degrades_without_a_browser() {
        let tool = tool();
        let result = tool
            .call(BrowserNetworkArgs {
                action: NetworkAction::MockList,
                ..log_args()
            })
            .await
            .unwrap();
        assert!(!result.success);
    }
}
