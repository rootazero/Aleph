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
type TestEngine = super::super::engine::ExecutionEngine<
    crate::thinker::SingleProviderRegistry,
    super::super::tests::EmptyToolRegistry,
>;

const SKILL: &str = "p411-probe";
const AGENT: &str = "p411-agent";
const PLUGIN: &str = "p411-plug";
/// The tools the gate below holds. The first three are mutating, so the
/// `ask` tier cards each of them unless something lifts it; `self_config`
/// and `file_ops` carry argument-level rules.
const TOOLS: [&str; 5] = ["bash", "file_write", "file_edit", "self_config", "file_ops"];

/// A leaf that always succeeds: reaching it means every gate let the call by.
struct Stub(&'static str);

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

fn skill_md(name: &str, allowed: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: a pre-grant probe\nallowed-tools: {allowed}\n---\n\
         Do the thing.\n"
    )
}

fn write_skill(root: &Path, name: &str, allowed: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), skill_md(name, allowed)).unwrap();
}

/// Where a request's slash mode comes from.
#[derive(Clone, Copy)]
enum Stamp {
    /// The `chat.send` / `agent.run` handlers' `stamp_slash_mode`: typed.
    Handler,
    /// Nobody before `execute()`: its safety net stamps it, unattested — the
    /// shape of `sessions_send`, a team task, cron, heartbeat, A2A.
    SafetyNet,
}

