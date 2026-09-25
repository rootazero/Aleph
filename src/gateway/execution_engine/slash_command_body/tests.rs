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
    manager: ExtensionManager,
}

impl Fixture {
    async fn new(body: &str, model: Option<&str>) -> Self {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join(PLUGIN);
        std::fs::create_dir_all(root.join("commands")).unwrap();
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let consent = Arc::new(ShellHookConsent::with_path(tmp.path().join("allow.json")));
        let manager = ExtensionManager::new(ExtensionConfig {
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
        .unwrap();
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

    /// `/plug:greet <args>` through `resolve_command_turn` — the registry,
    /// the record, the settings and the consented shell production uses —
    /// with `<tmp>/work` as the run's directory.
    async fn turn(&self, args: &str) -> Result<Option<CommandTurn>, String> {
        let mode = serde_json::json!({"type": "skill", "skill_id": "plug:greet", "args": args});
        resolve_command_turn(
            &mode,
            Some(self.work.clone()),
            None,
            &self.manager,
            Arc::clone(&self.consent),
        )
        .await
    }

    async fn block(&self, args: &str) -> String {
        self.turn(args)
            .await
            .expect("renders")
            .expect("a command turn")
            .block
    }

    /// A `/plug:greet World` request whose slash mode is the real envelope —
    /// the catalog's entry for the command, the command parser, and the
    /// serializer both slash faces stamp with — run in `workspace`.
    async fn request(&self, workspace: PathBuf) -> RunRequest {
        let catalog = crate::sync_primitives::Arc::new(crate::tool_metadata::ToolCatalog::new());
        let rejected = catalog
            .register_skills(&[crate::skill::SkillInfo {
                id: "plug:greet".into(),
                name: "greet".into(),
                description: "greets".into(),
                scope: crate::domain::skill::PromptScope::System,
                version: None,
                allowed_tools: None,
                argument_hint: None,
                plugin_id: Some(PLUGIN.into()),
            }])
            .await;
        assert!(rejected.is_empty(), "{rejected:?}");
        let parsed = crate::command::CommandParser::new(catalog)
            .parse_async("/plug:greet World")
            .await
            .expect("the command resolves");
        let mode = crate::gateway::inbound_router::serialize_parsed_command(&parsed)
            .expect("a command serializes");
        let session = crate::gateway::router::SessionKey::main("slash-command-body");
        let mut request = super::super::tests::gate_test_request(&session, "cmd-run");
        request.input = "/plug:greet World".into();
        request
            .metadata
            .insert(SLASH_COMMAND_MODE_KEY.to_string(), mode);
        request.workspace_override = Some(workspace);
        request
    }
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
        serde_json::json!({"type": "skill", "skill_id": "plug:helper", "args": ""}),
        serde_json::json!({"type": "skill", "skill_id": "not-registered", "args": ""}),
        serde_json::json!({"type": "direct_tool", "tool_id": "plug:greet", "args": ""}),
    ] {
        let turn = resolve_command_turn(&mode, None, None, &f.manager, Arc::clone(&f.consent))
            .await
            .unwrap();
        assert!(turn.is_none(), "{mode}");
    }
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
    let mode = serde_json::json!({"type": "skill", "skill_id": "orphan:greet", "args": "you"});
    let turn = resolve_command_turn(
        &mode,
        Some(f.work.clone()),
        None,
        &f.manager,
        Arc::clone(&f.consent),
    )
    .await
    .unwrap()
    .expect("a command turn");
    assert!(
        turn.block.contains("Hi you [[!`echo hi` not run"),
        "{}",
        turn.block
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

/// The execute.rs seam end to end below the fast path: the real slash
/// envelope in, the rendered block in the TRANSIENT slot the run loop reads,
/// the raw `/command` still the input — the text the run loop persists as
/// the user turn — and the command's model as this turn's override. The
/// rescue then strips both.
#[tokio::test]
#[cfg(unix)]
async fn stamp_renders_into_the_transient_slot_and_leaves_the_input_raw() {
    let f = Fixture::new("Say hello to $1 from !`pwd -P`.", Some("claude-sonnet-5")).await;
    f.approve("pwd -P");
    let temp = TempDir::new().unwrap();
    let agent = super::super::tests::gate_test_agent(&temp, "cmd-agent").await;
    let mut request = f.request(f.work.clone()).await;

    stamp_with(&mut request, &agent, &f.manager, Arc::clone(&f.consent))
        .await
        .expect("the command renders");

    let block = transient_block(&request.metadata).expect("a transient block");
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
    assert_eq!(carriers, [BODY_KEY], "the body rides one key only");
    assert_eq!(
        request.model_override,
        Some(ModelOverride::Raw {
            model: "claude-sonnet-5".into()
        })
    );

    let kept = strip(&mut request.metadata, request.model_override.clone());
    assert_eq!(kept, None, "the command's pin was for its own turn");
    assert_eq!(transient_block(&request.metadata), None);
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

    stamp_with(&mut request, &agent, &f.manager, Arc::clone(&f.consent))
        .await
        .unwrap();
    assert_eq!(request.model_override.as_ref(), Some(&user));
    assert!(transient_block(&request.metadata).is_some());
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
    stamp_with(&mut request, &agent, &f.manager, Arc::clone(&f.consent))
        .await
        .unwrap();
    let block = transient_block(&request.metadata).unwrap();
    assert!(
        block.contains("[[!`echo hi` not run: no working directory is known for this turn]]"),
        "{block}"
    );
}

/// A `model:` the provider would fail stops the turn BEFORE the body renders:
/// its approved inline command never runs, and nothing is stamped.
#[tokio::test]
#[cfg(unix)]
async fn a_retired_model_stops_the_turn_before_any_inline_command_runs() {
    let f = Fixture::new("!`touch RAN`", Some("deepseek-reasoner")).await;
    f.approve("touch RAN");
    let temp = TempDir::new().unwrap();
    let agent = super::super::tests::gate_test_agent(&temp, "cmd-agent").await;
    let mut request = f.request(f.work.clone()).await;

    let why = stamp_with(&mut request, &agent, &f.manager, Arc::clone(&f.consent))
        .await
        .expect_err("a retired model refuses the turn");
    assert!(why.starts_with("/plug:greet was not run:"), "{why}");
    assert!(!f.work.join("RAN").exists(), "an inline command ran first");
    assert_eq!(transient_block(&request.metadata), None);
    assert_eq!(request.model_override, None);
}

/// Nobody under `execution_engine/` spells the two metadata keys but this
/// module — every reader goes through `transient_block` / `strip`, so a
/// second reader cannot re-deliver the body (a continuation) or keep the
/// command's model past its turn. Literals are kept and comments dropped.
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
        for key in [BODY_KEY, MODEL_PIN_KEY] {
            if code.contains(&format!("\"{key}\"")) {
                offenders.push(format!("{rel}: {key}"));
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:?}");
}

/// The two fire sites, by position:
/// - `execute.rs` renders exactly once, inside the fast path's
///   `Fallthrough` arm — so after face ④ judged the owner visible, and never
///   for a command the fast path refused or served;
/// - `run_loop/inner.rs` reads the block exactly once, before the first
///   `transient_blocks.push` — the command is the turn's instruction, every
///   reminder annotates it.
///
/// Comment lines are stripped first, so the prose that names both anchors
/// can neither satisfy nor defeat a `find`.
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

    let execute = strip_comment_lines(&production_prefix(include_str!("../execute.rs")));
    let fast_path = once(&execute, ".execute_slash_command_fast_path(");
    let fallthrough = once(
        &execute,
        "Err(ExecutionError::Fallthrough { ref reason }) => {",
    );
    let render = once(&execute, "slash_command_body::stamp(");
    let refused = execute
        .get(fallthrough..)
        .and_then(|rest| rest.find("Err(ref e) => {"))
        .map(|at| at + fallthrough)
        .expect("the fast path's refusal arm follows the fallthrough arm");
    assert!(
        fast_path < fallthrough && fallthrough < render && render < refused,
        "execute.rs must render the command inside the Fallthrough arm, after the fast path \
         (face ④) — positions: fast path {fast_path}, fallthrough {fallthrough}, render \
         {render}, refusal arm {refused}"
    );

    let inner = strip_comment_lines(&production_prefix(include_str!("../run_loop/inner.rs")));
    let declared = once(&inner, "let mut transient_blocks");
    let read = once(&inner, "slash_command_body::transient_block(");
    let first_push = inner
        .get(declared..)
        .and_then(|rest| rest.find("transient_blocks.push("))
        .map(|at| at + declared)
        .expect("the loop pushes its reminders");
    assert!(
        declared < read && read < first_push,
        "inner.rs must push the command block before every reminder — positions: declared \
         {declared}, read {read}, first push {first_push}"
    );
}

/// Two more fire sites no behavioural test here can reach (the rescue needs
/// an orchestrator):
/// - the steering rescue strips the command's residue from the metadata it
///   clones, before it builds the continuation — or the body is delivered,
///   and the command's model pinned, a second time;
/// - "where does this run work" has ONE derivation: the helper holds the
///   only `agent.workspace()` fallback, and its three readers (the run's
///   task-locals, `effective_workspace`, this module's render) call it.
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
        3,
        "a reader stopped calling `run_workspace`"
    );
}
