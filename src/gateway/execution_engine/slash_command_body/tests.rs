//! The render, the consent-gated runner through the production builder, the
//! model pin, the rescue strip, and the two fire sites.

use super::*;
use crate::discovery::DiscoveryConfig;
use crate::extension::{ExtensionConfig, PluginKind, PluginOrigin, PluginRecord};
use crate::gateway::agent_instance::AgentInstance;
use tempfile::TempDir;

const PLUGIN: &str = "plug";

/// The unit-test engine (`execution_engine::tests::test_engine`).
type TestEngine = super::super::engine::ExecutionEngine<
    crate::thinker::SingleProviderRegistry,
    super::super::tests::EmptyToolRegistry,
>;

/// This turn's permissions as a default engine resolves them for its tool
/// gate (no configured policy).
async fn turn_permissions(request: &RunRequest, agent: &AgentInstance) -> TurnPermissions {
    super::super::tests::test_engine()
        .resolve_turn_permissions(request, agent)
        .await
}

fn command(body: &str) -> SkillRegistration {
    SkillRegistration {
        name: "greet".into(),
        plugin_id: PLUGIN.into(),
        skill_type: SkillType::Command,
        content: body.into(),
        source_path: PathBuf::from("/nowhere/commands/greet.md"),
        ..Default::default()
    }
}

struct Never;

#[async_trait::async_trait]
impl InlineShell for Never {
    async fn run(&self, cmd: &str, _args: &InlineArgs<'_>) -> Result<String, String> {
        Err(format!("[!`{cmd}` withheld]"))
    }
}

/// Plugin `plug` installed at `<tmp>/plug` with its `greet` command under
/// `commands/`, a run workspace `<tmp>/work`, a consent registry of its own,
/// and an extension manager that holds the plugin's record and the command.
struct Fixture {
    _tmp: TempDir,
    root: PathBuf,
    work: PathBuf,
    consent: Arc<ShellHookConsent>,
    manager: Arc<ExtensionManager>,
}

impl Fixture {
    async fn new(body: &str, model: Option<&str>) -> Self {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join(PLUGIN);
        std::fs::create_dir_all(root.join("commands")).unwrap();
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let consent = Arc::new(ShellHookConsent::with_path(tmp.path().join("allow.json")));
        let manager = Arc::new(
            ExtensionManager::new(ExtensionConfig {
                discovery: DiscoveryConfig {
                    working_dir: tmp.path().to_path_buf(),
                    scan_claude_dirs: false,
                    scan_project_dirs: false,
                    max_upward_depth: 0,
                    claude_home_override: None,
                },
                plugins_config_path: Some(tmp.path().join("plugins.toml")),
                ..Default::default()
            })
            .await
            .unwrap(),
        );
        {
            let mut registry = manager.get_plugin_registry_mut().await;
            let mut record = PluginRecord::new(
                PLUGIN.into(),
                PLUGIN.into(),
                PluginKind::Static,
                PluginOrigin::Global,
            );
            record.root_dir = root.clone();
            registry.register_plugin(record);
            registry.register_skill(SkillRegistration {
                source_path: root.join("commands/greet.md"),
                model: model.map(str::to_string),
                ..command(body)
            });
        }
        Self {
            _tmp: tmp,
            root,
            work,
            consent,
            manager,
        }
    }

    /// Approve `cmd` the way `aleph-server hooks test` does: a fire records it
    /// pending with its root, and the approval is bound to that root.
    fn approve(&self, cmd: &str) {
        self.consent.record_pending(
            PLUGIN,
            &ScopeKey::Global,
            cmd,
            INLINE_COMMAND_EVENT,
            &self.root,
        );
        let fingerprint = ShellHookConsent::fingerprint(PLUGIN, None, cmd);
        self.consent
            .approve(&fingerprint, Some(&self.root))
            .unwrap()
            .expect("the pending entry is approved");
    }

    fn shell(&self, cwd: Option<PathBuf>) -> ConsentedShell {
        ConsentedShell {
            plugin_id: PLUGIN.into(),
            scope: ScopeKey::Global,
            plugin_root: self.root.clone(),
            cwd,
            settings_env: Vec::new(),
            consent: Arc::clone(&self.consent),
        }
    }

    /// `/plug:greet <args>` through `render_command` — the registry, the
    /// record, the settings and the consented shell production uses — with
    /// `<tmp>/work` as the run's directory: its `<command>` block.
    async fn block(&self, args: &str) -> String {
        let mode = serde_json::json!({
            "type": "skill", "skill_id": "plug:greet", "owning_plugin": PLUGIN, "args": args
        });
        render_command(
            &mode,
            Some(self.work.clone()),
            None,
            &self.manager,
            Arc::clone(&self.consent),
        )
        .await
        .expect("renders")
        .expect("a command turn")
        .1
    }

    /// The two production steps for `request`: [`admit`] (the fast path's
    /// fallthrough arm), then [`render_admitted`] (the run loop, after its
    /// turn-start seams) in the run's own directory, under the permissions a
    /// default engine resolves for the turn.
    async fn admit_and_render(
        &self,
        request: &mut RunRequest,
        agent: &AgentInstance,
    ) -> Result<Option<String>, String> {
        self.admit_and_render_on(&super::super::tests::test_engine(), request, agent)
            .await
    }

