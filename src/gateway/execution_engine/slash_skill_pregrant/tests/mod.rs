//! `/name` through the production steps a turn takes, in their order: a
//! human-facing surface's stamp (`ExecutionEngine::stamp_slash_mode`, as the
//! `chat.send` / `agent.run` handlers call it) or none, then what `execute()`
//! does — the ingress strip (`slash_skill_scope::forget_at_ingress`), the
//! safety net's unattested stamp, the split ([`split_with`]) — the turn's
//! permission resolution (`resolve_turn_permissions`), and the tool gate the
//! run loop builds from that resolution (`build_request_tool_service`, with
//! the run loop's own narrowing). The gate has no approval channel, so a call
//! the tier would card is refused and a lifted one reaches its stub.
//!
//! Hermetic: `$ALEPH_HOME` and `$HOME` point into a temp dir for the whole
//! test (`HomeEnvGuards`), so no user-level root read here is the real one.
//! The fire sites no behavioural test can reach are pinned by source at the
//! end.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use super::super::slash_skill_scope;
use super::super::turn_permissions::{TurnPermissions, TurnToolPolicy};
use super::super::RunRequest;
use super::split_with;
use crate::config::types::policies::{ExecTier, EXEC_TIER_SESSION_KEY};
use crate::domain::skill::{PluginId, SkillSource};
use crate::extension::visibility::ScopeKey;
use crate::gateway::agent_instance::AgentInstance;
use crate::runtimes::post_install::HomeEnvGuards;
use crate::skill::{SkillInfo, SkillSystem};
use crate::sync_primitives::Arc;
use crate::tool_metadata::ToolCatalog;
use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult};
use crate::tools::service::ToolService;
use crate::tools::AlephTool;
use crate::utils::paths::{publish_plugin_skill_dirs, PublishedPluginSkillDir};

/// The unit-test engine (`execution_engine::tests::test_engine`).
pub(super) type TestEngine = super::super::engine::ExecutionEngine<
    crate::thinker::SingleProviderRegistry,
    super::super::tests::EmptyToolRegistry,
>;

pub(super) const SKILL: &str = "p411-probe";
pub(super) const AGENT: &str = "p411-agent";
pub(super) const PLUGIN: &str = "p411-plug";
/// The tools the gate below holds. The first three are mutating, so the
/// `ask` tier cards each of them unless something lifts it; `self_config`
/// and `file_ops` carry argument-level rules.
pub(super) const TOOLS: [&str; 5] = ["bash", "file_write", "file_edit", "self_config", "file_ops"];

/// A leaf that always succeeds: reaching it means every gate let the call by.
pub(super) struct Stub(&'static str);

#[async_trait::async_trait]
impl LoopTool for Stub {
    fn name(&self) -> &str {
        self.0
    }
    fn description(&self) -> &str {
        "stub"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> ToolResult {
        ToolResult::Success { output: json!({}) }
    }
}

/// The real `skill_manage` behind the gate.
struct RealSkillManage(crate::builtin_tools::skill_manage::SkillManageTool);

#[async_trait::async_trait]
impl LoopTool for RealSkillManage {
    fn name(&self) -> &str {
        "skill_manage"
    }
    fn description(&self) -> &str {
        "the real skill_manage"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, input: Value, _cancel: CancellationToken) -> ToolResult {
        match self.0.call_json(input).await {
            Ok(output) => ToolResult::Success { output },
            Err(e) => ToolResult::Error {
                error: e.to_string(),
                retryable: false,
            },
        }
    }
}

pub(super) fn skill_md(name: &str, allowed: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: a pre-grant probe\nallowed-tools: {allowed}\n---\n\
         Do the thing.\n"
    )
}

pub(super) fn write_skill(root: &Path, name: &str, allowed: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), skill_md(name, allowed)).unwrap();
}

/// Where a request's slash mode comes from.
#[derive(Clone, Copy)]
pub(super) enum Stamp {
    /// The `chat.send` / `agent.run` handlers' `stamp_slash_mode`: typed.
    Handler,
    /// Nobody before `execute()`: its safety net stamps it, unattested — the
    /// shape of `sessions_send`, a team task, cron, heartbeat, A2A.
    SafetyNet,
}

/// Temp `$ALEPH_HOME` + `$HOME` (so both user-level roots are temp dirs), a
/// git project the run works in, a fresh `SkillSystem`, the slash catalog and
/// a command parser over it, and the agent.
pub(super) struct World {
    // Declaration order is drop order: restore the env, then delete the dirs.
    _env: HomeEnvGuards,
    tmp: TempDir,
    /// `~/.aleph/skills`.
    user: PathBuf,
    /// `~/.claude/skills`.
    claude_user: PathBuf,
    project: PathBuf,
    skills: SkillSystem,
    catalog: Arc<ToolCatalog>,
    /// Holds the command parser: the stamps run on it.
    stamper: TestEngine,
    agent: Arc<AgentInstance>,
}