/// Temp `$ALEPH_HOME` + `$HOME` (so both user-level roots are temp dirs), a
/// git project the run works in, a fresh `SkillSystem`, the slash catalog and
/// a command parser over it, and the agent.
struct World {
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
        let rejected = self.catalog.register_skills(&infos).await;
        assert!(rejected.is_empty(), "{rejected:?}");
    }

    /// A plugin command's slash row.
    async fn register_command(&self, name: &str, allowed: &[&str]) {
        let rejected = self
            .catalog
            .register_skills(&[SkillInfo {
                id: format!("{PLUGIN}:{name}"),
                name: name.into(),
                description: "a plugin command".into(),
                scope: crate::domain::skill::PromptScope::System,
                version: None,
                allowed_tools: Some(names(allowed)),
                argument_hint: None,
                plugin_id: Some(PLUGIN.into()),
            }])
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
fn gate(
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

fn stubs(registry: &mut LoopToolRegistry, admits: &dyn Fn(&str) -> bool) {
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
async fn runs_with(gate: &Arc<dyn ToolService>, tool: &str, input: Value) -> bool {
    gate.execute(tool, input).await.is_ok()
}

/// A request for `input`, as `role`, at the `ask` tier, run in `project`.
fn request(input: &str, role: Option<&str>, project: &Path) -> RunRequest {
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

fn at(tier: ExecTier, mut request: RunRequest) -> RunRequest {
    request
        .metadata
        .insert(EXEC_TIER_SESSION_KEY.to_string(), tier.id().to_string());
    request
}

/// An engine whose install-wide `[policies.tool_permissions]` is `policy`.
fn engine(policy: Option<&str>) -> TestEngine {
    let mut config = crate::Config::default();
    if let Some(policy) = policy {
        config.policies.tool_permissions = serde_json::from_str(policy).unwrap();
    }
    super::super::tests::test_engine().with_app_config(Arc::new(tokio::sync::RwLock::new(config)))
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

// ---------------------------------------------------------------------------
// Rulings 3 and 4, the arm that grants.
// ---------------------------------------------------------------------------

/// An operator's `/skill` from either user-level root pre-grants exactly its
/// own list: the listed tools run without a card, an unlisted mutating tool
/// still cards, and the surface stays whole — a skill never narrows.
#[tokio::test]
async fn a_user_level_skill_pregrants_its_own_list_for_an_operator() {
    for (root, role) in [
        ("aleph", None),
        ("aleph", Some("operator")),
        ("claude", None),
    ] {
        let w = World::new().await;
        let dir = if root == "aleph" {
            w.user.clone()
        } else {
            w.claude_user.clone()
        };
        write_skill(&dir, SKILL, "[bash, file_write]");
        w.scan(&[w.user.clone(), w.claude_user.clone()]).await;
        let t = w.turn(&engine(None), w.skill_request(role).await).await;

        let case = format!("{root} / {role:?}");
        assert_eq!(t.pregrant(), names(&["bash", "file_write"]), "{case}");
        assert!(t.restriction().is_none(), "{case}: a skill narrowed");
        assert!(t.runs("bash").await, "{case}: bash still asks");
        assert!(t.runs("file_write").await, "{case}: file_write still asks");
        assert!(
            !t.runs("file_edit").await,
            "{case}: an unlisted mutating tool ran without a card"
        );
        let mut whole = names(&TOOLS);
        whole.sort();
        assert_eq!(t.surface().await, whole, "{case}");
    }
}

/// A global plugin's skill pre-grants; the same skill under a
/// project-scoped plugin does not.
#[tokio::test]
async fn a_plugin_skill_pregrants_only_from_a_global_plugin() {
    for global in [true, false] {
        let w = World::new().await;
        let plugin_skills = w.tmp.path().join("plug").join("skills");
        write_skill(&plugin_skills, SKILL, "[bash]");
        let scope_key = if global {
            ScopeKey::Global
        } else {
            ScopeKey::project(&w.project)
        };
        publish_plugin_skill_dirs(vec![PublishedPluginSkillDir {
            dir: plugin_skills.clone(),
            plugin_id: PLUGIN.into(),
            scope_key,
        }]);
        w.scan(std::slice::from_ref(&w.user)).await;
        let source = w
            .skills
            .get_skill(&SKILL.into())
            .await
            .map(|m| m.source().clone());
        let t = w.turn(&engine(None), w.skill_request(None).await).await;
        let bash_runs = t.runs("bash").await;
        publish_plugin_skill_dirs(Vec::new());

        assert_eq!(source, Some(SkillSource::Plugin(PluginId::new(PLUGIN))));
        if global {
            assert_eq!(t.pregrant(), names(&["bash"]));
            assert!(bash_runs, "a global plugin's skill did not pre-grant");
        } else {
            assert!(t.pregrant().is_empty(), "{:?}", t.pregrant());
            assert!(!bash_runs, "a project-scoped plugin's skill pre-granted");
        }
    }
}

// ---------------------------------------------------------------------------
// Ruling 1 — the kind comes from the registration.
// ---------------------------------------------------------------------------

/// A plugin COMMAND reaches `execute.rs` as the same `type: "skill"` mode a
/// skill does. Its kind comes from the registration, never from whether it
/// would be admitted: this command's plugin is in no registry at all (the
/// shape a disabled, hidden or orphaned plugin's command takes at the split,
/// which reads no admission state), and a user-level SKILL with the
/// command's bare name exists. The command keeps its RESTRICTION and is
/// never pre-granted — typed by an operator into a handler, as here.
#[tokio::test]
async fn a_plugin_command_restricts_and_never_pregrants_whatever_its_admission() {
    let w = World::new().await;
    write_skill(&w.user, "greet", "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;
    w.register_command("greet", &["bash"]).await;
    let request = w
        .request(&format!("/{PLUGIN}:greet hi"), None, Stamp::Handler, &[])
        .await;
    assert!(
        slash_skill_scope::is_typed(&request.metadata),
        "precondition"
    );
    let t = w.turn(&engine(None), request).await;

    assert!(
        t.pregrant().is_empty(),
        "a command pre-granted {:?}",
        t.pregrant()
    );
    assert_eq!(
        t.restriction(),
        Some(BTreeSet::from(["bash".to_string()])),
        "a command's allowed-tools must still restrict"
    );
    assert_eq!(t.surface().await, names(&["bash"]));
    assert!(
        !t.runs("bash").await,
        "restricted to bash, and bash still asks"
    );
    assert!(!t.runs("file_write").await, "narrowed away");
}

// ---------------------------------------------------------------------------
// Ruling 2 — the key is unforgeable.
// ---------------------------------------------------------------------------

/// Whatever pre-grant a request arrives with is gone at ingress: a plain
/// turn and a guest's `/skill` keep none, and an operator's `/skill` keeps
/// only the skill's own list, not the wider one it arrived with.
#[tokio::test]
async fn a_pregrant_the_request_arrives_with_never_survives_ingress() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[file_write]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let forged = || {
        let mut m = HashMap::new();
        let wide = names(&["bash", "file_write", "file_edit"]);
        slash_skill_scope::stamp_pregrant_from_names(&mut m, &wide, &wide);
        assert!(!m.is_empty(), "the forged key was written");
        m
    };

    let mut plain = w.request("hello", None, Stamp::Handler, &[]).await;
    plain.metadata.extend(forged());
    let t = w.turn(&engine(None), plain).await;
    assert!(
        t.pregrant().is_empty(),
        "plain turn kept {:?}",
        t.pregrant()
    );
    assert!(!t.runs("bash").await, "plain turn ran bash uncarded");

    let mut guest = w.skill_request(Some("guest")).await;
    guest.metadata.extend(forged());
    let t = w.turn(&engine(None), guest).await;
    assert!(t.pregrant().is_empty(), "guest kept {:?}", t.pregrant());
    assert!(!t.runs("bash").await, "guest ran bash uncarded");

    let mut operator = w.skill_request(None).await;
    operator.metadata.extend(forged());
    let t = w.turn(&engine(None), operator).await;
    assert_eq!(t.pregrant(), names(&["file_write"]));
    assert!(t.runs("file_write").await);
    assert!(!t.runs("bash").await, "the forged wider list survived");
    assert!(!t.runs("file_edit").await, "the forged wider list survived");
}

// ---------------------------------------------------------------------------
// Ruling 3 — operator only.
// ---------------------------------------------------------------------------

/// A guest's or member's `/skill` still runs, on the whole surface its caller
/// has on a plain turn, with nothing pre-granted.
#[tokio::test]
async fn a_non_operator_skill_runs_on_the_whole_surface_with_nothing_pregranted() {
    for role in ["guest", "member"] {
        let w = World::new().await;
        write_skill(&w.user, SKILL, "[bash]");
        w.scan(std::slice::from_ref(&w.user)).await;
        let t = w
            .turn(&engine(None), w.skill_request(Some(role)).await)
            .await;
        let plain = w.request("hello", Some(role), Stamp::Handler, &[]).await;
        let control = w.turn(&engine(None), plain).await;

        assert!(t.pregrant().is_empty(), "{role}: {:?}", t.pregrant());
        assert!(t.restriction().is_none(), "{role}: a skill narrowed");
        assert_eq!(t.surface().await, control.surface().await, "{role}");
        assert!(t.surface().await.contains(&"bash".to_string()), "{role}");
        assert!(!t.runs("bash").await, "{role}: bash ran without a card");
    }
}

// ---------------------------------------------------------------------------
// Fix round F2 — only a `/skill` a person typed pre-grants.
// ---------------------------------------------------------------------------

/// `/p411-probe` put in a request by something other than a person: the
/// shapes `sessions_send` (the model's text, the origin's operator role
/// propagated), a team task (fresh metadata, no role) and A2A (a remote
/// peer's text, unattended, no role) take. None of them passes a handler,
/// so `execute()`'s safety net stamps the mode — unattested. Nothing is
/// pre-granted. The control is the same `/p411-probe` typed into a handler
/// by an operator, which does pre-grant.
#[tokio::test]
async fn a_slash_no_person_typed_pregrants_nothing() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let input = format!("/{SKILL} go");
    let unattended = super::super::UNATTENDED_KEY;
    let shapes: [(&str, Option<&str>, &[(&str, &str)]); 3] = [
        ("sessions_send", Some("operator"), &[]),
        ("team task", None, &[]),
        ("A2A", None, &[(unattended, "true")]),
    ];
    for (shape, role, extra) in shapes {
        let request = w.request(&input, role, Stamp::SafetyNet, extra).await;
        let t = w.turn(&engine(None), request).await;
        assert!(
            t.request
                .metadata
                .contains_key(crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY),
            "{shape}: precondition — the safety net stamped the skill's mode"
        );
        assert!(t.pregrant().is_empty(), "{shape}: {:?}", t.pregrant());
        assert!(!t.runs("bash").await, "{shape}: bash ran without a card");
    }

    let t = w.turn(&engine(None), w.skill_request(None).await).await;
    assert_eq!(
        t.pregrant(),
        names(&["bash"]),
        "control: a handler's /skill"
    );
    assert!(t.runs("bash").await, "control: a handler's /skill");
}

/// A typed `/skill` on a run with nobody there pre-grants nothing either.
#[tokio::test]
async fn a_typed_slash_on_an_unattended_run_pregrants_nothing() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let request = w
        .request(
            &format!("/{SKILL} go"),
            None,
            Stamp::Handler,
            &[(super::super::UNATTENDED_KEY, "true")],
        )
        .await;
    assert!(
        slash_skill_scope::is_typed(&request.metadata),
        "precondition"
    );
    let t = w.turn(&engine(None), request).await;
    assert!(t.pregrant().is_empty(), "{:?}", t.pregrant());
    assert!(!t.runs("bash").await);
}

/// A producer that sets the typed marker itself, on text the safety net will
/// stamp, gets nothing: the marker vouches for a mode, and there is none at
/// ingress.
#[tokio::test]
async fn a_forged_typed_marker_never_survives_ingress() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let mut request = w
        .request(&format!("/{SKILL} go"), None, Stamp::SafetyNet, &[])
        .await;
    slash_skill_scope::mark_typed(&mut request.metadata);
    let t = w.turn(&engine(None), request).await;
    assert!(!slash_skill_scope::is_typed(&t.request.metadata));
    assert!(t.pregrant().is_empty(), "{:?}", t.pregrant());
    assert!(!t.runs("bash").await);
}

// ---------------------------------------------------------------------------
// Ruling 4 — origin.
// ---------------------------------------------------------------------------

/// A skill a repository brings (`.claude/skills` — which the path guess
/// labels `Bundled` — or `.aleph/skills`) pre-grants nothing, and never
/// narrows either.
#[tokio::test]
async fn a_project_skill_pregrants_nothing() {
    for flavour in [".claude", ".aleph"] {
        let w = World::new().await;
        let dir = w.project_skills(flavour);
        write_skill(&dir, SKILL, "[bash]");
        w.scan(&[w.user.clone(), dir.clone()]).await;
        let t = w.turn(&engine(None), w.skill_request(None).await).await;

        assert!(t.pregrant().is_empty(), "{flavour}: {:?}", t.pregrant());
        assert!(t.restriction().is_none(), "{flavour}: a skill narrowed");
        assert!(!t.runs("bash").await, "{flavour}: bash ran without a card");
    }
}

/// `skill_read` loads a project's `foo` before the user's `foo`, while the
/// skill registry answers with the user's (`Global` outranks the project
/// `.claude` skill's `Bundled`). Judging the registry's winner would grant
/// the user's list to the repository's body.
#[tokio::test]
async fn a_user_level_skill_shadowed_by_a_project_skill_pregrants_nothing() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash]");
    let dir = w.project_skills(".claude");
    write_skill(&dir, SKILL, "[bash]");
    w.scan(&[w.user.clone(), dir.clone()]).await;
    let registered = w
        .skills
        .get_skill(&SKILL.into())
        .await
        .map(|m| m.source().clone());
    assert_eq!(
        registered,
        Some(SkillSource::Global),
        "precondition: the registry answers with the user's skill"
    );
    let t = w.turn(&engine(None), w.skill_request(None).await).await;

    assert!(t.pregrant().is_empty(), "{:?}", t.pregrant());
    assert!(!t.runs("bash").await);
}