    /// [`Self::admit_and_render`] under the permissions `engine` resolves for
    /// the turn — its configured policy, the request's tier and channel layer.
    async fn admit_and_render_on(
        &self,
        engine: &TestEngine,
        request: &mut RunRequest,
        agent: &AgentInstance,
    ) -> Result<Option<String>, String> {
        admit_with(request, &self.manager).await?;
        let run_dir = super::super::run_loop::run_workspace(request, agent);
        let permissions = engine.resolve_turn_permissions(request, agent).await;
        render_admitted(
            request,
            &run_dir,
            &permissions,
            Some(&self.manager),
            Arc::clone(&self.consent),
            &CancellationToken::new(),
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// A `/plug:greet World` request whose slash mode is the real envelope —
    /// the catalog's entry for the command, the command parser, and the
    /// serializer both slash faces stamp with — run in `workspace`.
    async fn request(&self, workspace: PathBuf) -> RunRequest {
        let entry = catalog_entry("plug:greet", Some(PLUGIN));
        envelope_request(entry, "/plug:greet World", workspace).await
    }
}

/// The catalog's entry for a slash command: `id`, owned by `plugin` (a
/// plugin command's row) or by nobody (a bundled or user skill's row).
fn catalog_entry(id: &str, plugin: Option<&str>) -> crate::skill::SkillInfo {
    crate::skill::SkillInfo {
        id: id.into(),
        name: id.rsplit(':').next().unwrap_or(id).into(),
        description: "a slash command".into(),
        scope: crate::domain::skill::PromptScope::System,
        version: None,
        allowed_tools: None,
        argument_hint: None,
        plugin_id: plugin.map(str::to_string),
    }
}

/// A request for `input` whose slash mode is the real envelope — `entry`
/// registered in a catalog, the command parser, and the serializer both
/// slash faces stamp with — run in `workspace`.
async fn envelope_request(
    entry: crate::skill::SkillInfo,
    input: &str,
    workspace: PathBuf,
) -> RunRequest {
    let catalog = crate::sync_primitives::Arc::new(crate::tool_metadata::ToolCatalog::new());
    catalog.register_skills(&[entry]).await;
    let parsed = crate::command::CommandParser::new(catalog)
        .parse_async(input)
        .await
        .expect("the command resolves");
    let mode = crate::gateway::inbound_router::serialize_parsed_command(&parsed)
        .expect("a command serializes");
    let session = crate::gateway::router::SessionKey::main("slash-command-body");
    let mut request = super::super::tests::gate_test_request(&session, "cmd-run");
    request.input = input.into();
    request
        .metadata
        .insert(SLASH_COMMAND_MODE_KEY.to_string(), mode);
    request.workspace_override = Some(workspace);
    request
}

fn no_args() -> InlineArgs<'static> {
    InlineArgs {
        raw: "",
        positional: &[],
    }
}

fn canonical(path: &std::path::Path) -> String {
    path.canonicalize().unwrap().display().to_string()
}

#[tokio::test]
async fn the_rendered_body_is_wrapped_as_a_command_block() {
    let reg = command("Say hello to $1 and mention QA_MARKER_$1.");
    let rendered = render_registration(&reg, "World", Some(&Never))
        .await
        .unwrap();
    assert_eq!(rendered, "Say hello to World and mention QA_MARKER_World.");
    let block = wrap_block("plug:greet", "plug", &rendered);
    assert!(
        block.starts_with("<command name=\"greet\" plugin=\"plug\" invoked=\"/plug:greet\">"),
        "{block}"
    );
    assert!(block.ends_with("</command>"));
    assert!(block.contains("QA_MARKER_World"));
}

/// `/my-skill` resolves through the same `type: "skill"` envelope; only a
/// `SkillType::Command` registration renders a body.
#[tokio::test]
async fn a_skill_mode_that_names_a_real_skill_is_not_a_command_turn() {
    let f = Fixture::new("body", None).await;
    f.manager
        .get_plugin_registry_mut()
        .await
        .register_skill(SkillRegistration {
            name: "helper".into(),
            skill_type: SkillType::Skill,
            ..command("a skill body")
        });
    for mode in [
        serde_json::json!({"type": "skill", "skill_id": "plug:helper", "owning_plugin": PLUGIN, "args": ""}),
        serde_json::json!({"type": "skill", "skill_id": "not-registered", "args": ""}),
        serde_json::json!({"type": "direct_tool", "tool_id": "plug:greet", "args": ""}),
    ] {
        let turn = render_command(&mode, None, None, &f.manager, Arc::clone(&f.consent))
            .await
            .unwrap();
        assert!(turn.is_none(), "{mode}");
    }
}

/// S1. A bundled or user skill and a plugin command can share a bare name
/// (`/code-review`). The parser resolved the SKILL's row — no owner, so the
/// owner gate admitted it — and the plugin's command must not render in its
/// place: not its body, not its model, above all not its approved inline
/// commands, whether or not the session can see that plugin.
#[tokio::test]
#[cfg(unix)]
async fn a_bare_skill_never_renders_a_same_named_plugin_command() {
    let f = Fixture::new("Squatted [!`touch RAN`]", Some("claude-sonnet-5")).await;
    f.approve("touch RAN");
    let temp = TempDir::new().unwrap();
    let agent = super::super::tests::gate_test_agent(&temp, "cmd-agent").await;
    let mut request =
        envelope_request(catalog_entry("greet", None), "/greet World", f.work.clone()).await;

    let block = f
        .admit_and_render(&mut request, &agent)
        .await
        .expect("the skill's turn goes ahead");
    assert_eq!(block, None, "rendered a plugin command for a skill");
    assert!(
        !f.work.join("RAN").exists(),
        "a plugin's inline command ran for a skill"
    );
    assert_eq!(
        request.model_override, None,
        "a plugin command's model pinned a skill's turn"
    );
}

/// S1. The registration rendered is the one the owner gate judged: a mode
/// whose owner is another plugin, a bare id even with the right owner, and a
/// mode with no owner at all name no command of `plug`.
#[tokio::test]
async fn only_the_judged_owners_command_under_its_exact_key_renders() {
    let f = Fixture::new("Hello [!`echo hi`]", None).await;
    for mode in [
        serde_json::json!({"type": "skill", "skill_id": "plug:greet", "owning_plugin": "other", "args": ""}),
        serde_json::json!({"type": "skill", "skill_id": "greet", "owning_plugin": PLUGIN, "args": ""}),
        serde_json::json!({"type": "skill", "skill_id": "plug:greet", "args": ""}),
    ] {
        let turn = render_command(
            &mode,
            Some(f.work.clone()),
            None,
            &f.manager,
            Arc::clone(&f.consent),
        )
        .await
        .unwrap();
        assert!(turn.is_none(), "rendered for {mode}");
    }
    assert!(
        f.consent.entries().is_empty(),
        "an inline command was reached"
    );
}

/// Filed like a `hooks.json` command — under the plugin id, the plugin's
/// visibility key and install root — and withheld until approved.
#[tokio::test]
async fn consent_is_asked_under_the_plugin_id_and_withheld_by_default() {
    let f = Fixture::new("", None).await;
    let shell = f.shell(Some(f.work.clone()));
    let out = shell.run("echo hi", &no_args()).await;
    let placeholder = out.expect_err("unapproved must be withheld");
    assert!(
        placeholder.contains("aleph-server hooks"),
        "names the remedy"
    );

    let entries = f.consent.entries();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.plugin_name, PLUGIN);
    assert_eq!(entry.command, "echo hi");
    assert_eq!(entry.event, INLINE_COMMAND_EVENT);
    assert_eq!(entry.plugin_root.as_deref(), Some(f.root.as_path()));
    assert_eq!(entry.project_root, None);
    assert!(
        entry.invoker_arguments_note().is_some(),
        "the review surfaces say who picks the arguments"
    );