impl World {
    async fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let aleph_home = tmp.path().join("aleph");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&aleph_home).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let env = HomeEnvGuards::acquire_and_set(&aleph_home, &home);
        let project = tmp.path().join("repo");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        let project = project.canonicalize().unwrap();
        let user = crate::utils::paths::get_skills_dir().unwrap();
        std::fs::create_dir_all(&user).unwrap();
        let claude_user = home.join(".claude").join("skills");
        std::fs::create_dir_all(&claude_user).unwrap();
        let agent = super::super::tests::gate_test_agent(&tmp, AGENT).await;
        let catalog = Arc::new(ToolCatalog::new());
        let parser = Arc::new(crate::command::CommandParser::new(Arc::clone(&catalog)));
        let stamper = super::super::tests::test_engine()
            .with_command_parser_cell(Arc::new(tokio::sync::RwLock::new(Some(parser))));
        Self {
            _env: env,
            tmp,
            user,
            claude_user,
            project,
            skills: SkillSystem::new(),
            catalog,
            stamper,
            agent,
        }
    }

    /// `<project>/<flavour>/skills` — `.claude` or `.aleph`.
    fn project_skills(&self, flavour: &str) -> PathBuf {
        self.project.join(flavour).join("skills")
    }

    /// What boot does: scan the base dirs (plus whatever plugin dirs are
    /// published) and register every scanned skill as a slash row
    /// (`SkillInfo::from(&manifest)`, validated by `register_skills`).
    async fn scan(&self, dirs: &[PathBuf]) {
        self.skills.init(dirs.to_vec()).await;
        let infos: Vec<SkillInfo> = self
            .skills
            .list_skills()
            .await
            .iter()
            .map(SkillInfo::from)
            .collect();
        self.catalog.register_skills(&infos).await;
    }

    /// A plugin command's slash row.
    async fn register_command(&self, name: &str, allowed: &[&str]) {
        let rejected = self
            .catalog
            .register_plugin_commands(
                &[SkillInfo {
                    id: format!("{PLUGIN}:{name}"),
                    name: name.into(),
                    description: "a plugin command".into(),
                    scope: crate::domain::skill::PromptScope::System,
                    version: None,
                    allowed_tools: Some(names(allowed)),
                    argument_hint: None,
                    plugin_id: Some(PLUGIN.into()),
                }],
                &|_| false,
            )
            .await;
        assert!(rejected.is_empty(), "{rejected:?}");
    }

    /// `input` from `role`, stamped by `stamp`, with `extra` metadata.
    async fn request(
        &self,
        input: &str,
        role: Option<&str>,
        stamp: Stamp,
        extra: &[(&str, &str)],
    ) -> RunRequest {
        let mut request = request(input, role, &self.project);
        for (key, value) in extra {
            request.metadata.insert(key.to_string(), value.to_string());
        }
        if let Stamp::Handler = stamp {
            self.stamper
                .stamp_slash_mode(&request.input, &mut request.metadata)
                .await;
        }
        request
    }

    /// `/p411-probe go`, typed by `role` into a handler.
    async fn skill_request(&self, role: Option<&str>) -> RunRequest {
        self.request(&format!("/{SKILL} go"), role, Stamp::Handler, &[])
            .await
    }

    /// The tool gate an operator's plain turn at `tier` builds, holding the
    /// real `skill_manage` over this world's skills.
    async fn skill_manage_gate(&self, tier: ExecTier) -> Arc<dyn ToolService> {
        let plain = self.request("hello", None, Stamp::Handler, &[]).await;
        let t = self.turn(&engine(None), at(tier, plain)).await;
        assert_eq!(t.permissions.tier, tier, "precondition");
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(RealSkillManage(
            crate::builtin_tools::skill_manage::SkillManageTool::new(self.skills.clone())
                .with_authoring_root(self.user.clone()),
        )));
        super::super::build_request_tool_service(
            Arc::new(registry),
            BTreeSet::new(),
            None,
            Some(t.permissions.turn_context(&t.request, "p411-run", false)),
            None,
            "p411",
            t.permissions.explicit.clone(),
            t.permissions.tier,
            false,
            &[],
            false,
            crate::tools::scoped::DeferredTools::empty(),
            None,
        )
    }

    /// What `execute()` does in order — ingress strip, the safety net's
    /// stamp (a no-op for a handler-stamped or non-slash request), the split
    /// — then the resolution and the tool gate.
    async fn turn(&self, engine: &TestEngine, mut request: RunRequest) -> Turn {
        slash_skill_scope::forget_at_ingress(&mut request.metadata);
        self.stamper
            .stamp_slash_mode_unattested(&request.input, &mut request.metadata)
            .await;
        split_with(&mut request, AGENT, &self.skills).await;
        let permissions = engine.resolve_turn_permissions(&request, &self.agent).await;
        let gate = gate(&request, &permissions, permissions.explicit.clone(), stubs);
        Turn {
            request,
            permissions,
            gate,
        }
    }
}