/// An agent-level skill (`~/.aleph/agents/<id>/skills`) is loaded first by
/// `skill_read` and is not one of the two user-level roots: no pre-grant.
#[tokio::test]
async fn an_agent_level_skill_pregrants_nothing() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash]");
    let agent_skills = crate::utils::paths::get_config_dir()
        .unwrap()
        .join("agents")
        .join(AGENT)
        .join("skills");
    write_skill(&agent_skills, SKILL, "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let t = w.turn(&engine(None), w.skill_request(None).await).await;

    assert!(t.pregrant().is_empty(), "{:?}", t.pregrant());
    assert!(!t.runs("bash").await);
}

// ---------------------------------------------------------------------------
// Fix round F1 + F3 — what may be pre-granted.
// ---------------------------------------------------------------------------

/// The file is re-read at turn start; registration validated an earlier
/// version of it (`[file_write]`). An edit since — a new tool, a glob that
/// the policy would read back as "every tool" — grants nothing it added.
#[tokio::test]
async fn a_file_widened_after_registration_pregrants_only_the_registered_names() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[file_write]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let request = w.skill_request(None).await;
    write_skill(&w.user, SKILL, r#"[file_write, bash, "?*", "file_*"]"#);
    let t = w.turn(&engine(None), request).await;

    assert_eq!(t.pregrant(), names(&["file_write"]));
    assert!(t.runs("file_write").await, "the registered name is granted");
    assert!(!t.runs("bash").await, "a name added after registration ran");
    assert!(
        !t.runs("file_edit").await,
        "a glob added after registration ran"
    );
    let gate_write = json!({ "action": "update_config", "config_path": "policies.exec_tier" });
    assert!(!t.runs_with("self_config", gate_write).await);
}

