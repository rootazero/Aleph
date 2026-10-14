//! `AllowlistToolService` — filters a parent `ToolService` using `AgentDef::is_tool_allowed`.
//!
//! Used by the subagent spawner so that a sub-agent can only see / execute
//! the tools its `AgentDef` permits. Delegates all passing calls to the inner
//! service unchanged.
//!
//! It is also where a delegated role's **identity** enters the signed ledger.
//! A subagent runs on the parent's `ScopedToolService` and under the parent's
//! `TURN_CONTEXT`, so the chokepoint would otherwise file its actions under
//! whoever spawned it. This wrapper is the one layer that knows the acting
//! `AgentDef`, and it sits inside each of the tasks the harness Act phase
//! spawns per tool call — which is exactly where the scope has to be opened
//! for the chokepoint to see it. See [`crate::identity::actor`].

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::agents::AgentDef;
use crate::builtin_tools::terminal::capabilities::is_tool_allowed_with_legacy_terminal_alias;
use crate::session::events::ToolOutput;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

pub struct AllowlistToolService {
    inner: Arc<dyn ToolService>,
    agent_def: Arc<AgentDef>,
}

impl AllowlistToolService {
    pub fn new(inner: Arc<dyn ToolService>, agent_def: Arc<AgentDef>) -> Self {
        Self { inner, agent_def }
    }

    /// The single policy answer for every face of this service. Delegates to
    /// the shared legacy-`terminal` adapter so the five observation verbs
    /// obey the same policy `tools.invoke` / `tools.effective` apply.
    fn is_allowed(&self, name: &str) -> bool {
        is_tool_allowed_with_legacy_terminal_alias(&self.agent_def, name)
    }

    /// Refuse a call the allowlist denies — recording it first.
    ///
    /// This gate sits **above** the `ScopedToolService` chokepoint, so its
    /// refusals never passed the one place tool refusals are ledgered: a
    /// denied sub-agent used to leave no trace on any chain, which is exactly
    /// the gap a signed operation ledger exists to close. The record is filed
    /// under the sub-agent's own identity (the same attribution its allowed
    /// calls get via [`crate::identity::as_actor`]), never the parent's.
    async fn deny(&self, name: &str, input: &Value) -> ToolError {
        let reason = format!("agent '{}' disallows this tool", self.agent_def.id);
        crate::tools::scoped::record_allowlist_refusal(&self.agent_def.id, name, input, &reason)
            .await;
        ToolError::PermissionDenied {
            name: name.to_string(),
            reason,
        }
    }
}