/// The run loop's narrowing, then the tool gate over `tools`: a
/// restricted-away tool is not in the request's registry at all.
pub(super) fn gate(
    request: &RunRequest,
    permissions: &TurnPermissions,
    explicit: Option<TurnToolPolicy>,
    tools: fn(&mut LoopToolRegistry, &dyn Fn(&str) -> bool),
) -> Arc<dyn ToolService> {
    let scope = slash_skill_scope::from_metadata(&request.metadata);
    let mut registry = LoopToolRegistry::new();
    tools(&mut registry, &|name| {
        slash_skill_scope::admits(scope.as_ref(), name)
    });
    super::super::build_request_tool_service(
        Arc::new(registry),
        BTreeSet::new(),
        None,
        Some(permissions.turn_context(request, "p411-run", false)),
        None,
        "p411",
        explicit,
        permissions.tier,
        false,
        &[],
        false,
        crate::tools::scoped::DeferredTools::empty(),
        None,
    )
}

pub(super) fn stubs(registry: &mut LoopToolRegistry, admits: &dyn Fn(&str) -> bool) {
    for name in TOOLS {
        if admits(name) {
            registry.register(Box::new(Stub(name)));
        }
    }
}

struct Turn {
    request: RunRequest,
    permissions: TurnPermissions,
    gate: Arc<dyn ToolService>,
}

impl Turn {
    fn pregrant(&self) -> Vec<String> {
        slash_skill_scope::pregrant_from_metadata(&self.request.metadata)
    }

    fn restriction(&self) -> Option<BTreeSet<String>> {
        slash_skill_scope::from_metadata(&self.request.metadata).map(|s| s.into_iter().collect())
    }

    async fn runs_with(&self, tool: &str, input: Value) -> bool {
        runs_with(&self.gate, tool, input).await
    }

    async fn runs(&self, tool: &str) -> bool {
        self.runs_with(tool, json!({})).await
    }

    async fn surface(&self) -> Vec<String> {
        let mut names: Vec<String> = self.gate.list().await.into_iter().map(|d| d.name).collect();
        names.sort();
        names
    }
}

/// Whether `gate` lets `tool` run with `input`.
pub(super) async fn runs_with(gate: &Arc<dyn ToolService>, tool: &str, input: Value) -> bool {
    gate.execute(tool, input).await.is_ok()
}

/// A request for `input`, as `role`, at the `ask` tier, run in `project`.
pub(super) fn request(input: &str, role: Option<&str>, project: &Path) -> RunRequest {
    let session = crate::gateway::router::SessionKey::main("p411");
    let mut request = super::super::tests::gate_test_request(&session, "p411-run");
    request.input = input.to_string();
    if let Some(role) = role {
        request
            .metadata
            .insert("caller_role".to_string(), role.to_string());
    }
    request.metadata.insert(
        EXEC_TIER_SESSION_KEY.to_string(),
        ExecTier::Ask.id().to_string(),
    );
    request.workspace_override = Some(project.to_path_buf());
    request
}

pub(super) fn at(tier: ExecTier, mut request: RunRequest) -> RunRequest {
    request
        .metadata
        .insert(EXEC_TIER_SESSION_KEY.to_string(), tier.id().to_string());
    request
}

/// An engine whose install-wide `[policies.tool_permissions]` is `policy`.
pub(super) fn engine(policy: Option<&str>) -> TestEngine {
    let mut config = crate::Config::default();
    if let Some(policy) = policy {
        config.policies.tool_permissions = serde_json::from_str(policy).unwrap();
    }
    super::super::tests::test_engine().with_app_config(Arc::new(tokio::sync::RwLock::new(config)))
}

pub(super) fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}
mod fire_sites;
mod grant_arm;
mod kind_and_key;
mod scope_and_children;
mod what_may_be_pregranted;
mod who_may_pregrant;