    f.consent
        .approve(&entry.fingerprint, Some(&f.root))
        .unwrap()
        .expect("approved");
    assert_eq!(shell.run("echo hi", &no_args()).await.unwrap().trim(), "hi");
}

/// `/cmd $(touch M)`: re-driven through the consented runner and an
/// approved entry, the argument is text to the shell, never code.
#[tokio::test]
#[cfg(unix)]
async fn the_arguments_never_run_as_code_through_the_consented_shell() {
    let f = Fixture::new("[!`echo $ARGUMENTS`] [!`echo $1`]", None).await;
    f.approve("echo $ARGUMENTS");
    f.approve("echo $1");

    let block = f.block("$(touch M)").await;
    assert!(block.contains("[$(touch M)] [$(touch]"), "{block}");
    let block = f.block(";touch${IFS}N").await;
    assert!(block.contains("[;touch${IFS}N] [;touch${IFS}N]"), "{block}");
    for dir in [&f.work, &f.root] {
        assert!(!dir.join("M").exists(), "the argument ran as code");
        assert!(!dir.join("N").exists(), "the argument ran as code");
    }
}

/// The other half, through the same runner: the arguments DO arrive —
/// positional, defaulted and whole — so "nothing ran" above is not "nothing
/// arrived".
#[tokio::test]
#[cfg(unix)]
async fn the_arguments_reach_an_inline_command_through_the_consented_shell() {
    let cmd = r#"printf '%s' "$1|$2|${3:-none}|$ARGUMENTS|$#""#;
    let f = Fixture::new(&format!("[!`{cmd}`]"), None).await;
    f.approve(cmd);
    let block = f.block("a  b").await;
    assert!(block.contains("[a|b|none|a  b|2]"), "{block}");
}

/// The run's directory, and the plugin's INSTALL root — not its `commands/`
/// directory, which is where the command file lives.
#[tokio::test]
#[cfg(unix)]
async fn an_inline_command_runs_in_the_run_directory_with_the_install_root() {
    let root_cmd = r#"printf '%s' "$CLAUDE_PLUGIN_ROOT""#;
    let f = Fixture::new(&format!("[!`pwd -P`] [!`{root_cmd}`]"), None).await;
    f.approve("pwd -P");
    f.approve(root_cmd);
    let block = f.block("").await;
    assert!(
        block.contains(&format!("[{}]", canonical(&f.work))),
        "not the run's directory: {block}"
    );
    assert!(
        block.contains(&format!("[{}]", f.root.display())),
        "not the install root: {block}"
    );
}

#[tokio::test]
async fn an_approved_inline_command_is_withheld_when_no_directory_is_known() {
    let f = Fixture::new("", None).await;
    f.approve("echo hi");
    let out = f.shell(None).run("echo hi", &no_args()).await;
    assert_eq!(
        out,
        Err("[!`echo hi` not run: no working directory is known for this turn]".to_string())
    );
}

/// S3. Consent reviews and hashes a relative script in the plugin's ROOT,
/// while an inline command runs in the SESSION's directory: an approved
/// `sh scripts/check.sh` would attest to the plugin's copy and run the
/// project's. It is withheld, approval or not; the plugin's own script,
/// written through `${CLAUDE_PLUGIN_ROOT}`, runs.
#[tokio::test]
#[cfg(unix)]
async fn an_inline_command_naming_a_relative_script_never_runs() {
    let rooted = "sh ${CLAUDE_PLUGIN_ROOT}/scripts/check.sh";
    let f = Fixture::new(&format!("[!`sh scripts/check.sh`] [!`{rooted}`]"), None).await;
    for (dir, script) in [
        (&f.root, "printf PLUGIN\n"),
        (&f.work, "touch RAN; printf PROJECT\n"),
    ] {
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join("scripts/check.sh"), script).unwrap();
    }
    f.approve("sh scripts/check.sh");
    f.approve(rooted);

    let block = f.block("").await;
    assert!(
        block.contains("[[!`sh scripts/check.sh` not run: `scripts/check.sh` is a relative path"),
        "{block}"
    );
    assert!(!f.work.join("RAN").exists(), "the project's script ran");
    assert!(
        block.contains("[PLUGIN]"),
        "the plugin's own script did not run: {block}"
    );
}

/// N3. What a command writes to and a URL are not its script: an approved
/// `git status --short 2>/dev/null` runs, an unapproved `curl … https://…` is
/// filed for review like any command (not refused as a relative path), and a
/// relative script is still refused with its output redirected.
#[tokio::test]
#[cfg(unix)]
async fn a_redirection_or_a_url_is_not_a_relative_script() {
    let status = "git status --short 2>/dev/null";
    let curl = "curl -s https://example.com/x";
    let script = "sh scripts/x.sh 2>&1";
    let f = Fixture::new(&format!("[!`{status}`] [!`{curl}`] [!`{script}`]"), None).await;
    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&f.work)
        .status()
        .expect("git runs");
    assert!(init.success());
    std::fs::write(f.work.join("new.txt"), "x").unwrap();
    f.approve(status);

    let block = f.block("").await;
    assert!(block.contains("[?? new.txt"), "{block}");
    assert!(
        block.contains(&format!("[[!`{curl}` not run: pending operator approval")),
        "{block}"
    );
    assert!(
        f.consent.entries().iter().any(|e| e.command == curl),
        "the URL command was not filed for review"
    );
    assert!(
        block.contains(&format!(
            "[[!`{script}` not run: `scripts/x.sh` is a relative path"
        )),
        "{block}"
    );
}

/// S7. The consent key has no event, so a `hooks.json` command with the same
/// text shares it. An approval given to the hook — reviewed without
/// arguments, in its root — does not cover the inline face.
#[tokio::test]
async fn an_approval_given_to_a_hook_does_not_run_the_inline_command() {
    let f = Fixture::new("", None).await;
    f.consent
        .record_pending(PLUGIN, &ScopeKey::Global, "echo hi", "PreToolUse", &f.root);
    let key = ShellHookConsent::fingerprint(PLUGIN, None, "echo hi");
    f.consent
        .approve(&key, Some(&f.root))
        .unwrap()
        .expect("approved as a hook");

    let out = f
        .shell(Some(f.work.clone()))
        .run("echo hi", &no_args())
        .await;
    let placeholder = out.expect_err("a hook's approval runs no inline command");
    assert!(placeholder.contains("given to a hook"), "{placeholder}");
}