#[async_trait]
impl ToolService for AllowlistToolService {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        if !self.is_allowed(name) {
            return Err(self.deny(name, &input).await);
        }
        // The narrowed retrieval set rides down to the dispatch this delegates
        // to, so its footers (Layer 2, a tool's own offload) name only what
        // this agent may call.
        crate::tools::result_processing::with_recovery_tools(
            self.recovery_tools(),
            crate::identity::as_actor(&self.agent_def.id, self.inner.execute(name, input)),
        )
        .await
    }

    async fn execute_with_cancel(
        &self,
        name: &str,
        input: Value,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        // Run the allowlist check first so a disallowed tool returns the same
        // `PermissionDenied` error regardless of which call path the harness
        // took, then delegate to the inner cancel-aware path.
        if !self.is_allowed(name) {
            return Err(self.deny(name, &input).await);
        }
        crate::tools::result_processing::with_recovery_tools(
            self.recovery_tools(),
            crate::identity::as_actor(
                &self.agent_def.id,
                self.inner.execute_with_cancel(name, input, cancel),
            ),
        )
        .await
    }

    async fn execute_with_cancel_effective(
        &self,
        name: &str,
        input: Value,
        cancel: CancellationToken,
    ) -> (Result<ToolOutput, ToolError>, Option<Value>) {
        // Same allowlist gate as `execute_with_cancel`: a disallowed tool is
        // refused BEFORE dispatch, so it carries no replay-eligible marker.
        if !self.is_allowed(name) {
            return (Err(self.deny(name, &input).await), None);
        }
        crate::tools::result_processing::with_recovery_tools(
            self.recovery_tools(),
            crate::identity::as_actor(
                &self.agent_def.id,
                self.inner
                    .execute_with_cancel_effective(name, input, cancel),
            ),
        )
        .await
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        self.inner
            .list()
            .await
            .into_iter()
            .filter(|d| self.is_allowed(&d.name))
            .collect()
    }

    async fn dispatchable_list(&self) -> Vec<ToolDefinition> {
        // Forward the inner service's dispatchable set (visible + deferred
        // tier), filtered by the same allowlist as `list()`. The trait default
        // falls back to `list()`, which silently DROPS the parent
        // `ScopedToolService`'s deferred MCP names — so a subagent's correct
        // call to a deferred tool missed the name-repairer's Exact tier and
        // the Fuzzy tier was free to rewrite it into a different resident
        // tool, the exact regression `dispatchable_list` exists to prevent.
        self.inner
            .dispatchable_list()
            .await
            .into_iter()
            .filter(|d| self.is_allowed(&d.name))
            .collect()
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        if !self.is_allowed(name) {
            return None;
        }
        self.inner.describe(name).await
    }

    /// Forwarded unchanged: this wrapper narrows WHICH tools a child may call,
    /// never whether a call pauses for a human. That question is answered one
    /// layer down, by the parent `ScopedToolService` this delegates every
    /// execution to — so the tier a child's prompt states is the tier a
    /// child's call meets.
    fn enforced_exec_tier(&self) -> Option<crate::config::types::policies::ExecTier> {
        self.inner.enforced_exec_tier()
    }

    /// Narrowed, not forwarded: a retrieval tool this agent may not call is no
    /// recovery handle for it, whatever the parent can dispatch.
    fn recovery_tools(&self) -> crate::tools::result_processing::RecoveryTools {
        use crate::builtin_tools::{CtxSearchTool, FileReadTool};
        use crate::tools::AlephTool;
        let parent = self.inner.recovery_tools();
        let allowed = |name: &str| self.is_allowed(name);
        crate::tools::result_processing::RecoveryTools {
            ctx_search: parent.ctx_search && allowed(<CtxSearchTool as AlephTool>::NAME),
            file_read: parent.file_read && allowed(<FileReadTool as AlephTool>::NAME),
        }
    }

    fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
        // Filter the parent's metadata schema down to what this child agent
        // is allowed to see. Returning an empty slice here (the previous
        // behavior) silently hid every tool from the child LLM — `list()` /
        // `describe()` / `execute()` were properly filtered, but the LLM-facing
        // schema served by Orchestrator goes through `metadata_schema()`,
        // so subagents got an empty tool catalog and gave up after one turn.
        let inner = self.inner.metadata_schema();
        let filtered: Vec<crate::tool_metadata::ToolDefinition> = inner
            .iter()
            .filter(|d| self.is_allowed(&d.name))
            .cloned()
            .collect();
        std::sync::Arc::from(filtered)
    }

    async fn call_concurrency_claim(
        &self,
        name: &str,
        input: &Value,
    ) -> crate::tools::concurrency::ConcurrencyClaim {
        // Disallowed tools are whole-world exclusive (never parallel); otherwise
        // forward the inner service's bounded scope so disjoint-path mutations
        // still parallelize for subagents.
        if !self.is_allowed(name) {
            return crate::tools::concurrency::ConcurrencyClaim::global();
        }
        self.inner.call_concurrency_claim(name, input).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentDef, AgentMode};
    use crate::session::events::{ToolOutput, ToolOutputMetadata};
    use crate::tools::result_processing::RecoveryTools;
    use crate::tools::service::{ToolDefinition, ToolError, ToolService, ToolSource};
    use async_trait::async_trait;
    use serde_json::json;

    struct FakeTools;

    #[async_trait]
    impl ToolService for FakeTools {
        async fn execute(&self, name: &str, _: serde_json::Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: json!({ "tool": name }),
                metadata: ToolOutputMetadata::default(),
            })
        }

        async fn list(&self) -> Vec<ToolDefinition> {
            ["read", "write", "exec"]
                .iter()
                .map(|n| ToolDefinition {
                    name: (*n).into(),
                    description: "fake".into(),
                    input_schema: json!({}),
                    source: ToolSource::Builtin,
                    metadata: Default::default(),
                })
                .collect()
        }

        async fn describe(&self, name: &str) -> Option<ToolDefinition> {
            self.list().await.into_iter().find(|d| d.name == name)
        }
        fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
            // Mirror list() so tests can verify the AllowlistToolService
            // wrapper passes its own filter through metadata_schema().
            let defs: Vec<crate::tool_metadata::ToolDefinition> = ["read", "write", "exec"]
                .iter()
                .map(|n| {
                    crate::tool_metadata::ToolDefinition::new(
                        *n,
                        "fake",
                        json!({}),
                        crate::tool_metadata::ToolCategory::Builtin,
                    )
                })
                .collect();
            std::sync::Arc::from(defs)
        }
    }

    fn agent_with_allowed(tools: Vec<&str>) -> Arc<AgentDef> {
        let mut def = AgentDef::new("test", AgentMode::SubAgent);
        def.allowed_tools = tools.into_iter().map(String::from).collect();
        Arc::new(def)
    }

    /// A parent that can dispatch only some of the retrieval tools.
    struct GatedParent(RecoveryTools);

    #[async_trait]
    impl ToolService for GatedParent {
        async fn execute(&self, name: &str, _: serde_json::Value) -> Result<ToolOutput, ToolError> {
            Err(ToolError::NotFound { name: name.into() })
        }
        async fn list(&self) -> Vec<ToolDefinition> {
            vec![]
        }
        async fn describe(&self, _: &str) -> Option<ToolDefinition> {
            None
        }
        fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
            std::sync::Arc::from(Vec::new())
        }
        fn recovery_tools(&self) -> RecoveryTools {
            self.0
        }
    }

    /// A tool this agent may not call is no recovery handle for it, whatever
    /// the parent can dispatch: the wrapper narrows, it does not forward.
    #[test]
    fn recovery_tools_narrow_to_the_allowlist() {
        let svc = AllowlistToolService::new(
            Arc::new(FakeTools),
            agent_with_allowed(vec!["read", "file_read"]),
        );
        assert_eq!(
            svc.recovery_tools(),
            RecoveryTools {
                ctx_search: false,
                file_read: true,
            }
        );
        let svc = AllowlistToolService::new(Arc::new(FakeTools), agent_with_allowed(vec!["*"]));
        assert_eq!(svc.recovery_tools(), RecoveryTools::ALL);
    }

    /// The chain a subagent runs behind (allowlist → MCP scope view → parent):
    /// the parent's own narrowing must survive both wrappers.
    #[test]
    fn recovery_tools_survive_the_wrappers_a_subagent_runs_behind() {
        let parent = RecoveryTools {
            ctx_search: false,
            file_read: true,
        };
        let with_mcp: Arc<dyn ToolService> =
            Arc::new(crate::tools::mcp_scope_view::McpScopedToolService::new(
                Arc::new(GatedParent(parent)),
                Vec::new(),
            ));
        let child = AllowlistToolService::new(with_mcp, agent_with_allowed(vec!["*"]));
        assert_eq!(child.recovery_tools(), parent);
    }

    #[tokio::test]
    async fn allowed_tool_executes_delegates_to_inner() {
        let def = agent_with_allowed(vec!["read"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        let out = svc.execute("read", json!({})).await.unwrap();
        assert_eq!(out.value, json!({ "tool": "read" }));
    }

    #[tokio::test]
    async fn disallowed_tool_returns_permission_denied() {
        let def = agent_with_allowed(vec!["read"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        let err = svc.execute("exec", json!({})).await.unwrap_err();
        assert!(matches!(err, ToolError::PermissionDenied { .. }));
    }

    #[tokio::test]
    async fn empty_allowlist_denies_everything() {
        let def = agent_with_allowed(vec![]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        for name in ["read", "write", "exec"] {
            assert!(matches!(
                svc.execute(name, json!({})).await.unwrap_err(),
                ToolError::PermissionDenied { .. }
            ));
        }
    }

    #[tokio::test]
    async fn wildcard_allowlist_allows_everything() {
        let def = agent_with_allowed(vec!["*"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        for name in ["read", "write", "exec"] {
            assert!(svc.execute(name, json!({})).await.is_ok());
        }
    }

    #[tokio::test]
    async fn list_filters_to_allowed_subset() {
        let def = agent_with_allowed(vec!["read", "write"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        let list = svc.list().await;
        let names: Vec<_> = list.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["read", "write"]);
    }

    #[tokio::test]
    async fn describe_returns_none_for_disallowed() {
        let def = agent_with_allowed(vec!["read"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        assert!(svc.describe("read").await.is_some());
        assert!(svc.describe("exec").await.is_none());
    }

    /// Regression — `metadata_schema` previously returned an empty slice,
    /// hiding every tool from the LLM-facing tool pipeline. The wrapper
    /// must filter the inner schema using the same allowlist as `list()` /
    /// `describe()` / `execute()`.
    #[test]
    fn metadata_schema_filters_to_allowed_subset() {
        let def = agent_with_allowed(vec!["read", "write"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        let schema = svc.metadata_schema();
        let names: Vec<&str> = schema.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["read", "write"],
            "metadata_schema must surface the allowed subset, not the empty slice"
        );
    }

    /// Wildcard agent should see every parent tool through metadata_schema.
    #[test]
    fn metadata_schema_wildcard_passes_everything_through() {
        let def = agent_with_allowed(vec!["*"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        let schema = svc.metadata_schema();
        assert_eq!(schema.len(), 3);
    }

    /// Reports the ledger actor the inner service would see — i.e. exactly what
    /// `ScopedToolService::ledger_agent_id` reads at the chokepoint.
    struct ActorProbe;

    #[async_trait]
    impl ToolService for ActorProbe {
        async fn execute(&self, _: &str, _: serde_json::Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: json!({ "actor": crate::identity::current_actor() }),
                metadata: ToolOutputMetadata::default(),
            })
        }
        async fn list(&self) -> Vec<ToolDefinition> {
            vec![]
        }
        async fn describe(&self, _: &str) -> Option<ToolDefinition> {
            None
        }
        fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
            std::sync::Arc::from(Vec::new())
        }
    }

    /// The wiring the signed ledger depends on: a delegated role's calls must
    /// reach the inner service carrying that role's identity. Without it the
    /// chokepoint falls back to `TURN_CONTEXT`, which for a subagent is the
    /// *parent's* — and `SessionKey::Subagent::agent_id()` delegates to the
    /// parent too, so nothing downstream could have noticed.
    #[tokio::test]
    async fn the_acting_role_reaches_the_inner_service() {
        let def = agent_with_allowed(vec!["*"]);
        let svc = AllowlistToolService::new(Arc::new(ActorProbe), def);

        let out = svc.execute("anything", json!({})).await.unwrap();
        assert_eq!(out.value["actor"], json!("test"));

        let out = svc
            .execute_with_cancel("anything", json!({}), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            out.value["actor"],
            json!("test"),
            "the cancel-aware path is the one the harness actually takes"
        );
    }

    #[tokio::test]
    async fn a_denied_call_scopes_no_actor() {
        // The gate returns before delegating, so no actor scope is opened for
        // the inner service. The refusal itself no longer vanishes: `deny`
        // files a `ToolDenied` record under the sub-agent's own chain before
        // returning (no ledger is installed in this test, so that call is a
        // no-op here; the wired path is covered by the integration tests).
        let def = agent_with_allowed(vec![]);
        let svc = AllowlistToolService::new(Arc::new(ActorProbe), def);
        assert!(svc.execute("anything", json!({})).await.is_err());
        assert_eq!(crate::identity::current_actor(), None);
    }

    // ---------------------------------------------------------------------
    // A4 compat — a legacy `terminal` policy entry keeps governing the five
    // legacy observation verbs now that the child sees canonical names.
    // ---------------------------------------------------------------------

    const OBSERVATION_FIVE: [&str; 5] = ["list", "read", "status", "wait", "explain"];

    fn canonical(verb: &str) -> String {
        format!("terminal_sessions_{verb}")
    }

    /// Parent exposing the six canonical observation tools plus `read`;
    /// every delegated execution is counted and echoes name + input.
    struct TerminalParent {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl TerminalParent {
        fn names() -> Vec<String> {
            let mut v: Vec<String> = OBSERVATION_FIVE.iter().map(|a| canonical(a)).collect();
            v.push(canonical("attach"));
            v.push("read".into());
            v
        }

        fn count(&self, name: &str, input: Value) -> ToolOutput {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            ToolOutput {
                value: json!({ "tool": name, "input": input }),
                metadata: ToolOutputMetadata::default(),
            }
        }
    }

    #[async_trait]
    impl ToolService for TerminalParent {
        async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
            Ok(self.count(name, input))
        }
        async fn list(&self) -> Vec<ToolDefinition> {
            Self::names()
                .into_iter()
                .map(|name| ToolDefinition {
                    name,
                    description: "fake".into(),
                    input_schema: json!({}),
                    source: ToolSource::Builtin,
                    metadata: Default::default(),
                })
                .collect()
        }
        async fn describe(&self, name: &str) -> Option<ToolDefinition> {
            self.list().await.into_iter().find(|d| d.name == name)
        }
        fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
            let defs: Vec<_> = Self::names()
                .into_iter()
                .map(|n| {
                    crate::tool_metadata::ToolDefinition::new(
                        n,
                        "fake",
                        json!({}),
                        crate::tool_metadata::ToolCategory::Builtin,
                    )
                })
                .collect();
            std::sync::Arc::from(defs)
        }
    }

    fn agent(allowed: &[&str], denied: &[&str]) -> Arc<AgentDef> {
        let mut def = AgentDef::new("test", AgentMode::SubAgent);
        def.allowed_tools = allowed.iter().map(|s| (*s).to_owned()).collect();
        def.denied_tools = denied.iter().map(|s| (*s).to_owned()).collect();
        Arc::new(def)
    }

    fn wrapped(def: Arc<AgentDef>) -> (AllowlistToolService, Arc<std::sync::atomic::AtomicUsize>) {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let parent = Arc::new(TerminalParent {
            calls: calls.clone(),
        });
        (AllowlistToolService::new(parent, def), calls)
    }

    fn only_names(defs: &[ToolDefinition]) -> Vec<String> {
        let mut v: Vec<String> = defs
            .iter()
            .map(|d| d.name.clone())
            .filter(|n| n.starts_with("terminal_sessions_"))
            .collect();
        v.sort();
        v
    }

    fn sorted_five() -> Vec<String> {
        let mut v: Vec<String> = OBSERVATION_FIVE.iter().map(|a| canonical(a)).collect();
        v.sort();
        v
    }

    /// Drive every execution face for `name` and report whether each ran the
    /// inner service (`true`) or was refused with the exact existing
    /// `PermissionDenied` shape (`false`); inner call count is checked against it.
    async fn faces_allow(def: Arc<AgentDef>, name: &str) -> bool {
        let (svc, calls) = wrapped(def);
        let input = json!({ "session_id": "x" });
        let a = svc.execute(name, input.clone()).await;
        let b = svc
            .execute_with_cancel(name, input.clone(), CancellationToken::new())
            .await;
        let (c, _) = svc
            .execute_with_cancel_effective(name, input.clone(), CancellationToken::new())
            .await;
        let results = [a, b, c];
        let allowed = results[0].is_ok();
        for r in &results {
            assert_eq!(r.is_ok(), allowed, "every execute face must agree");
            match r {
                Ok(out) => assert_eq!(out.value, json!({ "tool": name, "input": input })),
                Err(ToolError::PermissionDenied { name: n, reason }) => {
                    assert_eq!(n, name);
                    assert_eq!(reason, "agent 'test' disallows this tool");
                }
                Err(other) => panic!("unexpected error {other:?}"),
            }
        }
        let expected_calls = if allowed { 3 } else { 0 };
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            expected_calls,
            "inner must run exactly once per allowed face and never when denied"
        );
        allowed
    }

    /// `allowed:[*], denied:[terminal]`: the five are refused on every face
    /// and hidden from list/describe/metadata; `attach` is untouched.
    #[tokio::test]
    async fn terminal_capability_legacy_deny_blocks_the_five_not_attach() {
        let def = || agent(&["*"], &["terminal"]);
        for verb in OBSERVATION_FIVE {
            assert!(
                !faces_allow(def(), &canonical(verb)).await,
                "legacy deny must refuse {verb}"
            );
        }
        assert!(faces_allow(def(), &canonical("attach")).await);
        assert!(faces_allow(def(), "read").await);

        let (svc, _) = wrapped(def());
        let expected = vec![canonical("attach")];
        assert_eq!(only_names(&svc.list().await), expected);
        assert_eq!(only_names(&svc.dispatchable_list().await), expected);
        assert!(svc.describe(&canonical("list")).await.is_none());
        assert!(svc.describe(&canonical("attach")).await.is_some());
        let schema: Vec<String> = svc
            .metadata_schema()
            .iter()
            .map(|d| d.name.clone())
            .filter(|n| n.starts_with("terminal_sessions_"))
            .collect();
        assert_eq!(schema, vec![canonical("attach")]);
    }

    /// `allowed:[terminal]`: the five run, `attach` is not granted.
    #[tokio::test]
    async fn terminal_capability_legacy_allow_admits_the_five_not_attach() {
        let def = || agent(&["terminal"], &[]);
        for verb in OBSERVATION_FIVE {
            assert!(
                faces_allow(def(), &canonical(verb)).await,
                "legacy allow must admit {verb}"
            );
        }
        assert!(!faces_allow(def(), &canonical("attach")).await);
        assert!(!faces_allow(def(), "read").await);

        let (svc, _) = wrapped(def());
        assert_eq!(only_names(&svc.list().await), sorted_five());
        assert_eq!(only_names(&svc.dispatchable_list().await), sorted_five());
        assert!(svc.describe(&canonical("explain")).await.is_some());
        assert!(svc.describe(&canonical("attach")).await.is_none());
    }

    /// Deny-first in both directions across the two spellings.
    #[tokio::test]
    async fn terminal_capability_canonical_deny_beats_legacy_allow_and_vice_versa() {
        for verb in OBSERVATION_FIVE {
            let name = canonical(verb);
            assert!(
                !faces_allow(agent(&["terminal"], &[&name]), &name).await,
                "canonical deny beats legacy allow ({verb})"
            );
            assert!(
                !faces_allow(agent(&[&name], &["terminal"]), &name).await,
                "legacy deny beats canonical allow ({verb})"
            );
        }
    }

    /// Non-regression and persistence: the other tools keep their answer, and
    /// the shared policy value is never rewritten.
    #[tokio::test]
    async fn terminal_capability_legacy_alias_leaves_policy_and_other_tools_alone() {
        let def = agent(&["terminal", "read"], &["terminal"]);
        let before = serde_json::to_vec(&*def).unwrap();
        assert!(faces_allow(agent(&["terminal", "read"], &[]), "read").await);
        assert!(faces_allow(def.clone(), "read").await);
        assert!(faces_allow(agent(&["read"], &["terminal"]), "read").await);
        let (svc, _) = wrapped(def.clone());
        let _ = svc.list().await;
        let _ = svc.execute(&canonical("list"), json!({})).await;
        assert_eq!(serde_json::to_vec(&*def).unwrap(), before);
    }
}
