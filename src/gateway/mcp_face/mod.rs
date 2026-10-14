//! Aleph as an MCP **server** (Streamable HTTP, `/mcp`) — spec §3.7.
//!
//! An interface face (R4): it translates `tools/list` / `tools/call` into the
//! same scoped tool dispatch a chat turn uses and nothing else. Wire payloads
//! come from `crate::mcp::protocol`; the JSON-RPC envelope is the gateway's
//! own `crate::gateway::protocol`. Nothing here is a second MCP implementation
//! (CLAUDE.md 禁用清单).

use std::collections::BTreeSet;

use arc_swap::ArcSwap;
use futures::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::executor::ToolRegistry;
use crate::mcp::protocol::{ToolAnnotations, ToolCallResult, ToolDefinition, ToolResultContent};
use crate::session::events::ToolOutput;
use crate::sync_primitives::Arc;
use crate::tool_metadata::{ToolHealthCache, UnifiedTool};
use crate::tools::service::{ToolError, ToolService};

pub mod auth;
pub mod config;
pub mod http;
pub mod protocol;
pub mod session;

pub use auth::McpCaller;
pub use config::McpFaceConfig;
pub use session::{McpClient, SessionTable, SessionView};

/// Is an Aleph operator surface (the Panel) connected right now? Boot wires
/// this to the presence probe so the face can mark a turn `unattended` when
/// nobody could answer an approval card.
pub type OperatorPresence = Arc<dyn Fn() -> BoxFuture<'static, bool> + Send + Sync>;

/// A `tools/call` for a name this face does not expose: not in the whitelist,
/// or in the whitelist but not registered on this host. A protocol error
/// (`-32602`), not an `isError` result — the client's model never called
/// anything.
#[derive(Debug, PartialEq, Eq)]
pub struct UnknownTool(pub String);

/// Appended to an `isError` text when the gate refused because nobody could
/// be asked. The gate's own text tells a chat model what to do next; an MCP
/// client's model needs to know where the card would have gone.
pub(crate) const MCP_APPROVAL_HINT: &str = "Approval could not be obtained: no Aleph operator \
surface (the Aleph Panel) is connected to receive the approval card. Ask the Aleph operator to \
open the Panel and retry, or to grant this action standing permission (`exec_grants` / \
`[policies.tool_permissions]`), or to expose only read-only tools to this MCP client.";

/// How `ConfirmDenial::lead` spells the arm where nobody was asked
/// (`({outcome:?})` of `ApprovalOutcome::Unavailable`). Pinned by a test.
const UNAVAILABLE_MARKER: &str = "(Unavailable)";

/// The face. One per process, installed by boot ([`install_mcp_face`]).
pub struct McpFace {
    enabled: bool,
    /// The whitelist. Swapped whole by `apply_expose` (P6.9); every call
    /// loads it once and works on that snapshot.
    expose: ArcSwap<BTreeSet<String>>,
    /// Non-plugin rows of the executor's runtime map, snapshotted at boot
    /// (builtins with full schemas). Plugin and MCP-bridged tools are read
    /// live per call — that is what `list_changed` promises.
    static_tools: Vec<UnifiedTool>,
    tool_registry: Arc<dyn ToolRegistry>,
    app_config: Option<Arc<tokio::sync::RwLock<crate::Config>>>,
    tool_health: Option<Arc<ToolHealthCache>>,
    operator_presence: OperatorPresence,
    sessions: SessionTable,
}

impl McpFace {
    #[must_use]
    pub fn new(
        config: &McpFaceConfig,
        tool_registry: Arc<dyn ToolRegistry>,
        static_tools: Vec<UnifiedTool>,
        app_config: Option<Arc<tokio::sync::RwLock<crate::Config>>>,
        tool_health: Option<Arc<ToolHealthCache>>,
        operator_presence: OperatorPresence,
    ) -> Self {
        Self {
            enabled: config.enabled,
            expose: ArcSwap::from_pointee(
                config.expose.iter().cloned().collect::<BTreeSet<String>>(),
            ),
            static_tools: static_tools
                .into_iter()
                .filter(|t| !matches!(t.source, crate::tool_metadata::ToolSource::Plugin { .. }))
                .collect(),
            tool_registry,
            app_config,
            tool_health,
            operator_presence,
            sessions: SessionTable::new(session::MCP_SESSION_IDLE_TTL),
        }
    }

    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub const fn sessions(&self) -> &SessionTable {
        &self.sessions
    }