/// The plugin's settings reach an inline command, minus every key that
/// holds a vault reference: whoever sends `/command` picks its arguments.
#[tokio::test]
#[cfg(unix)]
async fn an_inline_command_gets_the_plugins_settings_but_no_secret() {
    let cmd = r#"printf '%s|%s|%s' "$CLAUDE_PLUGIN_OPTION_ENDPOINT" "${CLAUDE_PLUGIN_OPTION_TOKEN-unset}" "$ALEPH_PLUGIN_CONFIG""#;
    let f = Fixture::new(&format!("[!`{cmd}`]"), None).await;
    f.manager
        .set_plugin_settings(
            PLUGIN,
            serde_json::json!({"endpoint": "https://api.example", "token": "{{secret:TOKEN}}"}),
        )
        .await
        .expect("settings accepted");
    f.approve(cmd);
    let block = f.block("").await;
    assert!(block.contains("[https://api.example|unset|{"), "{block}");
    assert!(block.contains("\"endpoint\""), "{block}");
    assert!(
        !block.contains("token"),
        "a secret-backed key leaked: {block}"
    );
    assert!(
        !block.contains("secret"),
        "a vault reference leaked: {block}"
    );
}

/// N10. A command renders only while its plugin has an active record here.
/// With no record (an orphan registration), or disabled — which leaves its
/// registrations in place — the mode names no command: nothing is admitted,
/// nothing renders, nothing is filed for review. A command admitted while its
/// plugin was active fails the turn visibly when the plugin is disabled
/// before the run loop renders it.
#[tokio::test]
async fn a_command_renders_only_while_its_plugin_is_active_here() {
    let temp = TempDir::new().unwrap();
    let agent = super::super::tests::gate_test_agent(&temp, "cmd-agent").await;

    let f = Fixture::new("body", None).await;
    f.manager
        .get_plugin_registry_mut()
        .await
        .register_skill(SkillRegistration {
            plugin_id: "orphan".into(),
            ..command("Hi $1 [!`echo hi`]")
        });
    let orphan = serde_json::json!({
        "type": "skill", "skill_id": "orphan:greet", "owning_plugin": "orphan", "args": "you"
    });
    let rendered = render_command(
        &orphan,
        Some(f.work.clone()),
        None,
        &f.manager,
        Arc::clone(&f.consent),
    )
    .await
    .unwrap();
    assert_eq!(rendered, None, "an orphan registration rendered");

    let f = Fixture::new("Hi [!`echo hi`]", None).await;
    assert!(f
        .manager
        .get_plugin_registry_mut()
        .await
        .disable_plugin(PLUGIN));
    assert!(
        f.manager
            .get_plugin_registry()
            .await
            .get_skill("plug:greet")
            .is_some(),
        "disabling a plugin keeps its registrations"
    );
    let mut request = f.request(f.work.clone()).await;
    let rendered = f.admit_and_render(&mut request, &agent).await.unwrap();
    assert_eq!(rendered, None, "a disabled plugin's command rendered");
    assert!(!request.metadata.contains_key(ADMITTED_KEY));

    let f = Fixture::new("Hi [!`echo hi`]", None).await;
    let mut request = f.request(f.work.clone()).await;
    admit_with(&mut request, &f.manager).await.unwrap();
    assert!(request.metadata.contains_key(ADMITTED_KEY));
    f.manager
        .get_plugin_registry_mut()
        .await
        .disable_plugin(PLUGIN);
    let run_dir = super::super::run_loop::run_workspace(&request, &agent);
    let refused = render_admitted(
        &request,
        &run_dir,
        &turn_permissions(&request, &agent).await,
        Some(&f.manager),
        Arc::clone(&f.consent),
        &CancellationToken::new(),
    )
    .await
    .expect_err("a command disabled after admission must not render");
    assert!(
        refused
            .to_string()
            .contains("no longer an active plugin's command"),
        "{refused}"
    );
    assert!(
        f.consent.entries().is_empty(),
        "nothing was filed for review"
    );
}

#[test]
fn a_declared_model_pins_this_turn_only_when_the_request_has_none() {
    let raw = |model: &str| {
        Some(ModelOverride::Raw {
            model: model.into(),
        })
    };
    assert_eq!(
        command_model_pin(None, Some("claude-sonnet-5")),
        Ok(raw("claude-sonnet-5"))
    );
    // An id the catalog does not know is pinned as written — as
    // `select_model` accepts it; the provider then answers for it.
    assert_eq!(
        command_model_pin(None, Some("some-new-model")),
        Ok(raw("some-new-model"))
    );
    let user = ModelOverride::Raw {
        model: "gpt-5".into(),
    };
    assert_eq!(
        command_model_pin(Some(&user), Some("claude-sonnet-5")),
        Ok(None),
        "the user's pick wins"
    );
    // Claude Code's aliases name no routable id (no alias table): not applied.
    for declared in ["sonnet", "opus", "haiku", "inherit", "  ", ""] {
        assert_eq!(
            command_model_pin(None, Some(declared)),
            Ok(None),
            "{declared:?}"
        );
    }
    assert_eq!(command_model_pin(None, None), Ok(None));
    // A retired id would fail at the provider: refused before the turn runs.
    let why = command_model_pin(None, Some("deepseek-reasoner")).unwrap_err();
    assert!(why.contains("retired"), "{why}");
}