/// `skill_manage` — the model's authoring tool, writing into the
/// operator-owned `~/.aleph/skills` the pre-grant trusts — may keep or narrow
/// a skill's `allowed-tools`, never add to it: `edit` and `patch` that widen
/// are refused under every tier (here `full`, which cards nothing), and under
/// `plan` the tool does not run at all. Through the real tool gate and the
/// real tool.
#[tokio::test]
async fn skill_manage_never_adds_to_a_skills_grant() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[file_write]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let svc = w.skill_manage_gate(ExecTier::Full).await;
    let file = w.user.join(SKILL).join("SKILL.md");

    let widen_edit = json!({
        "action": "edit", "skill_id": SKILL, "content": skill_md(SKILL, "[file_write, bash]")
    });
    assert!(
        !runs_with(&svc, "skill_manage", widen_edit).await,
        "an edit added bash to the grant"
    );
    let widen_patch = json!({
        "action": "patch", "skill_id": SKILL,
        "find": "allowed-tools: [file_write]", "replace": "allowed-tools: [file_write, bash]"
    });
    assert!(
        !runs_with(&svc, "skill_manage", widen_patch).await,
        "a patch added bash to the grant"
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        skill_md(SKILL, "[file_write]"),
        "a refused write touched the file"
    );
    let keep = json!({
        "action": "patch", "skill_id": SKILL, "find": "Do the thing.", "replace": "Do it well."
    });
    assert!(
        runs_with(&svc, "skill_manage", keep).await,
        "a body edit that keeps the grant was refused"
    );
    let narrow = json!({
        "action": "edit", "skill_id": SKILL, "content": skill_md(SKILL, "[]")
    });
    assert!(
        runs_with(&svc, "skill_manage", narrow).await,
        "a narrowing edit was refused"
    );

    let svc = w.skill_manage_gate(ExecTier::Plan).await;
    let keep = json!({
        "action": "patch", "skill_id": SKILL, "find": "Do it well.", "replace": "Do it."
    });
    assert!(
        !runs_with(&svc, "skill_manage", keep).await,
        "skill_manage wrote under plan"
    );
}

