//! The render, the consent-gated runner through the production builder, the
//! model pin, the rescue strip, and the two fire sites.

use super::*;
use crate::discovery::DiscoveryConfig;
use crate::extension::{ExtensionConfig, PluginKind, PluginOrigin, PluginRecord};
use tempfile::TempDir;

const PLUGIN: &str = "plug";

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

    /// Approve `cmd` the way `aleph hooks test` does: a fire records it
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
    /// turn-start seams) in the run's own directory.
    async fn admit_and_render(
        &self,
        request: &mut RunRequest,
        agent: &crate::gateway::agent_instance::AgentInstance,
    ) -> Result<Option<String>, String> {
        admit_with(request, &self.manager).await?;
        let run_dir = super::super::run_loop::run_workspace(request, agent);
        render_admitted(
            request,
            &run_dir,
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
    let rejected = catalog.register_skills(&[entry]).await;
    assert!(rejected.is_empty(), "{rejected:?}");
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
        let turn = render_command(&mode, None, &f.manager, Arc::clone(&f.consent))
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
    assert!(placeholder.contains("aleph hooks"), "names the remedy");

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

/// A command whose plugin has no record cannot be tied to a root or a
/// consent key: its inline commands are withheld, the rest renders.
#[tokio::test]
async fn a_command_without_a_plugin_record_withholds_its_inline_commands() {
    let f = Fixture::new("body", None).await;
    f.manager
        .get_plugin_registry_mut()
        .await
        .register_skill(SkillRegistration {
            plugin_id: "orphan".into(),
            ..command("Hi $1 [!`echo hi`]")
        });
    let mode = serde_json::json!({
        "type": "skill", "skill_id": "orphan:greet", "owning_plugin": "orphan", "args": "you"
    });
    let (_, block) = render_command(
        &mode,
        Some(f.work.clone()),
        &f.manager,
        Arc::clone(&f.consent),
    )
    .await
    .unwrap()
    .expect("a command turn");
    assert!(
        block.contains(
            "Hi you [[!`echo hi` not run: the plugin that ships this command has no record here]]"
        ),
        "{block}"
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
    assert!(
        inner
            .get(render..first)
            .is_some_and(|call| call.contains("&effective_workspace")),
        "the render runs in the run's own directory (`run_workspace`)"
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
/// with an optional `UserPromptSubmit` interceptor running `hook`. Returns
/// whether the inline command ran. The run runs in the agent's own workspace
/// (no `workspace_override`: an override would be registered in the real
/// project catalogue).
#[cfg(unix)]
async fn run_loop_with_prompt_hook(hook: Option<&str>) -> bool {
    use crate::extension::hooks::HookExecutor;
    use crate::extension::{HookAction, HookConfig, HookEvent, HookKind, HookPriority};
    let f = Fixture::new("[!`touch RAN`]", None).await;
    f.approve("touch RAN");
    let temp = TempDir::new().unwrap();
    let agent = super::super::tests::gate_test_agent(&temp, "cmd-agent").await;
    std::fs::create_dir_all(agent.workspace()).unwrap();
    let mut request = f.request(f.work.clone()).await;
    request.workspace_override = None;
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
        !run_loop_with_prompt_hook(Some("echo 'deny: stopped by the test'")).await,
        "an approved inline command ran for a turn its UserPromptSubmit hook denied"
    );
}

/// S2's other half: the same loop does render the admitted command — and
/// run its approved inline command — when nothing stops the turn.
#[cfg(unix)]
#[tokio::test]
async fn the_run_loop_renders_an_admitted_command_when_its_hooks_allow() {
    assert!(
        run_loop_with_prompt_hook(None).await,
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
    let mut request = f.request(f.work.clone()).await;
    admit_with(&mut request, &f.manager).await.unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let rendered = render_admitted(
        &request,
        &f.work,
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