    /// A snapshot of the whitelist (one `ArcSwap` load).
    #[must_use]
    pub fn expose(&self) -> Arc<BTreeSet<String>> {
        self.expose.load_full()
    }

    /// G5, runtime half: every exposed name the face cannot serve right now.
    /// ONE derivation, read by boot (P6.7, warns) and by live-apply (P6.9,
    /// warns again after a swap).
    #[must_use]
    pub fn unknown_expose(&self) -> Vec<String> {
        let known = self.known_tool_names();
        let expose = self.expose();
        let configured: Vec<String> = expose.iter().cloned().collect();
        config::unknown_expose_names(&configured, &known)
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    /// Every name the face could serve right now, ignoring `expose`.
    #[must_use]
    pub fn known_tool_names(&self) -> BTreeSet<String> {
        let mut names: BTreeSet<String> =
            self.static_tools.iter().map(|t| t.name.clone()).collect();
        if let Some(ext) = crate::extension::try_extension_manager() {
            names.extend(
                ext.active_plugin_tools_snapshot()
                    .into_iter()
                    .map(|t| t.name),
            );
        }
        if let Some(reg) =
            crate::gateway::execution_engine::tool_service_builder::mcp_tool_registry()
        {
            names.extend(reg.snapshot().keys().cloned());
        }
        names
    }

    /// The per-call registry: `expose ∩ (static builtins ∪ live plugin tools
    /// ∪ bridged MCP tools)`. Same three sources, same join order and same
    /// "existing names win" rule as `run_loop/inner.rs`.
    fn surface(
        &self,
    ) -> (
        Arc<crate::tools::runtime::LoopToolRegistry>,
        BTreeSet<String>,
    ) {
        let expose = self.expose();
        let mut unified: Vec<UnifiedTool> = self
            .static_tools
            .iter()
            .filter(|t| expose.contains(&t.name))
            .cloned()
            .collect();
        if let Some(ext) = crate::extension::try_extension_manager() {
            unified.extend(
                ext.active_plugin_tools_snapshot()
                    .into_iter()
                    .filter(|t| expose.contains(&t.name))
                    .map(
                        crate::gateway::execution_engine::tool_refresh::plugin_tool_to_unified_tool,
                    ),
            );
        }
        let mut registry = crate::tools::adapters::build_registry_from_tools(
            Arc::clone(&self.tool_registry),
            &unified,
        );
        let mut allowed: BTreeSet<String> = unified.iter().map(|t| t.name.clone()).collect();
        if let Some(reg) =
            crate::gateway::execution_engine::tool_service_builder::mcp_tool_registry()
        {
            let snapshot = reg.entries_snapshot();
            for (name, entry) in snapshot.iter() {
                if !expose.contains(name) || registry.get(name).is_some() {
                    continue;
                }
                registry.register(Box::new(
                    crate::tools::adapters::McpRegistryTool::from_registry_entry(
                        Arc::clone(&entry.handler),
                        &entry.descriptor,
                    ),
                ));
                allowed.insert(name.clone());
            }
        }
        (Arc::new(registry), allowed)
    }

    /// The scoped service for one call: tier and explicit policy read live
    /// from `[policies]` (global rung only — an MCP session has no
    /// per-session knob), hooks from the extension manager, attendance from
    /// the presence probe.
    async fn tool_service(
        &self,
        caller: &McpCaller,
        session: &SessionView,
    ) -> Arc<dyn ToolService> {
        let (registry, allowed) = self.surface();
        let (global_tier, explicit) = match self.app_config.as_ref() {
            Some(cfg) => {
                let guard = cfg.read().await;
                let perms = guard.policies.tool_permissions.clone();
                let all_default = perms.default == crate::extension::PermissionAction::Allow
                    && perms.overrides.is_empty();
                (
                    guard.policies.exec_tier,
                    (!all_default).then(|| crate::gateway::execution_engine::TurnToolPolicy {
                        policy: perms,
                        pregranted: Default::default(),
                    }),
                )
            }
            None => Default::default(),
        };
        let exec_tier = crate::gateway::execution_engine::resolve_exec_tier(
            global_tier,
            None,
            None,
            Some(caller.role),
        );
        let hook_executor = match crate::extension::try_extension_manager() {
            Some(ext) => {
                let snapshot = ext.hook_executor_snapshot().await;
                (snapshot.hook_count() > 0).then(|| Arc::new(snapshot))
            }
            None => None,
        };
        let unattended = !(self.operator_presence)().await;
        let turn_context = crate::tools::turn_context::TurnContext {
            session_key: session.aleph_key.clone(),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some(caller.role.to_string()),
            channel_tool_permissions: None,
            unattended,
            plan_gate: None,
            side_question: false,
        };
        crate::gateway::execution_engine::build_request_tool_service(
            registry,
            allowed,
            None,
            Some(turn_context),
            hook_executor,
            session.aleph_key.to_key_string(),
            explicit,
            exec_tier,
            unattended,
            &[],
            false,
            crate::tools::scoped::DeferredTools::empty(),
            self.tool_health.clone(),
        )
    }

    /// `tools/list`: what this caller may call, with full schemas.
    pub async fn list_tools(
        &self,
        caller: &McpCaller,
        session: &SessionView,
    ) -> Vec<ToolDefinition> {
        let svc = self.tool_service(caller, session).await;
        svc.list()
            .await
            .into_iter()
            .map(|d| ToolDefinition {
                name: d.name,
                description: Some(d.description),
                input_schema: Some(d.input_schema),
                annotations: d.metadata.idempotent.then(|| ToolAnnotations {
                    read_only_hint: Some(true),
                    ..ToolAnnotations::default()
                }),
            })
            .collect()
    }

    /// `tools/call`. `Err` only for a name the face does not serve; every
    /// execution outcome — including a gate refusal — is an `isError` result.
    pub async fn call_tool(
        &self,
        caller: &McpCaller,
        session: &SessionView,
        name: &str,
        arguments: Value,
    ) -> Result<ToolCallResult, UnknownTool> {
        if !self.expose().contains(name) {
            return Err(UnknownTool(name.to_string()));
        }
        let svc = self.tool_service(caller, session).await;
        // §6.2 — every production dispatch into the scoped gate is scoped by a
        // per-call identity, so the gate's park / decision / memo rows land
        // under this session's own key instead of being dropped for a missing
        // identity (session isolation). An MCP `tools/call` has no model turn
        // and no `call.id`, so the face mints one fresh turn id and one fresh
        // call id per call — the same per-future scoping the harness Act phase
        // uses (`with_call_identity`), keyed under the session the service was
        // built for in `tool_service`.
        let identity = crate::approval::CallIdentity {
            turn_id: uuid::Uuid::new_v4(),
            call_id: format!("mcp:{}:{}", session.id, uuid::Uuid::new_v4()),
        };
        let outcome = crate::gateway::caller_identity::with_caller_identity(
            Some(caller.role.to_string()),
            caller.user.clone(),
            caller.is_local,
            None,
            crate::approval::with_call_identity(
                Some(identity),
                svc.execute_with_cancel(name, arguments, CancellationToken::new()),
            ),
        )
        .await;
        match outcome {
            Ok(output) => Ok(output_to_result(output)),
            Err(ToolError::NotFound { name }) => Err(UnknownTool(name)),
            Err(err) => Ok(error_to_result(&err)),
        }
    }

    /// `notifications/tools/list_changed` to every open stream. Called by
    /// `lifecycle::after_transition` (P1). No-op when the face is disabled.
    pub fn notify_tools_list_changed(&self) {
        if !self.enabled {
            return;
        }
        let note = crate::gateway::protocol::JsonRpcRequest::notification(
            "notifications/tools/list_changed",
            None,
        );
        let delivered = self.sessions.broadcast(&note);
        tracing::debug!(delivered, "mcp_face: tools/list_changed broadcast");
    }
}

fn output_to_result(output: ToolOutput) -> ToolCallResult {
    let text = match output.value {
        Value::String(s) => s,
        other => serde_json::to_string_pretty(&other).unwrap_or_else(|_| other.to_string()),
    };
    let mut content = vec![ToolResultContent::Text { text }];
    content.extend(
        output
            .metadata
            .images
            .into_iter()
            .map(|img| ToolResultContent::Image {
                data: img.data,
                mime_type: img.mime_type,
            }),
    );
    ToolCallResult {
        content,
        is_error: Some(false),
    }
}

fn error_to_result(err: &ToolError) -> ToolCallResult {
    let mut text = err.to_string();
    if text.contains(UNAVAILABLE_MARKER) {
        text.push(' ');
        text.push_str(MCP_APPROVAL_HINT);
    }
    ToolCallResult {
        content: vec![ToolResultContent::Text { text }],
        is_error: Some(true),
    }
}

// ── process-global handle ────────────────────────────────────────────────

/// `FailsClosed`: without the face, `/mcp` is not mounted (404) and
/// `notify_tools_list_changed` has nobody to call — the feature is dead and
/// says nothing, which is the honest reading of "boot never installed it".
static MCP_FACE: CapabilitySlot<Arc<McpFace>> =
    CapabilitySlot::new("gateway/mcp-face", MissingSemantics::FailsClosed);

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn mcp_face_slot() -> &'static dyn SlotStatus {
    &MCP_FACE
}