// ---------------------------------------------------------------------------
// Ruling 5 — a model-initiated load never pre-grants.
// ---------------------------------------------------------------------------

/// The model loading a skill itself (`skill_read`) is a turn with no slash
/// mode: nothing is pre-granted however much the skill declares, and a real
/// `skill_read` of it leaves the turn's facts where they were. A command
/// restricted to `skill_read` stays restricted to it.
#[tokio::test]
async fn a_model_initiated_skill_load_grants_nothing() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;

    let plain = w
        .request(&format!("use the {SKILL} skill"), None, Stamp::Handler, &[])
        .await;
    let t = w.turn(&engine(None), plain).await;
    assert!(t.pregrant().is_empty(), "{:?}", t.pregrant());
    assert!(!t.runs("bash").await);

    w.register_command("reader", &["skill_read"]).await;
    let request = w
        .request(&format!("/{PLUGIN}:reader"), None, Stamp::Handler, &[])
        .await;
    let t = w.turn(&engine(None), request).await;
    let before = (t.restriction(), t.pregrant());
    let body =
        crate::builtin_tools::skill_reader::ReadSkillTool::with_auto_discover(Some(&w.project))
            .call_json(json!({ "skill_id": SKILL }))
            .await
            .expect("the model's skill_read loads the skill");
    assert!(body.to_string().contains("Do the thing."), "{body}");
    assert_eq!((t.restriction(), t.pregrant()), before);
    assert_eq!(before.0, Some(BTreeSet::from(["skill_read".to_string()])));
    assert!(before.1.is_empty());
}

// ---------------------------------------------------------------------------
// Fix round F4 — children.
// ---------------------------------------------------------------------------

