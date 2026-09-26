//! `/name` through the production steps a turn takes, in their order: the
//! split `execute.rs` runs before the fast path ([`split_with`]), the turn's
//! permission resolution (`resolve_turn_permissions`), and the tool gate the
//! run loop builds from that resolution (`build_request_tool_service`, with
//! the run loop's own narrowing). The gate has no approval channel, so a call
//! the tier would card is refused and a lifted one reaches its stub. The fire
//! sites no behavioural test can reach are pinned by source at the end.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use super::super::slash_skill_scope;
use super::super::turn_permissions::TurnPermissions;
use super::super::RunRequest;
use super::split_with;
use crate::config::types::policies::{ExecTier, EXEC_TIER_SESSION_KEY};
use crate::domain::skill::{PluginId, SkillSource};
use crate::extension::visibility::ScopeKey;
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY;
use crate::skill::{SkillInfo, SkillSystem};
use crate::sync_primitives::Arc;
use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult};
use crate::tools::service::ToolService;
use crate::utils::paths::{publish_plugin_skill_dirs, IsolatedAlephHome, PublishedPluginSkillDir};

/// The unit-test engine (`execution_engine::tests::test_engine`).
type TestEngine = super::super::engine::ExecutionEngine<
    crate::thinker::SingleProviderRegistry,
    super::super::tests::EmptyToolRegistry,
>;

const SKILL: &str = "p411-probe";
const AGENT: &str = "p411-agent";
const PLUGIN: &str = "p411-plug";
/// The tools the gate below holds. The first three are mutating, so the
/// `ask` tier cards each of them unless something lifts it.
const TOOLS: [&str; 4] = ["bash", "file_write", "file_edit", "self_config"];

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

fn write_skill(root: &Path, name: &str, allowed: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\nname: {name}\ndescription: a pre-grant probe\nallowed-tools: {allowed}\n---\n\
             Do the thing.\n"
        ),
    )
    .unwrap();
}

/// An isolated Aleph home with its user-level skills root, a git project the
/// run works in, a fresh `SkillSystem`, and the agent.
struct World {
    _home: IsolatedAlephHome,
    tmp: TempDir,
    user: PathBuf,
    project: PathBuf,
    skills: SkillSystem,
    agent: Arc<AgentInstance>,
}

impl World {
    async fn new() -> Self {
        let home = IsolatedAlephHome::new();
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("repo");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        let project = project.canonicalize().unwrap();
        let user = crate::utils::paths::get_skills_dir().unwrap();
        std::fs::create_dir_all(&user).unwrap();
        let agent = super::super::tests::gate_test_agent(&tmp, AGENT).await;
        Self {
            _home: home,
            tmp,
            user,
            project,
            skills: SkillSystem::new(),
            agent,
        }
    }

    /// `<project>/<flavour>/skills` — `.claude` or `.aleph`.
    fn project_skills(&self, flavour: &str) -> PathBuf {
        self.project.join(flavour).join("skills")
    }

    /// The base dirs scanned the way boot scans them (`SkillSystem::init`,
    /// plus whatever plugin dirs are published).
    async fn scan(&self, dirs: &[PathBuf]) {
        self.skills.init(dirs.to_vec()).await;
    }

    /// `/p411-probe go`, its slash mode built from the scanned manifest the
    /// way boot builds the catalog row (`SkillInfo::from(&manifest)`).
    async fn skill_request(&self, role: Option<&str>) -> RunRequest {
        let manifest = self
            .skills
            .get_skill(&SKILL.into())
            .await
            .expect("the probe skill was scanned");
        let mode = envelope(SkillInfo::from(&manifest), &format!("/{SKILL} go")).await;
        request(Some(mode), role, &self.project)
    }