/// Both production steps below the fast path: the real slash envelope in,
/// the rendered block handed to the run loop's TRANSIENT blocks and nowhere
/// else — no metadata value holds it — the raw `/command` still the input
/// (the text the run loop persists as the user turn), and the command's model
/// as this turn's override. The rescue then strips the admission and the pin.
#[tokio::test]
#[cfg(unix)]
async fn an_admitted_command_renders_for_the_transient_blocks_and_leaves_the_input_raw() {
    let f = Fixture::new("Say hello to $1 from !`pwd -P`.", Some("claude-sonnet-5")).await;
    f.approve("pwd -P");
    let temp = TempDir::new().unwrap();
    let agent = super::super::tests::gate_test_agent(&temp, "cmd-agent").await;
    let mut request = f.request(f.work.clone()).await;

    let block = f
        .admit_and_render(&mut request, &agent)
        .await
        .expect("the command renders")
        .expect("a command block");
    assert!(
        block.starts_with("<command name=\"greet\" plugin=\"plug\" invoked=\"/plug:greet\">"),
        "{block}"
    );
    assert!(
        block.contains(&format!("Say hello to World from {}.", canonical(&f.work))),
        "{block}"
    );
    assert_eq!(
        request.input, "/plug:greet World",
        "the persisted turn is raw"
    );
    let carriers: Vec<&String> = request
        .metadata
        .iter()
        .filter(|(_, value)| value.contains("Say hello to World"))
        .map(|(key, _)| key)
        .collect();
    assert!(
        carriers.is_empty(),
        "the body rides in metadata: {carriers:?}"
    );
    assert_eq!(
        request.metadata.get(ADMITTED_KEY).map(String::as_str),
        Some("plug:greet")
    );
    assert_eq!(
        request.model_override,
        Some(ModelOverride::Raw {
            model: "claude-sonnet-5".into()
        })
    );

    let kept = strip(&mut request.metadata, request.model_override.clone());
    assert_eq!(kept, None, "the command's pin was for its own turn");
    let run_dir = super::super::run_loop::run_workspace(&request, &agent);
    let again = render_admitted(
        &request,
        &run_dir,
        &turn_permissions(&request, &agent).await,
        Some(&f.manager),
        Arc::clone(&f.consent),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(again, None, "a stripped request renders its command again");
}

/// The composer's pick outranks the command's `model:`, and survives the
/// rescue: only a pin the command made is stripped.
#[tokio::test]
async fn the_users_model_pick_survives_the_command_and_the_rescue() {
    let f = Fixture::new("Hi", Some("claude-sonnet-5")).await;
    let temp = TempDir::new().unwrap();
    let agent = super::super::tests::gate_test_agent(&temp, "cmd-agent").await;
    let mut request = f.request(f.work.clone()).await;
    let user = ModelOverride::Raw {
        model: "gpt-5".into(),
    };
    request.model_override = Some(user.clone());

    let block = f.admit_and_render(&mut request, &agent).await.unwrap();
    assert_eq!(request.model_override.as_ref(), Some(&user));
    assert!(block.is_some());
    assert_eq!(
        strip(&mut request.metadata, request.model_override.clone()),
        Some(user)
    );
}

/// The run's directory is checked at render time: a vanished project folder
/// withholds the inline commands with the visible placeholder.
#[tokio::test]
async fn a_vanished_run_directory_withholds_the_inline_commands() {
    let f = Fixture::new("[!`echo hi`]", None).await;
    f.approve("echo hi");
    let temp = TempDir::new().unwrap();
    let agent = super::super::tests::gate_test_agent(&temp, "cmd-agent").await;
    let mut request = f.request(f.work.join("gone")).await;
    let block = f
        .admit_and_render(&mut request, &agent)
        .await
        .unwrap()
        .unwrap();
    assert!(
        block.contains("[[!`echo hi` not run: no working directory is known for this turn]]"),
        "{block}"
    );
}

/// A `model:` the provider would fail stops the turn at admission, BEFORE the
/// body renders: nothing is admitted, so the run loop renders nothing and the
/// approved inline command never runs.
#[tokio::test]
#[cfg(unix)]
async fn a_retired_model_stops_the_turn_before_any_inline_command_runs() {
    let f = Fixture::new("!`touch RAN`", Some("deepseek-reasoner")).await;
    f.approve("touch RAN");
    let temp = TempDir::new().unwrap();
    let agent = super::super::tests::gate_test_agent(&temp, "cmd-agent").await;
    let mut request = f.request(f.work.clone()).await;

    let why = f
        .admit_and_render(&mut request, &agent)
        .await
        .expect_err("a retired model refuses the turn");
    assert!(why.starts_with("/plug:greet was not run:"), "{why}");
    assert!(!request.metadata.contains_key(ADMITTED_KEY));
    let run_dir = super::super::run_loop::run_workspace(&request, &agent);
    let rendered = render_admitted(
        &request,
        &run_dir,
        &turn_permissions(&request, &agent).await,
        Some(&f.manager),
        Arc::clone(&f.consent),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(rendered, None);
    assert!(!f.work.join("RAN").exists(), "an inline command ran");
    assert_eq!(request.model_override, None);
}

/// Nobody under `execution_engine/` spells the two metadata keys but this
/// module — every reader goes through `render_admitted` / `strip`, so a
/// second reader cannot render the command again (a continuation) or keep
/// the command's model past its turn. Literals are kept and comments dropped.
#[test]
fn only_this_module_spells_its_metadata_keys() {
    use crate::utils::source_scan::{code_keeping_literals, production_text, rust_sources_under};
    const OWN: &str = "src/gateway/execution_engine/slash_command_body/";
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/gateway/execution_engine");
    let sources = rust_sources_under(&root);
    assert!(
        sources.len() > 10,
        "the walk found only {} files",
        sources.len()
    );
    let mut offenders = Vec::new();
    for (rel, text) in &sources {
        if rel.contains(OWN) {
            continue;
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        let code = code_keeping_literals(&production_text(&path, text));
        for key in [ADMITTED_KEY, MODEL_PIN_KEY] {
            if code.contains(&format!("\"{key}\"")) {
                offenders.push(format!("{rel}: {key}"));
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:?}");
}

/// The fire sites, by position:
/// - the owner gate runs inside the fast path, before its dispatch;
/// - `execute.rs` admits exactly once, inside the fast path's `Fallthrough`
///   arm — after face ④ judged the owner, never for a command the fast path
///   refused or served — with no `active_runs` guard alive across the call
///   (its refusal takes that lock again), and then records the run's model
///   for the busy lane;
/// - `run_loop/inner.rs` renders exactly once, AFTER its `UserPromptSubmit`
///   seam (and so after `BeforeAgentStart`, which fires earlier in
///   `run_agent_loop`) and before the transient context is joined, in the
///   run's own directory, and inserts the block at index 0 — the command is
///   the turn's instruction, every reminder annotates it.
///
/// Comment lines are stripped first, so the prose that names the anchors can
/// neither satisfy nor defeat a `find`.
#[test]
fn the_body_renders_after_the_owner_gate_and_is_pushed_first() {
    use crate::utils::source_scan::{production_prefix, strip_comment_lines};
    let once = |code: &str, needle: &str| -> usize {
        assert_eq!(
            code.matches(needle).count(),
            1,
            "`{needle}` must occur exactly once"
        );
        code.find(needle).expect("counted once above")
    };

    // "After the owner gate" means the gate is still where the fast path
    // runs it: inside `execute_slash_command_fast_path`, before it dispatches
    // on the mode's type.
    let slash = strip_comment_lines(&production_prefix(include_str!("../slash_command.rs")));
    let fast_path_fn = once(&slash, "async fn execute_slash_command_fast_path");
    let gate = once(&slash, "slash_owner_admits(");
    let dispatch = once(&slash, "match mode_type {");
    assert!(
        fast_path_fn < gate && gate < dispatch,
        "the owner gate must run inside the fast path, before its dispatch — positions: fast \
         path {fast_path_fn}, gate {gate}, dispatch {dispatch}"
    );

    let execute = strip_comment_lines(&production_prefix(include_str!("../execute.rs")));
    let fast_path = once(&execute, ".execute_slash_command_fast_path(");
    let fallthrough = once(
        &execute,
        "Err(ExecutionError::Fallthrough { ref reason }) => {",
    );
    let admit = once(&execute, "slash_command_body::admit(");
    let marked = once(&execute, ".mark_fallen_through(");
    let refused = execute
        .get(fallthrough..)
        .and_then(|rest| rest.find("Err(ref e) => {"))
        .map(|at| at + fallthrough)
        .expect("the fast path's refusal arm follows the fallthrough arm");
    assert!(
        fast_path < fallthrough && fallthrough < admit && admit < marked && marked < refused,
        "execute.rs must admit the command inside the Fallthrough arm, after the fast path \
         (face ④), then record the run's model — positions: fast path {fast_path}, fallthrough \
         {fallthrough}, admit {admit}, mark {marked}, refusal arm {refused}"
    );
    let arm = execute.get(fallthrough..refused).unwrap_or_default();
    assert!(
        !arm.contains("active_runs"),
        "the Fallthrough arm must not hold `active_runs` itself: a guard alive across \
         `admit` deadlocks its refusal"
    );

    let inner = strip_comment_lines(&production_prefix(include_str!("../run_loop/inner.rs")));
    let prompt_seam = once(&inner, "execute_interceptors(HookEvent::UserPromptSubmit");
    let render = once(&inner, "slash_command_body::render_admitted(");
    let first = once(&inner, "transient_blocks.insert(0, block)");
    let joined = once(&inner, "let transient_context =");
    assert!(
        prompt_seam < render && render < first && first < joined,
        "inner.rs must render the command after its UserPromptSubmit seam and put it first — \
         positions: seam {prompt_seam}, render {render}, insert {first}, joined {joined}"
    );
    let call = inner.get(render..first).unwrap_or_default();
    assert!(
        call.contains("&effective_workspace"),
        "the render runs in the run's own directory (`run_workspace`)"
    );
    assert!(
        call.contains("&turn_permissions"),
        "the render judges the permissions this turn's tool gate is built from"
    );
}

/// Two more fire sites no behavioural test here can reach (the rescue needs
/// an orchestrator):
/// - the steering rescue strips the command's residue from the metadata it
///   clones, before it builds the continuation — or the body is delivered,
///   and the command's model pinned, a second time;
/// - "where does this run work" has ONE derivation: the helper holds the
///   only `agent.workspace()` fallback, and its two callers (the run's
///   task-locals, `effective_workspace`) call it; the render runs in
///   `effective_workspace` (pinned above).
#[test]
fn the_rescue_strips_the_command_and_the_run_directory_has_one_derivation() {
    use crate::utils::source_scan::{production_prefix, strip_comment_lines};
    let code = |src: &str| strip_comment_lines(&production_prefix(src));

    let steering = code(include_str!("../steering.rs"));
    let after = |from: usize, needle: &str| -> usize {
        steering
            .get(from..)
            .and_then(|rest| rest.find(needle))
            .map(|at| at + from)
            .unwrap_or_else(|| panic!("`{needle}` after {from}"))
    };
    assert_eq!(steering.matches("slash_command_body::strip(").count(), 1);
    let rescue = after(0, "fn build_steering_rescue_request(");
    let cloned = after(rescue, "request.metadata.clone()");
    let stripped = after(rescue, "slash_command_body::strip(");
    let built = after(rescue, "Some(RunRequest {");
    assert!(
        cloned < stripped && stripped < built,
        "the rescue must strip the command residue from the metadata it clones, before it \
         builds the continuation — positions: cloned {cloned}, strip {stripped}, built {built}"
    );

    let readers = [
        code(include_str!("../run_loop/mod.rs")),
        code(include_str!("../run_loop/inner.rs")),
        code(include_str!("mod.rs")),
    ];
    let count = |needle: &str| -> usize { readers.iter().map(|c| c.matches(needle).count()).sum() };
    assert_eq!(
        count("agent.workspace()"),
        1,
        "a second copy of the run-directory fallback"
    );
    assert_eq!(
        count("run_workspace(request, "),
        2,
        "a reader stopped calling `run_workspace`"
    );
}

/// Admit `/plug:greet` (body: an approved `touch RAN`) the way the fallthrough
/// arm does, then drive the run loop that renders it — `run_agent_loop_inner`
/// with an optional `UserPromptSubmit` interceptor running `hook`, on a
/// request carrying `metadata` too. Returns whether the inline command ran.
/// The run runs in the agent's own workspace (no `workspace_override`: an
/// override would be registered in the real project catalogue).
#[cfg(unix)]
async fn run_loop_with_prompt_hook(hook: Option<&str>, metadata: &[(&str, &str)]) -> bool {
    use crate::extension::hooks::HookExecutor;
    use crate::extension::{HookAction, HookConfig, HookEvent, HookKind, HookPriority};
    let f = Fixture::new("[!`touch RAN`]", None).await;
    f.approve("touch RAN");
    let temp = TempDir::new().unwrap();
    let agent = super::super::tests::gate_test_agent(&temp, "cmd-agent").await;
    std::fs::create_dir_all(agent.workspace()).unwrap();
    let mut request = f.request(f.work.clone()).await;
    request.workspace_override = None;
    for (key, value) in metadata {
        request.metadata.insert(key.to_string(), value.to_string());
    }
    admit_with(&mut request, &f.manager)
        .await
        .expect("admitted");

    let engine = super::super::engine::ExecutionEngine::new(
        Default::default(),
        Arc::new(crate::thinker::SingleProviderRegistry::new(
            crate::providers::create_mock_provider(),
        )),
        Arc::new(super::super::tests::EmptyToolRegistry),
        Vec::new(),
        None,
    )
    .with_inline_consent(Arc::clone(&f.consent));
    let executor = hook.map(|command| {
        Arc::new(HookExecutor::new(vec![HookConfig {
            event: HookEvent::UserPromptSubmit,
            kind: HookKind::Interceptor,
            priority: HookPriority::Normal,
            matcher: None,
            actions: vec![HookAction::Command {
                command: command.to_string(),
            }],
            plugin_name: "turn-start-test".to_string(),
            plugin_root: temp.path().to_path_buf(),
            handler: None,
            timeout_secs: None,
            declared_event: None,
            scope_key: ScopeKey::Global,
        }]))
    });
    let result = engine
        .run_agent_loop_inner(
            "run-cmd",
            &request,
            Arc::clone(&agent),
            Arc::new(crate::gateway::event_emitter::NoOpEventEmitter::new()),
            Arc::new(tokio::sync::Mutex::new(
                tokio::time::Instant::now() + Duration::from_secs(60),
            )),
            None,
            CancellationToken::new(),
            Some(Arc::clone(&f.manager)),
            executor,
            "cmd-hooks".to_string(),
            Arc::new(std::sync::Mutex::new(None)),
        )
        .await;
    // Neither run reaches a provider: the deny ends one, and the test engine
    // has no orchestrator to dispatch the other to.
    assert!(result.is_err(), "{result:?}");
    agent.workspace().join("RAN").exists()
}

/// S2. A turn-start hook that stops the turn stops the command's inline
/// shell too: the body renders in the run loop only after `UserPromptSubmit`
/// let the turn go ahead.
#[cfg(unix)]
#[tokio::test]
async fn a_turn_start_deny_hook_stops_the_commands_inline_shell() {
    assert!(
        !run_loop_with_prompt_hook(Some("echo 'deny: stopped by the test'"), &[]).await,
        "an approved inline command ran for a turn its UserPromptSubmit hook denied"
    );
}

/// N9 at the fire site: the run loop hands the render the permissions it
/// resolved for its own tool gate, so on a `plan` turn — where the model may
/// not run `bash` — an approved inline command does not run either.
#[cfg(unix)]
#[tokio::test]
async fn a_plan_turn_runs_no_inline_command_in_the_run_loop() {
    use crate::config::types::policies::{ExecTier, EXEC_TIER_SESSION_KEY};
    assert!(
        !run_loop_with_prompt_hook(None, &[(EXEC_TIER_SESSION_KEY, ExecTier::Plan.id())]).await,
        "an approved inline command ran on a plan turn"
    );
}

/// S2's other half: the same loop does render the admitted command — and
/// run its approved inline command — when nothing stops the turn.
#[cfg(unix)]
#[tokio::test]
async fn the_run_loop_renders_an_admitted_command_when_its_hooks_allow() {
    assert!(
        run_loop_with_prompt_hook(None, &[]).await,
        "the run loop never rendered the admitted command"
    );
}

/// S8. A stopped turn does not wait on its command's inline shell: the render
/// races the run's cancel token.
#[cfg(unix)]
#[tokio::test]
async fn a_cancelled_turn_renders_nothing() {
    let f = Fixture::new("[!`touch RAN`]", None).await;
    f.approve("touch RAN");
    let temp = TempDir::new().unwrap();
    let agent = super::super::tests::gate_test_agent(&temp, "cmd-agent").await;
    let mut request = f.request(f.work.clone()).await;
    admit_with(&mut request, &f.manager).await.unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let rendered = render_admitted(
        &request,
        &f.work,
        &turn_permissions(&request, &agent).await,
        Some(&f.manager),
        Arc::clone(&f.consent),
        &cancel,
    )
    .await;
    assert!(
        matches!(rendered, Err(ExecutionError::Cancelled)),
        "{rendered:?}"
    );
    assert!(
        !f.work.join("RAN").exists(),
        "a cancelled turn ran an inline command"
    );
}

/// S11. The busy lane compares an incoming message's model against the
/// RUNNING copy of the request; once a `/command` pinned its model, that copy
/// must say so — or a steer on another model folds into this run.
#[tokio::test]
async fn the_busy_lane_sees_the_model_a_command_pinned() {
    let f = Fixture::new("Hi", Some("claude-sonnet-5")).await;
    let mut request = f.request(f.work.clone()).await;
    let engine = super::super::tests::test_engine();
    engine.active_runs.write().await.insert(
        request.run_id.clone(),
        super::super::ActiveRun {
            request: request.clone(),
            state: super::super::RunState::Running,
            started_at: chrono::Utc::now(),
            admitted_at: std::time::Instant::now(),
            completed_at: None,
            steps_completed: 0,
            current_tool: None,
            cancel_tx: None,
            seq_counter: crate::sync_primitives::AtomicU64::new(0),
            chunk_counter: crate::sync_primitives::AtomicU32::new(0),
        },
    );
    admit_with(&mut request, &f.manager).await.unwrap();
    engine.mark_fallen_through(&request.run_id, &request).await;

    let sibling = super::super::steering::find_busy_sibling(
        &engine.active_runs,
        "a-later-message",
        &request.session_key,
    )
    .await
    .expect("the run is running");
    assert_eq!(
        sibling.model_override,
        Some(ModelOverride::Raw {
            model: "claude-sonnet-5".into()
        })
    );
}

/// N1. The command's `model:` is pinned before the run is admitted, so the
/// run's registered copy — the busy lane's steer target while it parks for a
/// slot — names it from the start, and admission keeps it as the command's
/// pin. The user's own pick is never replaced; a retired `model:` pins
/// nothing here and is refused at admission, after the owner gate.
#[tokio::test]
async fn a_commands_model_is_pinned_before_admission() {
    let sonnet = Some(ModelOverride::Raw {
        model: "claude-sonnet-5".into(),
    });
    let f = Fixture::new("Hi", Some("claude-sonnet-5")).await;
    let mut request = f.request(f.work.clone()).await;
    pin_model_with(&mut request, &f.manager).await;
    assert_eq!(request.model_override, sonnet);
    admit_with(&mut request, &f.manager).await.unwrap();
    assert_eq!(request.model_override, sonnet, "admission keeps the pin");
    assert_eq!(
        strip(&mut request.metadata, request.model_override.clone()),
        None,
        "the pin is the command's, not the user's"
    );

    let mut request = f.request(f.work.clone()).await;
    let user = ModelOverride::Raw {
        model: "gpt-5".into(),
    };
    request.model_override = Some(user.clone());
    pin_model_with(&mut request, &f.manager).await;
    assert_eq!(request.model_override, Some(user), "the user's pick wins");

    let retired = Fixture::new("Hi", Some("deepseek-reasoner")).await;
    let mut request = retired.request(retired.work.clone()).await;
    pin_model_with(&mut request, &retired.manager).await;
    assert_eq!(request.model_override, None);
    assert!(admit_with(&mut request, &retired.manager).await.is_err());
}

/// N1's other half: a pin made before admission whose command is then not
/// admitted (its plugin disabled in between) goes with it — the turn does
/// not run on a model nothing asked for.
#[tokio::test]
async fn a_pin_whose_command_is_not_admitted_is_dropped() {
    let f = Fixture::new("Hi", Some("claude-sonnet-5")).await;
    let mut request = f.request(f.work.clone()).await;
    pin_model_with(&mut request, &f.manager).await;
    assert!(request.model_override.is_some());
    f.manager
        .get_plugin_registry_mut()
        .await
        .disable_plugin(PLUGIN);
    admit_with(&mut request, &f.manager).await.unwrap();
    assert!(!request.metadata.contains_key(ADMITTED_KEY));
    assert!(!request.metadata.contains_key(MODEL_PIN_KEY));
    assert_eq!(request.model_override, None);
}

/// N1's fire site, which no behavioural test here reaches (`execute()` cannot
/// be driven with a chosen extension manager): the command's model is pinned
/// before `admit_run` registers the run's copy.
#[test]
fn the_command_model_is_pinned_before_the_run_is_admitted() {
    use crate::utils::source_scan::{production_prefix, strip_comment_lines};
    let execute = strip_comment_lines(&production_prefix(include_str!("../execute.rs")));
    assert_eq!(
        execute.matches("slash_command_body::pin_model(").count(),
        1,
        "`pin_model` must be called exactly once"
    );
    let pin = execute.find("slash_command_body::pin_model(&mut request)");
    let admit = execute.find(".admit_run(");
    assert!(
        matches!((pin, admit), (Some(pin), Some(admit)) if pin < admit),
        "execute.rs must pin the command's model before `admit_run` — positions: pin {pin:?}, \
         admit {admit:?}"
    );
}

/// S5. Inline shell runs only for an operator (the `/moa` precedent): a guest
/// or member sender gets the body with every inline command withheld and
/// named, whatever the turn's permissions; an operator, and a loopback caller
/// with no role at all, run it.
#[tokio::test]
#[cfg(unix)]
async fn inline_commands_run_only_for_an_operator() {
    let deny_other = r#"{"default":"allow","overrides":{"web_fetch":"deny"}}"#;
    for (role, layer, withheld) in [
        (Some("guest"), None, true),
        (Some("member"), None, true),
        (Some("guest"), Some(deny_other), true),
        (Some("operator"), None, false),
        (None, None, false),
    ] {
        let f = Fixture::new("Hi [!`touch RAN`]", None).await;
        f.approve("touch RAN");
        let temp = TempDir::new().unwrap();
        let agent = super::super::tests::gate_test_agent(&temp, "cmd-agent").await;
        let mut request = f.request(f.work.clone()).await;
        if let Some(role) = role {
            request
                .metadata
                .insert("caller_role".to_string(), role.to_string());
        }
        if let Some(layer) = layer {
            request.metadata.insert(
                super::super::CHANNEL_TOOL_PERMISSIONS_KEY.to_string(),
                layer.to_string(),
            );
        }
        let block = f
            .admit_and_render(&mut request, &agent)
            .await
            .unwrap()
            .expect("the body still renders");
        let ran = f.work.join("RAN").exists();
        if withheld {
            assert!(!ran, "{role:?} / {layer:?}: the inline command ran");
            assert!(
                block.contains(
                    "Hi [[!`touch RAN` not run: inline commands run only for an operator]]"
                ),
                "{role:?} / {layer:?}: {block}"
            );
        } else {
            assert!(ran, "{role:?} / {layer:?}: withheld: {block}");
        }
    }
}

/// N9. For an operator, an inline command runs only where the turn's tool
/// gate would let the model run `bash` — one derivation, the gate's own: the
/// global, agent and channel policies merged, and the tier. A layer that
/// denies `bash`, a `plan` turn and a `/btw` side question all withhold; a
/// policy that denies another tool does not, and neither does a channel
/// layer that cannot be read — the tool gate skips it with a warning, and
/// the inline face follows the gate rather than keep a second parse of it.
#[tokio::test]
#[cfg(unix)]
async fn a_turn_whose_tool_gate_denies_bash_runs_no_inline_command() {
    use crate::config::types::policies::{ExecTier, EXEC_TIER_SESSION_KEY};
    let deny_bash = r#"{"default":"allow","overrides":{"bash":"deny"}}"#;
    let deny_other = r#"{"default":"allow","overrides":{"web_fetch":"deny"}}"#;
    let channel = super::super::CHANNEL_TOOL_PERMISSIONS_KEY;
    let btw = crate::gateway::btw::BTW_METADATA_KEY;
    let plan = ExecTier::Plan.id();
    // (case, the install's `[policies.tool_permissions]`, request metadata, withheld)
    let cases: [(&str, Option<&str>, &[(&str, &str)], bool); 8] = [
        (
            "a channel layer denies bash",
            None,
            &[(channel, deny_bash)],
            true,
        ),
        ("the install denies bash", Some(deny_bash), &[], true),
        ("a plan turn", None, &[(EXEC_TIER_SESSION_KEY, plan)], true),
        (
            "a /btw side question",
            None,
            &[(btw, "what is this?")],
            true,
        ),
        (
            "a channel layer denies another tool",
            None,
            &[(channel, deny_other)],
            false,
        ),
        (
            "the install denies another tool",
            Some(deny_other),
            &[],
            false,
        ),
        (
            "an unreadable channel layer",
            None,
            &[(channel, "not json")],
            false,
        ),
        ("nothing configured", None, &[], false),
    ];
    for (case, install, metadata, withheld) in cases {
        let f = Fixture::new("Hi [!`touch RAN`]", None).await;
        f.approve("touch RAN");
        let temp = TempDir::new().unwrap();
        let agent = super::super::tests::gate_test_agent(&temp, "cmd-agent").await;
        let mut request = f.request(f.work.clone()).await;
        request
            .metadata
            .insert("caller_role".to_string(), "operator".to_string());
        for (key, value) in metadata {
            request.metadata.insert(key.to_string(), value.to_string());
        }
        let mut config = crate::Config::default();
        if let Some(policy) = install {
            config.policies.tool_permissions = serde_json::from_str(policy).unwrap();
        }
        let engine = super::super::tests::test_engine()
            .with_app_config(Arc::new(tokio::sync::RwLock::new(config)));
        let block = f
            .admit_and_render_on(&engine, &mut request, &agent)
            .await
            .unwrap()
            .expect("the body still renders");
        let ran = f.work.join("RAN").exists();
        if withheld {
            assert!(!ran, "{case}: the inline command ran");
            assert!(
                block.contains("Hi [[!`touch RAN` not run: this turn's permissions deny `bash`]]"),
                "{case}: {block}"
            );
        } else {
            assert!(ran, "{case}: withheld: {block}");
        }
    }
}