/// A subagent this turn spawns runs under `for_children()` — the policy
/// without the pre-grant — so it is carded for the tool its parent turn ran
/// uncarded.
#[tokio::test]
async fn a_child_is_carded_for_what_its_parent_turn_pregranted() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let t = w.turn(&engine(None), w.skill_request(None).await).await;
    assert!(
        t.runs("bash").await,
        "precondition: the parent turn pre-granted bash"
    );

    let child = gate(
        &t.request,
        &t.permissions,
        t.permissions
            .explicit
            .as_ref()
            .and_then(TurnToolPolicy::for_children),
        stubs,
    );
    assert!(
        !runs_with(&child, "bash", json!({})).await,
        "a child ran its parent's pre-granted bash without a card"
    );
}

// ---------------------------------------------------------------------------
// Rulings 6 and 7 — the lift's scope, one derivation.
// ---------------------------------------------------------------------------

/// A skill declaring the four tools the rulings below speak about.
async fn declaring_world() -> World {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash, file_write, file_edit, self_config]");
    w.scan(std::slice::from_ref(&w.user)).await;
    w
}

/// The pre-grant lifts the tier's `Ask` and nothing else: an install's
/// explicit deny and glob win over it; a name nothing binds is lifted.
#[tokio::test]
async fn an_explicit_entry_outranks_the_pregrant() {
    let w = declaring_world().await;
    let t = w
        .turn(
            &engine(Some(
                r#"{"default":"allow","overrides":{"bash":"deny","file_e*":"ask"}}"#,
            )),
            w.skill_request(None).await,
        )
        .await;
    assert!(
        !t.runs("bash").await,
        "an explicit deny lost to the pre-grant"
    );
    assert!(!t.runs("file_edit").await, "a glob lost to the pre-grant");
    assert!(t.runs("file_write").await, "nothing bound file_write");
}