    /// Split, resolve, build the gate.
    async fn turn(&self, engine: &TestEngine, mut request: RunRequest) -> Turn {
        split_with(&mut request, AGENT, &self.skills).await;
        let permissions = engine.resolve_turn_permissions(&request, &self.agent).await;
        // The run loop's narrowing: a restricted-away tool is not in the
        // request's registry at all.
        let scope = slash_skill_scope::from_metadata(&request.metadata);
        let mut registry = LoopToolRegistry::new();
        for name in TOOLS {
            if slash_skill_scope::admits(scope.as_ref(), name) {
                registry.register(Box::new(Stub(name)));
            }
        }
        let gate = super::super::build_request_tool_service(
            Arc::new(registry),
            BTreeSet::new(),
            None,
            Some(permissions.turn_context(&request, "p411-run", false)),
            None,
            "p411",
            permissions.explicit.clone(),
            permissions.tier,
            false,
            &[],
            false,
            crate::tools::scoped::DeferredTools::empty(),
            None,
        );
        Turn {
            request,
            permissions,
            gate,
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

    /// Whether the gate lets `tool` run with `input`.
    async fn runs_with(&self, tool: &str, input: Value) -> bool {
        self.gate.execute(tool, input).await.is_ok()
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

/// The slash mode both slash faces stamp for `input`: `entry` registered in
/// a catalog, the command parser, the serializer.
async fn envelope(entry: SkillInfo, input: &str) -> String {
    let catalog = Arc::new(crate::tool_metadata::ToolCatalog::new());
    let rejected = catalog.register_skills(&[entry]).await;
    assert!(rejected.is_empty(), "{rejected:?}");
    let parsed = crate::command::CommandParser::new(catalog)
        .parse_async(input)
        .await
        .expect("the entry resolves as a slash command");
    crate::gateway::inbound_router::serialize_parsed_command(&parsed).expect("it serializes")
}

/// A request carrying `mode` (or none), as `role`, at the `ask` tier, run in
/// `project`.
fn request(mode: Option<String>, role: Option<&str>, project: &Path) -> RunRequest {
    let session = crate::gateway::router::SessionKey::main("p411");
    let mut request = super::super::tests::gate_test_request(&session, "p411-run");
    request.input = format!("/{SKILL} go");
    if let Some(mode) = mode {
        request
            .metadata
            .insert(SLASH_COMMAND_MODE_KEY.to_string(), mode);
    }
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

/// An operator's `/skill` from a user-level root pre-grants exactly its own
/// list: the listed tools run without a card, an unlisted mutating tool
/// still cards, and the surface stays whole — a skill never narrows.
#[tokio::test]
async fn a_user_level_skill_pregrants_its_own_list_for_an_operator() {
    for role in [None, Some("operator")] {
        let w = World::new().await;
        write_skill(&w.user, SKILL, "[bash, file_write]");
        w.scan(std::slice::from_ref(&w.user)).await;
        let t = w.turn(&engine(None), w.skill_request(role).await).await;

        assert_eq!(t.pregrant(), names(&["bash", "file_write"]), "{role:?}");
        assert!(t.restriction().is_none(), "{role:?}: a skill narrowed");
        assert!(t.runs("bash").await, "{role:?}: bash still asks");
        assert!(
            t.runs("file_write").await,
            "{role:?}: file_write still asks"
        );
        assert!(
            !t.runs("file_edit").await,
            "{role:?}: an unlisted mutating tool ran without a card"
        );
        let mut whole = names(&TOOLS);
        whole.sort();
        assert_eq!(t.surface().await, whole, "{role:?}");
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
/// never pre-granted.
#[tokio::test]
async fn a_plugin_command_restricts_and_never_pregrants_whatever_its_admission() {
    let w = World::new().await;
    write_skill(&w.user, "greet", "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let command = SkillInfo {
        id: format!("{PLUGIN}:greet"),
        name: "greet".into(),
        description: "a plugin command".into(),
        scope: crate::domain::skill::PromptScope::System,
        version: None,
        allowed_tools: Some(names(&["bash"])),
        argument_hint: None,
        plugin_id: Some(PLUGIN.into()),
    };
    let mode = envelope(command, &format!("/{PLUGIN}:greet hi")).await;
    let t = w
        .turn(&engine(None), request(Some(mode), None, &w.project))
        .await;

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

/// Whatever pre-grant a request arrives with is gone after the split: a
/// plain turn and a guest's `/skill` keep none, and an operator's `/skill`
/// keeps only the skill's own list, not the wider one it arrived with.
#[tokio::test]
async fn a_pregrant_the_request_arrives_with_never_survives_the_split() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[file_write]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let forged = || {
        let mut m = HashMap::new();
        slash_skill_scope::stamp_pregrant_from_names(
            &mut m,
            &names(&["bash", "file_write", "file_edit"]),
        );
        assert!(!m.is_empty(), "the forged key was written");
        m
    };

    let mut plain = request(None, None, &w.project);
    plain.input = "hello".into();
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
        let mut plain = request(None, Some(role), &w.project);
        plain.input = "hello".into();
        let control = w.turn(&engine(None), plain).await;

        assert!(t.pregrant().is_empty(), "{role}: {:?}", t.pregrant());
        assert!(t.restriction().is_none(), "{role}: a skill narrowed");
        assert_eq!(t.surface().await, control.surface().await, "{role}");
        assert!(t.surface().await.contains(&"bash".to_string()), "{role}");
        assert!(!t.runs("bash").await, "{role}: bash ran without a card");
    }
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
// Ruling 5 — a model-initiated load never pre-grants.
// ---------------------------------------------------------------------------

/// The model loading a skill itself (`skill_read`) is a turn with no slash
/// mode: nothing is pre-granted however much the skill declares, and a real
/// `skill_read` of it leaves the turn's facts where they were. A command
/// restricted to `skill_read` stays restricted to it.
#[tokio::test]
async fn a_model_initiated_skill_load_grants_nothing() {
    use crate::tools::AlephTool;
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;

    let mut plain = request(None, None, &w.project);
    plain.input = format!("use the {SKILL} skill");
    let t = w.turn(&engine(None), plain).await;
    assert!(t.pregrant().is_empty(), "{:?}", t.pregrant());
    assert!(!t.runs("bash").await);

    let command = SkillInfo {
        id: format!("{PLUGIN}:reader"),
        name: "reader".into(),
        description: "a plugin command".into(),
        scope: crate::domain::skill::PromptScope::System,
        version: None,
        allowed_tools: Some(names(&["skill_read"])),
        argument_hint: None,
        plugin_id: Some(PLUGIN.into()),
    };
    let mode = envelope(command, &format!("/{PLUGIN}:reader")).await;
    let t = w
        .turn(&engine(None), request(Some(mode), None, &w.project))
        .await;
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

    let mut plan = w.skill_request(None).await;
    plan.metadata.insert(
        EXEC_TIER_SESSION_KEY.to_string(),
        ExecTier::Plan.id().to_string(),
    );
    let t = w.turn(&engine(None), plan).await;
    assert_eq!(t.permissions.tier, ExecTier::Plan);
    assert!(
        !t.runs("file_write").await,
        "plan's floor lost to the pre-grant"
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

/// Only the split writes or clears the pre-grant, and only the turn's
/// permission resolution reads it — across every production file under
/// `src/`. So neither `skill_read`, a resume, a queue replay nor any other
/// producer can put one on a turn.
#[test]
fn only_the_split_writes_the_pregrant_and_only_the_resolution_reads_it() {
    use crate::utils::source_scan::{code_keeping_literals, production_text, rust_sources_under};
    const WIRE: &str = "src/gateway/execution_engine/slash_skill_scope.rs";
    const SPLIT: &str = "src/gateway/execution_engine/slash_skill_pregrant/mod.rs";
    const RESOLVE: &str = "src/gateway/execution_engine/turn_permissions.rs";
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let sources = rust_sources_under(&root);
    assert!(
        sources.len() > 500,
        "the walk found only {} files",
        sources.len()
    );
    let key = format!("\"{}\"", slash_skill_scope::SLASH_SKILL_PREGRANT_TOOLS_KEY);
    let owners: [(&str, &[&str]); 4] = [
        (key.as_str(), &[WIRE]),
        ("stamp_pregrant_from_names(", &[WIRE, SPLIT]),
        ("forget_pregrant(", &[WIRE, SPLIT]),
        ("pregrant_from_metadata(", &[WIRE, RESOLVE]),
    ];
    let mut offenders = Vec::new();
    let mut seen = [false; 4];
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
    assert_eq!(seen, [true; 4], "a needle matched nothing: {owners:?}");
    assert!(offenders.is_empty(), "{offenders:?}");
}

/// `execute.rs` runs the split exactly once, at the top level of
/// `execute()`'s body — for every request, not only for one that carries a
/// slash mode, or a pre-grant the request arrived with would survive on a
/// plain turn — and before the fast path. The restriction is no longer
/// stamped there by any other route. The resolution folds the pre-grant
/// into the one merge, after the channel layer and before the all-default
/// check; the steering rescue strips it from the metadata it re-drives.
#[test]
fn the_split_runs_for_every_request_and_folds_into_the_one_merge() {
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
