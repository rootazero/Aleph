//! `workflow` tool — manage and run declarative workflow templates (R8).
//!
//! The LLM-facing surface for the [`crate::workflow`] layer: save / list /
//! describe / delete reusable templates, and `run` one against a team. `run`
//! compiles the template into the existing `coord_tasks` DAG
//! ([`crate::workflow::materialize`]) and signals the dispatcher — execution
//! then proceeds on the existing autonomous loop. This tool performs **no
//! orchestration of its own** (R10).
//!
//! Single tool with an `action` discriminator, mirroring `workflow_step_review`
//! and `team_snapshot`.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::agents::swarm::tasks::CoordTaskStore;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::turn_context::current_turn_context;
use crate::tools::AlephTool;
use crate::workflow::{self, ClarifyContext, WorkflowDef, WorkflowManifest};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum WorkflowArgs {
    /// Save (create or overwrite) a reusable workflow template to disk.
    Save { definition: WorkflowDef },
    /// List the names of all saved workflow templates.
    List {},
    /// Show the full definition of a saved workflow template.
    Describe { name: String },
    /// Delete a saved workflow template. Idempotent.
    Delete { name: String },
    /// Run a saved workflow: compile its steps into coordination tasks owned
    /// by the named team's members and start execution. Create the team first
    /// (with `team_create`) so every step's `agent` resolves to a member.
    Run {
        /// Name of the saved template to run.
        name: String,
        /// Team that hosts the run; its members own the materialised steps.
        team_id: String,
        /// Run input substituted for `{input}` in each step's prompt.
        #[serde(default)]
        input: String,
    },
    /// Render a saved template into a Claude-Code-compatible `.workflow.js`.
    Export {
        /// Name of the saved template to render.
        name: String,
        /// Also write it to `$ALEPH_HOME/workflows/<name>.workflow.js`.
        #[serde(default)]
        write_file: bool,
    },
    /// Parse a `.workflow.js` (or AWI manifest JSON) into a WorkflowDef.
    Import {
        /// Raw `.workflow.js` text or AWI manifest JSON.
        source: String,
        /// Also persist the parsed template via the store.
        #[serde(default)]
        save: bool,
    },
    /// List the gated MetaSkill proposals the dream pipeline auto-drafted from
    /// recurring skill co-occurrence. These are NOT active until accepted.
    Proposals {},
    /// Accept (activate) a gated MetaSkill proposal: promote it from the
    /// `proposals/` draft dir into the active workflow store, then run it with
    /// `action='run'`. The draft is removed once accepted.
    AcceptProposal {
        /// Name of the pending proposal (see `action='proposals'`).
        name: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowToolOutput {
    pub action: String,
    pub message: String,
    /// Populated by `list`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub names: Option<Vec<String>>,
    /// Populated by `describe`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<WorkflowDef>,
    /// Populated by `run` — the created coordination-task ids.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_ids: Option<Vec<String>>,
    /// Populated by `export` — the rendered `.workflow.js` text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered: Option<String>,
    /// Populated by `import` — imperative constructs that could not be mapped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped: Option<Vec<String>>,
}

impl WorkflowToolOutput {
    fn msg(action: &str, message: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            message: message.into(),
            names: None,
            definition: None,
            task_ids: None,
            rendered: None,
            dropped: None,
        }
    }
}

#[derive(Clone)]
pub struct WorkflowTool {
    coord_store: Arc<dyn CoordTaskStore>,
    /// Wakes the team dispatcher after `run` so materialised tasks start
    /// without waiting for the fallback tick. `None` → tasks still run on the
    /// dispatcher's periodic tick, just with added latency.
    dispatch_signal: Option<Arc<tokio::sync::Notify>>,
}

impl WorkflowTool {
    pub fn new(
        coord_store: Arc<dyn CoordTaskStore>,
        dispatch_signal: Option<Arc<tokio::sync::Notify>>,
    ) -> Self {
        Self {
            coord_store,
            dispatch_signal,
        }
    }
}

#[async_trait]
impl AlephTool for WorkflowTool {
    const NAME: &'static str = "workflow";
    const DESCRIPTION: &'static str =
        "Manage and run reusable workflow templates. A template is a named, \
         declarative multi-step pipeline (each step = one agent + a prompt + \
         dependencies); running it compiles the steps into a coordination-task \
         DAG that executes concurrently where dependencies allow. \
         Actions: save / list / describe / delete / run / export / import / \
         proposals / accept_proposal. \
         `export` renders a template to a Claude-Code-compatible .workflow.js; \
         `import` parses one back into a template. `proposals` lists MetaSkill \
         drafts the dream pipeline auto-grew from recurring skill use; \
         `accept_proposal` activates one. For `run`, create a team first so \
         each step's agent resolves to a member.";

    type Args = WorkflowArgs;
    type Output = WorkflowToolOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"workflow(action='save', definition={"name":"research-report","description":"research then write","steps":[{"id":"gather","agent":"researcher","prompt":"research {input}"},{"id":"write","agent":"writer","prompt":"write a report","depends_on":["gather"]}]})"#.into(),
            "workflow(action='list')".into(),
            "workflow(action='describe', name='research-report')".into(),
            "workflow(action='run', name='research-report', team_id='team-42', input='quantum error correction')".into(),
            "workflow(action='delete', name='research-report')".into(),
            "workflow(action='export', name='research-report')".into(),
            r#"workflow(action='import', source='export const meta = { name: \"x\" }\nawait agent(\"do it\")', save=true)"#.into(),
            "workflow(action='proposals')".into(),
            "workflow(action='accept_proposal', name='metaskill-git-pr')".into(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args {
            WorkflowArgs::Save { definition } => {
                debug!(name = %definition.name, "workflow: save");
                // `save` authors the lean executable core; persist it as a
                // manifest (extras empty) so the on-disk format stays uniform.
                let manifest = WorkflowManifest::from_def(&definition);
                let path = workflow::store::save(&manifest)?;
                Ok(WorkflowToolOutput::msg(
                    "save",
                    format!("saved workflow '{}' → {}", definition.name, path.display()),
                ))
            }
            WorkflowArgs::List {} => {
                let names: Vec<String> = workflow::store::list()?
                    .into_iter()
                    .map(|m| m.name)
                    .collect();
                Ok(WorkflowToolOutput {
                    action: "list".into(),
                    message: format!("{} workflow(s)", names.len()),
                    names: Some(names),
                    definition: None,
                    task_ids: None,
                    rendered: None,
                    dropped: None,
                })
            }
            WorkflowArgs::Describe { name } => {
                let manifest = workflow::store::load(&name)?;
                // Output the executable projection — the tool's `definition`
                // field is a `WorkflowDef`; the extra metadata is reachable via
                // `export`.
                Ok(WorkflowToolOutput {
                    action: "describe".into(),
                    message: format!("workflow '{name}' has {} step(s)", manifest.steps.len()),
                    names: None,
                    definition: Some(manifest.to_def()),
                    task_ids: None,
                    rendered: None,
                    dropped: None,
                })
            }
            WorkflowArgs::Delete { name } => {
                let removed = workflow::store::delete(&name)?;
                let message = if removed {
                    format!("deleted workflow '{name}'")
                } else {
                    format!("workflow '{name}' did not exist")
                };
                Ok(WorkflowToolOutput::msg("delete", message))
            }
            WorkflowArgs::Run {
                name,
                team_id,
                input,
            } => {
                debug!(name = %name, team_id = %team_id, "workflow: run");
                // Project to the executable core — `materialize` consumes only
                // id/agent/prompt/depends_on (R10: the executor never sees the
                // interchange metadata).
                let def = workflow::store::load(&name)?.to_def();
                // Capture the originating channel so any `clarify` step can reach
                // the user from the autonomous dispatcher, where the launching
                // turn no longer exists. A non-interactive run yields `None`;
                // clarify steps then fail fast at delivery (clear reason) rather
                // than stalling the DAG.
                let clarify_ctx = current_turn_context()
                    .filter(|t| t.is_channel_routable())
                    .map(|t| ClarifyContext {
                        channel_id: t.channel_id.clone(),
                        conversation_id: t.conversation_id.clone(),
                        session_key: t.session_key.to_string(),
                    });
                let mat = workflow::materialize(
                    &def,
                    &input,
                    &team_id,
                    self.coord_store.as_ref(),
                    clarify_ctx.as_ref(),
                )
                .await?;
                if let Some(signal) = &self.dispatch_signal {
                    signal.notify_one();
                }
                Ok(WorkflowToolOutput {
                    action: "run".into(),
                    message: format!(
                        "started workflow '{name}' on team '{team_id}': {} task(s) queued",
                        mat.task_ids.len()
                    ),
                    names: None,
                    definition: None,
                    task_ids: Some(mat.task_ids),
                    rendered: None,
                    dropped: None,
                })
            }
            WorkflowArgs::Export { name, write_file } => {
                debug!(name = %name, write_file, "workflow: export");
                // The stored manifest carries the full `.workflow.js` metadata,
                // so the render is now faithful (phases, per-step
                // label/model/phase/schema) rather than a bare skeleton.
                let manifest = workflow::store::load(&name)?;
                let rendered = workflow::render_workflow_js(&manifest);
                let message = if write_file {
                    let path = workflow::store::write_text(&name, "workflow.js", &rendered)?;
                    format!("exported workflow '{name}' → {}", path.display())
                } else {
                    format!("rendered workflow '{name}' ({} bytes)", rendered.len())
                };
                Ok(WorkflowToolOutput {
                    action: "export".into(),
                    message,
                    names: None,
                    definition: None,
                    task_ids: None,
                    rendered: Some(rendered),
                    dropped: None,
                })
            }
            WorkflowArgs::Import { source, save } => {
                debug!(save, "workflow: import");
                let outcome = workflow::parse_workflow_js(&source)?;
                let def = outcome.manifest.to_def();
                // On validation failure, fold the best-effort scan's `dropped`
                // diagnostics into the error so the user keeps the context that
                // the import was lossy (imperative constructs were skipped) —
                // otherwise `?` would discard `outcome.dropped` silently.
                if let Err(e) = outcome.manifest.validate() {
                    if outcome.dropped.is_empty() {
                        return Err(e);
                    }
                    return Err(AlephError::invalid_input(format!(
                        "{e}; note: import dropped {} imperative construct(s): {}",
                        outcome.dropped.len(),
                        outcome.dropped.join("; ")
                    )));
                }
                let message = if save {
                    // Persist the full manifest so an `import` of a rich
                    // `.workflow.js` keeps its phases/schema/model on disk.
                    let path = workflow::store::save(&outcome.manifest)?;
                    format!(
                        "imported workflow '{}' ({} step(s)) → {}",
                        def.name,
                        def.steps.len(),
                        path.display()
                    )
                } else {
                    format!(
                        "parsed workflow '{}' ({} step(s); not saved)",
                        def.name,
                        def.steps.len()
                    )
                };
                Ok(WorkflowToolOutput {
                    action: "import".into(),
                    message,
                    names: None,
                    definition: Some(def),
                    task_ids: None,
                    rendered: None,
                    dropped: Some(outcome.dropped),
                })
            }
            WorkflowArgs::Proposals {} => {
                let names: Vec<String> = workflow::proposal::list_proposals()?
                    .into_iter()
                    .map(|m| m.name)
                    .collect();
                Ok(WorkflowToolOutput {
                    action: "proposals".into(),
                    message: format!(
                        "{} gated MetaSkill proposal(s); describe one with action='describe' is \
                         for active workflows — accept with action='accept_proposal'",
                        names.len()
                    ),
                    names: Some(names),
                    definition: None,
                    task_ids: None,
                    rendered: None,
                    dropped: None,
                })
            }
            WorkflowArgs::AcceptProposal { name } => {
                debug!(name = %name, "workflow: accept_proposal");
                let path = workflow::proposal::accept(&name)?;
                Ok(WorkflowToolOutput::msg(
                    "accept_proposal",
                    format!(
                        "accepted MetaSkill '{name}' → active at {} (run with action='run')",
                        path.display()
                    ),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::tasks::{store::SqliteCoordTaskStore, CoordTaskStatus};
    use crate::workflow::def::WorkflowStepDef;
    use rusqlite::Connection;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // `ALEPH_HOME` is process-global; the file-backed actions (save/list/
    // describe/delete/run-load) resolve their directory from it via
    // `workflow::store::*`. Serialise every test that touches it through this
    // guard so parallel `cargo test` threads can't read/write each other's
    // workflows dir. Pure serde/notify tests below need no env and skip it.
    static ENV_GUARD: Mutex<()> = Mutex::new(());

    async fn setup_store() -> SqliteCoordTaskStore {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let store = SqliteCoordTaskStore::new(conn);
        store.migrate().await.expect("migrate");
        store
    }

    fn linear_def() -> WorkflowDef {
        WorkflowDef {
            name: "pipeline".into(),
            description: "research then write".into(),
            steps: vec![
                WorkflowStepDef {
                    id: "gather".into(),
                    agent: "researcher".into(),
                    prompt: "research {input}".into(),
                    depends_on: vec![],
                    kind: crate::workflow::WorkflowStepKind::Agent,
                    choices: vec![],
                },
                WorkflowStepDef {
                    id: "write".into(),
                    agent: "writer".into(),
                    prompt: "write a report".into(),
                    depends_on: vec!["gather".into()],
                    kind: crate::workflow::WorkflowStepKind::Agent,
                    choices: vec![],
                },
            ],
        }
    }

    fn tool(store: SqliteCoordTaskStore, signal: Option<Arc<tokio::sync::Notify>>) -> WorkflowTool {
        WorkflowTool::new(Arc::new(store), signal)
    }

    // --- serde discriminator: the exact shape the agent loop deserialises ---

    #[test]
    fn deserialize_run_defaults_input() {
        // `input` omitted relies on #[serde(default)] → empty string.
        let args: WorkflowArgs =
            serde_json::from_value(serde_json::json!({"action":"run","name":"p","team_id":"t"}))
                .expect("deserialise run without input");
        match args {
            WorkflowArgs::Run {
                name,
                team_id,
                input,
            } => {
                assert_eq!(name, "p");
                assert_eq!(team_id, "t");
                assert_eq!(input, "", "missing input defaults to empty string");
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_save_nested_definition() {
        let args: WorkflowArgs = serde_json::from_value(serde_json::json!({
            "action": "save",
            "definition": {
                "name": "research-report",
                "steps": [
                    {"id": "gather", "agent": "researcher", "prompt": "research {input}"},
                    {"id": "write", "agent": "writer", "prompt": "write", "depends_on": ["gather"]}
                ]
            }
        }))
        .expect("deserialise save with nested definition");
        match args {
            WorkflowArgs::Save { definition } => {
                assert_eq!(definition.name, "research-report");
                assert_eq!(definition.steps.len(), 2);
                assert_eq!(definition.steps[1].depends_on, vec!["gather".to_string()]);
            }
            other => panic!("expected Save, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_list_unit_variant() {
        let args: WorkflowArgs =
            serde_json::from_value(serde_json::json!({"action":"list"})).expect("deserialise list");
        assert!(matches!(args, WorkflowArgs::List {}));
    }

    #[test]
    fn deserialize_rejects_unknown_action() {
        let err =
            serde_json::from_value::<WorkflowArgs>(serde_json::json!({"action":"frobnicate"}));
        assert!(err.is_err(), "unknown action must not deserialise");
    }

    // --- output shaping: which Option fields each action populates ---

    #[test]
    fn output_msg_helper_leaves_optionals_none() {
        let out = WorkflowToolOutput::msg("save", "ok");
        assert_eq!(out.action, "save");
        assert!(out.names.is_none());
        assert!(out.definition.is_none());
        assert!(out.task_ids.is_none());
    }

    // --- run action (fully injectable: no real team/dispatcher needed) ---

    #[tokio::test]
    async fn run_materializes_tasks_and_returns_ids() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        // `save` then `run` both resolve their dir from ALEPH_HOME; hold the
        // guard across both so the env stays hermetic for the whole sequence.
        let run_out = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored after the await completes.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }
            workflow::store::save(&WorkflowManifest::from_def(&linear_def()))
                .expect("save under temp ALEPH_HOME");
            let r = t
                .call(WorkflowArgs::Run {
                    name: "pipeline".into(),
                    team_id: "team-7".into(),
                    input: "quantum".into(),
                })
                .await;
            // SAFETY: same guarded invariant; restore prior value.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var("ALEPH_HOME", v),
                    None => std::env::remove_var("ALEPH_HOME"),
                }
            }
            r
        }
        .expect("run materialises");

        assert_eq!(run_out.action, "run");
        let ids = run_out.task_ids.as_ref().expect("run populates task_ids");
        assert_eq!(ids.len(), 2, "one task per step");
        // run shapes only task_ids — never names/definition.
        assert!(run_out.names.is_none());
        assert!(run_out.definition.is_none());

        // The returned ids correspond to actually-created, correctly-wired
        // coord_tasks: gather is Pending (no deps), write is Blocked on it.
        let cstore = t.coord_store.clone();
        let gather = cstore.get_task(&ids[0]).await.unwrap().unwrap();
        let write = cstore.get_task(&ids[1]).await.unwrap().unwrap();
        assert_eq!(gather.subject, "pipeline:gather");
        assert_eq!(
            gather.description, "research quantum",
            "{{input}} substituted"
        );
        assert_eq!(gather.status, CoordTaskStatus::Pending);
        assert_eq!(write.subject, "pipeline:write");
        assert_eq!(write.status, CoordTaskStatus::Blocked);
    }

    #[tokio::test]
    async fn run_notifies_dispatcher_when_signal_present() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let signal = Arc::new(tokio::sync::Notify::new());
        let t = tool(store, Some(signal.clone()));

        // Register a waiter BEFORE run so notify_one delivers a persistent
        // permit even if it fires before we await.
        let notified = signal.notified();

        let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("ALEPH_HOME");
        // SAFETY: guarded single mutator; restored below.
        unsafe {
            std::env::set_var("ALEPH_HOME", tmp.path());
        }
        workflow::store::save(&WorkflowManifest::from_def(&linear_def())).expect("save");
        let run = t
            .call(WorkflowArgs::Run {
                name: "pipeline".into(),
                team_id: "team-7".into(),
                input: "x".into(),
            })
            .await;
        // SAFETY: same guarded invariant; restore prior value.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("ALEPH_HOME", v),
                None => std::env::remove_var("ALEPH_HOME"),
            }
        }
        run.expect("run");

        // The waiter must resolve promptly; a generous timeout keeps the test
        // from hanging if the notify is ever dropped.
        tokio::time::timeout(std::time::Duration::from_secs(2), notified)
            .await
            .expect("dispatcher was signalled");
    }

    #[tokio::test]
    async fn run_without_signal_still_returns_ids() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("ALEPH_HOME");
        // SAFETY: guarded single mutator; restored below.
        unsafe {
            std::env::set_var("ALEPH_HOME", tmp.path());
        }
        workflow::store::save(&WorkflowManifest::from_def(&linear_def())).expect("save");
        let out = t
            .call(WorkflowArgs::Run {
                name: "pipeline".into(),
                team_id: "team-7".into(),
                input: String::new(),
            })
            .await;
        // SAFETY: same guarded invariant; restore prior value.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("ALEPH_HOME", v),
                None => std::env::remove_var("ALEPH_HOME"),
            }
        }
        let out = out.expect("run without signal must not panic");
        assert_eq!(out.task_ids.as_ref().map(|v| v.len()), Some(2));
    }

    #[tokio::test]
    async fn run_errors_on_missing_template() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("ALEPH_HOME");
        // SAFETY: guarded single mutator; restored below.
        unsafe {
            std::env::set_var("ALEPH_HOME", tmp.path());
        }
        let res = t
            .call(WorkflowArgs::Run {
                name: "does-not-exist".into(),
                team_id: "team-7".into(),
                input: String::new(),
            })
            .await;
        // SAFETY: same guarded invariant; restore prior value.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("ALEPH_HOME", v),
                None => std::env::remove_var("ALEPH_HOME"),
            }
        }
        assert!(res.is_err(), "loading a missing template surfaces an error");
    }

    // --- export / import actions ---

    #[test]
    fn deserialize_export_defaults_write_file_false() {
        let args: WorkflowArgs =
            serde_json::from_value(serde_json::json!({"action":"export","name":"p"}))
                .expect("deserialise export");
        match args {
            WorkflowArgs::Export { name, write_file } => {
                assert_eq!(name, "p");
                assert!(!write_file, "write_file defaults to false");
            }
            other => panic!("expected Export, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_import_defaults_save_false() {
        let args: WorkflowArgs =
            serde_json::from_value(serde_json::json!({"action":"import","source":"x"}))
                .expect("deserialise import");
        match args {
            WorkflowArgs::Import { source, save } => {
                assert_eq!(source, "x");
                assert!(!save, "save defaults to false");
            }
            other => panic!("expected Import, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn export_renders_without_writing_then_import_roundtrips() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        // Capture both call results under the env guard, restore ALEPH_HOME,
        // then assert — so a failing assertion can't panic before restore and
        // leak a dead-TempDir env into the next guarded test.
        let (exported, imported) = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored below.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }

            workflow::store::save(&WorkflowManifest::from_def(&linear_def()))
                .expect("save template");

            // export (no write_file) populates `rendered`, not task_ids/definition.
            let exported = t
                .call(WorkflowArgs::Export {
                    name: "pipeline".into(),
                    write_file: false,
                })
                .await
                .expect("export");
            // import the rendered text back (no save) → definition equals the
            // core, dropped is empty for the lossless embedded path.
            let js = exported
                .rendered
                .clone()
                .expect("export populates rendered");
            let imported = t
                .call(WorkflowArgs::Import {
                    source: js,
                    save: false,
                })
                .await
                .expect("import");

            // SAFETY: same guarded invariant; restore prior value.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var("ALEPH_HOME", v),
                    None => std::env::remove_var("ALEPH_HOME"),
                }
            }
            (exported, imported)
        };

        assert_eq!(exported.action, "export");
        let js = exported
            .rendered
            .as_ref()
            .expect("export populates rendered");
        assert!(js.contains("export const meta = {"));
        assert!(exported.task_ids.is_none() && exported.definition.is_none());

        assert_eq!(imported.action, "import");
        let def = imported
            .definition
            .as_ref()
            .expect("import populates definition");
        assert_eq!(def, &linear_def());
        assert_eq!(imported.dropped.as_deref(), Some(&[][..]));
    }

    #[tokio::test]
    async fn import_rich_manifest_then_export_reproduces_metadata() {
        // The headline fidelity guarantee: importing a rich AWI manifest
        // (per-step schema/model/phase + meta phases) with save=true, then
        // exporting it, reproduces that metadata — because the store now
        // persists the manifest superset, not just the executable core. (Bare
        // hand-written `.workflow.js` opts are out of scope for the scanner;
        // the lossless rich channels are manifest JSON and the embed block.)
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let rich_manifest_json = r#"{
  "name": "audit",
  "description": "two-phase audit",
  "whenToUse": "on any subsystem",
  "phases": [
    { "title": "Scan", "detail": "look", "model": "opus" },
    { "title": "Fix", "detail": "patch" }
  ],
  "steps": [
    { "id": "a", "agent": "scanner", "prompt": "scan {input}", "label": "scan:a", "phase": "Scan", "model": "haiku", "schema": {"type":"object"}, "isolation": "worktree", "agentType": "Explore" },
    { "id": "b", "agent": "fixer", "prompt": "fix it", "dependsOn": ["a"], "label": "fix:b", "phase": "Fix" }
  ]
}"#;

        let exported = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored below.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }
            // Import the rich AWI manifest JSON (lossless) and persist it.
            t.call(WorkflowArgs::Import {
                source: rich_manifest_json.into(),
                save: true,
            })
            .await
            .expect("import rich manifest + save");
            // Re-export from disk — must reproduce phases/schema/model/label.
            let exported = t
                .call(WorkflowArgs::Export {
                    name: "audit".into(),
                    write_file: false,
                })
                .await
                .expect("export");
            // SAFETY: same guarded invariant; restore prior value.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var("ALEPH_HOME", v),
                    None => std::env::remove_var("ALEPH_HOME"),
                }
            }
            exported
        };

        let js = exported
            .rendered
            .as_ref()
            .expect("export populates rendered");
        // meta block carries whenToUse + both phases.
        assert!(
            js.contains("whenToUse: \"on any subsystem\""),
            "whenToUse: {js}"
        );
        assert!(
            js.contains("title: \"Scan\"") && js.contains("title: \"Fix\""),
            "phases: {js}"
        );
        // per-step metadata survived: schema, model, label, phase markers, plus
        // the engineering-format agent-opts isolation + agentType.
        assert!(js.contains("schema: {\"type\":\"object\"}"), "schema: {js}");
        assert!(js.contains("model: \"haiku\""), "model: {js}");
        assert!(js.contains("label: \"scan:a\""), "label: {js}");
        assert!(js.contains("isolation: \"worktree\""), "isolation: {js}");
        assert!(js.contains("agentType: \"Explore\""), "agentType: {js}");
        // the Scan phase carries its per-phase model override in the meta block.
        assert!(
            js.contains("title: \"Scan\", detail: \"look\", model: \"opus\""),
            "phase model: {js}"
        );
        assert!(
            js.contains("phase(\"Scan\")") && js.contains("phase(\"Fix\")"),
            "phase markers: {js}"
        );
    }

    #[tokio::test]
    async fn import_with_save_persists_template() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        // Capture both call results under the env guard, restore ALEPH_HOME,
        // then assert (see sibling roundtrip test for rationale).
        let (imported, listed) = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored below.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }

            let source = "export const meta = { name: 'scanned' }\nawait agent('do the thing')";
            let imported = t
                .call(WorkflowArgs::Import {
                    source: source.into(),
                    save: true,
                })
                .await
                .expect("import + save");
            let listed = t.call(WorkflowArgs::List {}).await.expect("list");

            // SAFETY: same guarded invariant; restore prior value.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var("ALEPH_HOME", v),
                    None => std::env::remove_var("ALEPH_HOME"),
                }
            }
            (imported, listed)
        };

        assert!(imported.message.contains("imported"));
        assert_eq!(listed.names.as_deref(), Some(&["scanned".to_string()][..]));
    }

    #[tokio::test]
    async fn import_validate_failure_preserves_dropped_diagnostics() {
        // A bare scan can yield a structurally-invalid def (here: a whitespace
        // meta.name) while ALSO dropping imperative constructs. The error must
        // carry BOTH — the validation cause and the dropped note — so the lossy
        // import context isn't silently discarded by `?`. Pure parse/validate,
        // no store or ALEPH_HOME touched (save=false).
        let store = setup_store().await;
        let t = tool(store, None);
        let source = "export const meta = { name: '  ' }\n\
                      for (const x of items) { await agent('do thing') }";
        let err = t
            .call(WorkflowArgs::Import {
                source: source.into(),
                save: false,
            })
            .await
            .expect_err("whitespace name must fail validation");
        let msg = err.to_string();
        assert!(
            msg.contains("name must not be empty"),
            "validation cause: {msg}"
        );
        assert!(
            msg.contains("dropped"),
            "dropped diagnostics preserved: {msg}"
        );
        assert!(
            msg.contains("for loop"),
            "specific dropped construct named: {msg}"
        );
    }

    #[tokio::test]
    async fn export_missing_template_errors() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("ALEPH_HOME");
        // SAFETY: guarded single mutator; restored below.
        unsafe {
            std::env::set_var("ALEPH_HOME", tmp.path());
        }
        let res = t
            .call(WorkflowArgs::Export {
                name: "ghost".into(),
                write_file: false,
            })
            .await;
        // SAFETY: same guarded invariant; restore prior value.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("ALEPH_HOME", v),
                None => std::env::remove_var("ALEPH_HOME"),
            }
        }
        assert!(res.is_err(), "exporting a missing template errors");
    }

    // --- file-backed lifecycle: save → list → describe → delete ---
    //
    // One combined #[tokio::test] keeps every ALEPH_HOME-touching assertion in
    // a single env scope, so there is no cross-test race on the process-global
    // var and the round-trip ordering is deterministic.
    #[tokio::test]
    async fn file_actions_lifecycle_and_output_shapes() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("ALEPH_HOME");
        // SAFETY: guarded single mutator; restored at end of the test.
        unsafe {
            std::env::set_var("ALEPH_HOME", tmp.path());
        }

        // describe of an absent template errors.
        let missing = t
            .call(WorkflowArgs::Describe {
                name: "ghost".into(),
            })
            .await;
        assert!(missing.is_err(), "describe of missing template errors");

        // empty list before any save.
        let empty = t.call(WorkflowArgs::List {}).await.expect("list");
        assert_eq!(empty.action, "list");
        assert_eq!(empty.names.as_deref(), Some(&[][..]));
        assert!(empty.definition.is_none() && empty.task_ids.is_none());

        // save → only the message is shaped (no optionals).
        let saved = t
            .call(WorkflowArgs::Save {
                definition: linear_def(),
            })
            .await
            .expect("save");
        assert_eq!(saved.action, "save");
        assert!(saved.message.contains("pipeline"));
        assert!(saved.names.is_none() && saved.definition.is_none() && saved.task_ids.is_none());

        // list reflects the saved template — only names populated.
        let listed = t.call(WorkflowArgs::List {}).await.expect("list");
        assert_eq!(listed.names.as_deref(), Some(&["pipeline".to_string()][..]));
        assert!(listed.definition.is_none() && listed.task_ids.is_none());

        // describe round-trips the definition — only definition populated.
        let described = t
            .call(WorkflowArgs::Describe {
                name: "pipeline".into(),
            })
            .await
            .expect("describe");
        assert_eq!(described.action, "describe");
        let def = described
            .definition
            .as_ref()
            .expect("describe populates definition");
        assert_eq!(def, &linear_def());
        assert!(described.message.contains("2 step"));
        assert!(described.names.is_none() && described.task_ids.is_none());

        // serde wire shape: describe omits the None fields entirely.
        let wire = serde_json::to_value(&described).unwrap();
        assert!(wire.get("definition").is_some());
        assert!(
            wire.get("names").is_none(),
            "skip_serializing_if drops None names"
        );
        assert!(wire.get("task_ids").is_none());

        // delete present → "deleted" message; delete again → idempotent branch.
        let del1 = t
            .call(WorkflowArgs::Delete {
                name: "pipeline".into(),
            })
            .await
            .expect("delete present");
        assert!(del1.message.contains("deleted"));
        let del2 = t
            .call(WorkflowArgs::Delete {
                name: "pipeline".into(),
            })
            .await
            .expect("delete absent");
        assert!(
            del2.message.contains("did not exist"),
            "idempotent delete branch"
        );

        // after delete the list is empty again.
        let after = t.call(WorkflowArgs::List {}).await.expect("list");
        assert_eq!(after.names.as_deref(), Some(&[][..]));

        // SAFETY: guarded single mutator; restore prior value.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("ALEPH_HOME", v),
                None => std::env::remove_var("ALEPH_HOME"),
            }
        }
    }
}