/// A policy whose `default` is `deny` takes no pre-grant: an exact `allow`
/// would outrank the operator's default, which is not the tier.
#[tokio::test]
async fn a_deny_default_takes_no_pregrant() {
    let w = declaring_world().await;
    let t = w
        .turn(
            &engine(Some(r#"{"default":"deny","overrides":{}}"#)),
            w.skill_request(None).await,
        )
        .await;
    assert!(
        !t.runs("file_write").await,
        "a deny default lost to the pre-grant"
    );
}

/// The floors hold under a live pre-grant: the gate-removal floor still
/// cards a `self_config` write the skill listed, and `plan` still refuses.
#[tokio::test]
async fn the_floors_hold_under_a_pregrant() {
    let w = declaring_world().await;
    let t = w.turn(&engine(None), w.skill_request(None).await).await;
    assert!(t.runs("file_write").await, "control: the pre-grant is live");
    let gate_write = json!({ "action": "update_config", "config_path": "policies.exec_tier" });
    assert!(
        !t.runs_with("self_config", gate_write).await,
        "the gate-removal floor stood down for a skill's list"
    );

    let t = w
        .turn(
            &engine(None),
            at(ExecTier::Plan, w.skill_request(None).await),
        )
        .await;
    assert_eq!(t.permissions.tier, ExecTier::Plan);
    assert!(
        !t.runs("file_write").await,
        "plan's floor lost to the pre-grant"
    );
}

/// A pre-grant lifts the NAME-level `Ask` only. Its entry is not a decision
/// a person wrote, so the tool gate must not read it as one: the
/// argument-level cards stay. A pre-granted `file_ops` lists without a card
/// and still cards a destructive `delete` — under `auto`, and under `ask`,
/// where the name-level `Ask` the pre-grant lifted was what covered it. The
/// control is the same call under an operator's own exact `allow`, which is a
/// person's decision and does stand the card down.
#[tokio::test]
async fn a_pregranted_tool_keeps_its_argument_cards() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[file_ops]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let list = json!({ "operation": "list", "path": "." });
    let delete = json!({ "operation": "delete", "path": "gone.txt" });

    for tier in [ExecTier::Auto, ExecTier::Ask] {
        let t = w
            .turn(&engine(None), at(tier, w.skill_request(None).await))
            .await;
        assert_eq!(t.pregrant(), names(&["file_ops"]), "{tier:?}");
        assert!(
            t.runs_with("file_ops", list.clone()).await,
            "{tier:?}: the name-level lift is gone"
        );
        assert!(
            !t.runs_with("file_ops", delete.clone()).await,
            "{tier:?}: a pre-granted destructive file_ops ran without its argument card"
        );
    }

    let t = w
        .turn(
            &engine(Some(
                r#"{"default":"allow","overrides":{"file_ops":"allow"}}"#,
            )),
            at(ExecTier::Auto, w.skill_request(None).await),
        )
        .await;
    assert!(
        t.runs_with("file_ops", delete).await,
        "control: an operator's own exact allow stands the argument card down"
    );
}

/// The one merge: the value a plugin command's inline shell reads
/// (`builtin_permission`) and the tool gate agree on a pre-granted `bash`,
/// and an explicit deny still reads `Deny` on both — the inline face
/// withholds on `Deny` only, so a pre-grant never changes it.
#[tokio::test]
async fn the_inline_face_and_the_tool_gate_read_one_pregrant() {
    use crate::extension::PermissionAction;
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;

    let t = w.turn(&engine(None), w.skill_request(None).await).await;
    assert_eq!(
        t.permissions.builtin_permission("bash"),
        PermissionAction::Allow
    );
    assert!(t.runs("bash").await);

    let t = w
        .turn(
            &engine(Some(r#"{"default":"allow","overrides":{"bash":"deny"}}"#)),
            w.skill_request(None).await,
        )
        .await;
    assert_eq!(
        t.permissions.builtin_permission("bash"),
        PermissionAction::Deny
    );
    assert!(!t.runs("bash").await);
}

// ---------------------------------------------------------------------------
// Fire sites.
// ---------------------------------------------------------------------------

/// Who may spell each key and call each function of the wire, across every
/// production file under `src/`: the pre-grant is written only by the split
/// and read only by the resolution; the typed marker is written only by the
/// two human-facing stamps; the ingress strip runs only in `execute()`. So
/// neither `skill_read`, a resume, a queue replay nor any other producer can
/// put either on a turn.
#[test]
fn the_wire_has_one_writer_and_one_reader_per_fact() {
    use crate::utils::source_scan::{code_keeping_literals, production_text, rust_sources_under};
    const WIRE: &str = "src/gateway/execution_engine/slash_skill_scope.rs";
    const SPLIT: &str = "src/gateway/execution_engine/slash_skill_pregrant/mod.rs";
    const RESOLVE: &str = "src/gateway/execution_engine/turn_permissions.rs";
    const EXECUTE: &str = "src/gateway/execution_engine/execute.rs";
    const STAMP: &str = "src/gateway/execution_engine/slash_command.rs";
    const ROUTER: &str = "src/gateway/inbound_router/executor.rs";
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let sources = rust_sources_under(&root);
    assert!(
        sources.len() > 500,
        "the walk found only {} files",
        sources.len()
    );
    let pregrant_key = format!("\"{}\"", slash_skill_scope::SLASH_SKILL_PREGRANT_TOOLS_KEY);
    let typed_key = format!("\"{}\"", slash_skill_scope::SLASH_MODE_TYPED_KEY);
    let owners: [(&str, &[&str]); 8] = [
        (pregrant_key.as_str(), &[WIRE]),
        (typed_key.as_str(), &[WIRE]),
        ("stamp_pregrant_from_names(", &[WIRE, SPLIT]),
        ("forget_pregrant(", &[WIRE]),
        ("pregrant_from_metadata(", &[WIRE, RESOLVE]),
        ("mark_typed(", &[WIRE, STAMP, ROUTER]),
        ("is_typed(", &[WIRE, SPLIT]),
        ("forget_at_ingress(", &[WIRE, EXECUTE]),
    ];
    let mut offenders = Vec::new();
    let mut seen = [false; 8];
    for (rel, text) in &sources {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        let code = code_keeping_literals(&production_text(&path, text));
        for (i, (needle, allowed)) in owners.iter().enumerate() {
            seen[i] |= code.contains(needle);
            if code.contains(needle) && !allowed.contains(&rel.as_str()) {
                offenders.push(format!("{rel}: {needle}"));
            }
        }
    }
    // Self-defence: a needle nobody spells would pass for the wrong reason.
    assert_eq!(seen, [true; 8], "a needle matched nothing: {owners:?}");
    assert!(offenders.is_empty(), "{offenders:?}");
}

/// `execute.rs`:
/// - strips at ingress as its very first metadata operation, before
///   `stamp_btw` and before `admit_run` (whose steer-fold resolves this
///   turn's permissions);
/// - stamps its safety net unattested, never with the human-facing stamp;
/// - runs the split exactly once, at `execute()`'s top level — for every
///   request, not only for one that carries a slash mode — and before the
///   fast path; the restriction is stamped by no other route.
///
/// The router marks the mode it inserts for a channel message, in the same
/// block. The resolution folds the pre-grant into the one merge, after the
/// channel layer and before the all-default check. The run loop builds the
/// children's view from `for_children()` and its own from the whole policy.
/// The steering rescue strips the skill facts from the metadata it re-drives.
#[test]
fn the_fire_sites_are_where_the_design_puts_them() {
    use crate::utils::source_scan::{production_prefix, strip_comment_lines};
    let code = |src: &str| strip_comment_lines(&production_prefix(src));
    let once = |code: &str, needle: &str| -> usize {
        assert_eq!(
            code.matches(needle).count(),
            1,
            "`{needle}` must occur exactly once"
        );
        code.find(needle).expect("counted once above")
    };

    let execute = code(include_str!("../execute.rs"));
    let ingress = once(&execute, "slash_skill_scope::forget_at_ingress(");
    let btw = once(&execute, "stamp_btw(&request.input");
    let admit = once(&execute, ".admit_run(");
    assert!(
        ingress < btw && btw < admit,
        "the ingress strip must be execute()'s first metadata step, before admit_run"
    );
    once(&execute, ".stamp_slash_mode_unattested(");
    assert!(
        !execute.contains(".stamp_slash_mode("),
        "execute()'s safety net must not use the human-facing stamp"
    );
    let split = once(&execute, "slash_skill_pregrant::split(");
    let fast_path = once(&execute, ".execute_slash_command_fast_path(");
    assert!(split < fast_path, "the split must run before the fast path");
    assert!(
        !execute.contains("stamp_from_mode("),
        "execute.rs restricts only through the split"
    );
    let line = execute
        .lines()
        .find(|l| l.contains("slash_skill_pregrant::split("))
        .expect("found above");
    assert!(
        line.starts_with("        super::") && !line.starts_with("         "),
        "the split must sit at `execute()`'s top level, not inside a branch: {line:?}"
    );

    let router = code(include_str!("../../inbound_router/executor.rs"));
    let inserted = once(
        &router,
        "metadata.insert(SLASH_COMMAND_MODE_KEY.to_string(), mode);",
    );
    let marked = once(&router, "slash_skill_scope::mark_typed(&mut metadata)");
    let block_end = router
        .get(inserted..)
        .and_then(|rest| rest.find('}'))
        .map(|at| at + inserted)
        .expect("the insert's block closes");
    assert!(
        inserted < marked && marked < block_end,
        "the router must mark exactly the mode it inserts, in the same block"
    );

    let resolve = code(include_str!("../turn_permissions.rs"));
    let channel = once(
        &resolve,
        "merged = ToolPermissionsConfig::merge(&merged, &channel_perms)",
    );
    let fold = once(&resolve, "apply_pregrant(&mut merged, &pregrant)");
    let all_default = once(&resolve, "let is_all_default");
    assert!(
        channel < fold && fold < all_default,
        "the pre-grant folds after the channel layer and before the all-default check"
    );

    let inner = code(include_str!("../run_loop/inner.rs"));
    let children = once(&inner, "let parent_view_for_children");
    let child_policy = once(&inner, "explicit.for_children()");
    let own = once(
        &inner,
        "let tool_service = super::super::build_request_tool_service(",
    );
    assert!(
        children < child_policy && child_policy < own,
        "the children's view must be built from `for_children()`"
    );
    assert_eq!(
        inner.matches("turn_permissions.explicit.clone()").count(),
        1,
        "the run's own service takes the whole policy, once"
    );

    let steering = code(include_str!("../steering.rs"));
    let rescue = once(&steering, "fn build_steering_rescue_request(");
    let after = |needle: &str| {
        steering
            .get(rescue..)
            .and_then(|rest| rest.find(needle))
            .map(|at| at + rescue)
            .unwrap_or_else(|| panic!("`{needle}` after the rescue"))
    };
    let cloned = after("request.metadata.clone()");
    let stripped = after("slash_skill_scope::strip(&mut metadata)");
    let built = after("Some(RunRequest {");
    assert!(
        cloned < stripped && stripped < built,
        "the rescue must strip the skill facts before it re-drives"
    );
}