/// Install the process-wide face. Idempotent (mirrors `spend::install_ledger`).
pub fn install_mcp_face(face: Arc<McpFace>) {
    let _ = MCP_FACE.install(face);
}

/// Record that boot reached the face and had nothing to install
/// (`[mcp_server] enabled = false`, or simulated mode with no tool registry).
pub fn decline_mcp_face(because: &'static str) {
    MCP_FACE.decline(because);
}

/// The installed face, if any. Lifecycle notifies through this.
pub fn try_mcp_face() -> Option<&'static Arc<McpFace>> {
    MCP_FACE.get()
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::error::Result as AlephResult;
    use crate::mcp::protocol::ToolResultContent;
    use crate::sync_primitives::Mutex;
    use crate::tool_metadata::{ToolSource, UnifiedTool};
    use serde_json::json;
    use std::collections::HashMap;

    /// Canned executor — the same shape `handlers/tools_invoke.rs` tests use.
    pub(crate) struct StubRegistry {
        results: Mutex<HashMap<String, AlephResult<Value>>>,
    }

    impl StubRegistry {
        pub(crate) fn with(pairs: Vec<(&str, AlephResult<Value>)>) -> Arc<dyn ToolRegistry> {
            let mut m = HashMap::new();
            for (k, v) in pairs {
                m.insert(k.to_string(), v);
            }
            Arc::new(Self {
                results: Mutex::new(m),
            })
        }
    }

    impl ToolRegistry for StubRegistry {
        fn get_tool(&self, _name: &str) -> Option<&UnifiedTool> {
            None
        }
        fn execute_tool(
            &self,
            tool_name: &str,
            _arguments: Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AlephResult<Value>> + Send + '_>>
        {
            let canned = self
                .results
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(tool_name)
                .map(|r| match r {
                    Ok(v) => Ok(v.clone()),
                    Err(e) => Err(crate::error::AlephError::tool(e.to_string())),
                })
                .unwrap_or_else(|| {
                    Err(crate::error::AlephError::tool(format!(
                        "unknown: {tool_name}"
                    )))
                });
            Box::pin(async move { canned })
        }
    }

    pub(crate) fn tool(name: &str) -> UnifiedTool {
        UnifiedTool::new(
            format!("builtin:{name}"),
            name,
            format!("{name} description"),
            ToolSource::Builtin,
        )
        .with_parameters_schema(
            json!({"type": "object", "properties": {"text": {"type": "string"}}}),
        )
    }

    /// A face over four stub tools: `echo` (string), `structured` (json),
    /// `hidden` (registered, usually not exposed), `broken` (fails).
    /// `operator_present` drives the attendance probe.
    pub(crate) fn face_with(expose: &[&str], enabled: bool, operator_present: bool) -> McpFace {
        let cfg = McpFaceConfig {
            enabled,
            expose: expose.iter().map(|s| (*s).to_string()).collect(),
        };
        McpFace::new(
            &cfg,
            StubRegistry::with(vec![
                ("echo", Ok(json!("hello"))),
                ("structured", Ok(json!({"rows": [1, 2]}))),
                ("hidden", Ok(json!("should never be reachable"))),
                ("broken", Err(crate::error::AlephError::tool("boom"))),
            ]),
            vec![
                tool("echo"),
                tool("structured"),
                tool("hidden"),
                tool("broken"),
            ],
            None,
            None,
            Arc::new(move || Box::pin(async move { operator_present })),
        )
    }

    pub(crate) fn face(expose: &[&str], enabled: bool) -> McpFace {
        face_with(expose, enabled, false)
    }

    pub(crate) fn operator() -> McpCaller {
        McpCaller {
            role: "operator",
            user: Some("u-owner".to_string()),
            is_local: true,
            device_id: None,
        }
    }

    pub(crate) fn session(face: &McpFace) -> SessionView {
        face.sessions().create(
            McpClient {
                client_name: "test".to_string(),
                client_version: "0".to_string(),
            },
            "2025-03-26",
        )
    }

    pub(crate) fn text_of(result: &ToolCallResult) -> String {
        match &result.content[0] {
            ToolResultContent::Text { text } => text.clone(),
            other => panic!("expected text, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn list_tools_is_the_exposed_subset_with_full_schemas() {
        let f = face(&["echo", "structured"], true);
        let s = session(&f);
        let mut names: Vec<String> = f
            .list_tools(&operator(), &s)
            .await
            .into_iter()
            .map(|t| t.name)
            .collect();
        names.sort();
        assert_eq!(names, vec!["echo".to_string(), "structured".to_string()]);
        let listed = f.list_tools(&operator(), &s).await;
        let echo = listed.iter().find(|t| t.name == "echo").unwrap();
        assert_eq!(
            echo.input_schema.as_ref().unwrap()["properties"]["text"]["type"],
            "string"
        );
        assert_eq!(echo.description.as_deref(), Some("echo description"));
    }

    #[tokio::test]
    async fn known_tool_names_is_what_the_face_can_serve_not_what_it_exposes() {
        let f = face(&["echo"], true);
        let known = f.known_tool_names();
        assert!(known.contains("hidden"), "known = everything registered");
        assert!(f.expose().contains("echo") && !f.expose().contains("hidden"));
    }

    #[test]
    fn unknown_expose_names_the_configured_strangers_and_nothing_else() {
        // G5 runtime half, on the face: boot (P6.7) and live-apply (P6.9)
        // both read this, so there is one derivation of "unknown".
        let f = face(&["echo", "no_such_tool"], true);
        assert_eq!(f.unknown_expose(), vec!["no_such_tool".to_string()]);
        assert!(face(&["echo"], true).unknown_expose().is_empty());
    }

    #[tokio::test]
    async fn a_string_value_is_returned_verbatim_as_text() {
        let f = face(&["echo"], true);
        let s = session(&f);
        let r = f
            .call_tool(&operator(), &s, "echo", json!({"text": "x"}))
            .await
            .unwrap();
        assert_eq!(text_of(&r), "hello");
        assert_eq!(r.is_error, Some(false));
    }

    #[test]
    fn a_json_value_is_pretty_printed() {
        // HEAD's scoped dispatch (`apply_layer_two`) flattens a structured tool
        // value to a compact JSON *string* before the face sees it, so the
        // integration path always arrives as `Value::String`. The pretty branch
        // is exercised directly: it is what a raw `Value::Object` becomes.
        let out = crate::session::events::ToolOutput {
            value: json!({"rows": [1, 2]}),
            metadata: crate::session::events::ToolOutputMetadata::default(),
        };
        let r = output_to_result(out);
        assert_eq!(
            text_of(&r),
            serde_json::to_string_pretty(&json!({"rows": [1, 2]})).unwrap()
        );
    }

    #[tokio::test]
    async fn a_tool_outside_expose_is_unknown_even_though_it_is_registered() {
        let f = face(&["echo"], true);
        let s = session(&f);
        assert_eq!(
            f.call_tool(&operator(), &s, "hidden", json!({}))
                .await
                .unwrap_err(),
            UnknownTool("hidden".to_string())
        );
    }

    #[tokio::test]
    async fn a_failing_tool_is_an_is_error_result_not_a_protocol_error() {
        let f = face(&["broken"], true);
        let s = session(&f);
        let r = f
            .call_tool(&operator(), &s, "broken", json!({}))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        assert!(text_of(&r).contains("boom"), "{}", text_of(&r));
        assert!(
            !text_of(&r).contains(MCP_APPROVAL_HINT),
            "a plain failure gets no approval hint"
        );
    }

    #[tokio::test]
    async fn an_empty_expose_lists_nothing_and_calls_nothing() {
        let f = face(&[], true);
        let s = session(&f);
        assert!(f.list_tools(&operator(), &s).await.is_empty());
        assert!(f
            .call_tool(&operator(), &s, "echo", json!({}))
            .await
            .is_err());
    }

    #[test]
    fn the_nobody_was_asked_marker_is_how_the_gate_spells_it() {
        // `ConfirmDenial::lead` (tools/scoped/dispatch.rs) prints `({outcome:?})`;
        // the face keys its approval hint on the `Unavailable` arm. Both halves
        // are pinned here so a rewording on either side turns this red.
        assert_eq!(
            format!(
                "({:?})",
                crate::sandbox::exec_approval::gate::ApprovalOutcome::Unavailable
            ),
            UNAVAILABLE_MARKER
        );
        let dispatch = include_str!("../../tools/scoped/dispatch.rs");
        assert!(
            dispatch.contains("({outcome:?})"),
            "ConfirmDenial::lead no longer prints the outcome"
        );
        let shaped = error_to_result(&ToolError::Execution {
            name: "x".to_string(),
            cause: "running `x` was not authorized — nobody was asked (Unavailable). Do not retry."
                .to_string(),
        });
        assert!(text_of(&shaped).ends_with(MCP_APPROVAL_HINT));
    }

    #[tokio::test]
    async fn notify_reaches_open_streams_and_is_a_no_op_when_disabled() {
        let f = face(&["echo"], true);
        let s = session(&f);
        let mut rx = f.sessions().attach_stream(&s.id).unwrap();
        f.notify_tools_list_changed();
        assert_eq!(
            rx.recv().await.unwrap().method,
            "notifications/tools/list_changed"
        );

        let off = face(&["echo"], false);
        let s2 = session(&off);
        let mut rx2 = off.sessions().attach_stream(&s2.id).unwrap();
        off.notify_tools_list_changed();
        assert!(rx2.try_recv().is_err(), "disabled face must not broadcast");
    }

    #[test]
    fn the_slot_is_on_the_roster_and_fails_closed() {
        let slot = mcp_face_slot();
        assert_eq!(slot.id(), "gateway/mcp-face");
        assert!(matches!(
            slot.missing(),
            crate::capability::MissingSemantics::FailsClosed
        ));
        assert!(crate::capability::ALL_SLOTS
            .iter()
            .any(|s| s.id() == "gateway/mcp-face"));
    }
}
