//! A skill's inline `` !`cmd` `` through the chain production runs (判据 §4):
//! this chokepoint publishes whether the call may run one
//! (`turn_context::TURN_INLINE_SHELL`), `skill_read` — the production
//! `AlephTool` — reads the skill, and `skill::preprocess` runs its commands
//! against a real consent registry, spawning real `sh`.

use super::*;
use crate::builtin_tools::skill_reader::ReadSkillTool;
use crate::config::types::policies::ToolPermissionsConfig;
use crate::extension::hooks::{ConsentEntry, ConsentStatus, ShellHookConsent, SKILL_INLINE_EVENT};
use crate::extension::visibility::ScopeKey;
use crate::extension::PermissionAction;
use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult as LoopToolResult};
use crate::tools::AlephTool;
use crate::utils::paths::{publish_plugin_skill_dirs, IsolatedAlephHome, PublishedPluginSkillDir};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// `skill_read` as the builtin registry dispatches it — the production tool,
/// through `call_json` — with the test's consent file.
struct SkillRead(ReadSkillTool);

#[async_trait::async_trait]
impl LoopTool for SkillRead {
    fn name(&self) -> &str {
        ReadSkillTool::NAME
    }
    fn description(&self) -> &str {
        "skill_read"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, input: Value, _cancel: CancellationToken) -> LoopToolResult {
        match self.0.call_json(input).await {
            Ok(output) => LoopToolResult::Success { output },
            Err(e) => LoopToolResult::Error {
                error: e.to_string(),
                retryable: false,
            },
        }
    }
}

/// Who calls, and what this turn's policy says about `bash`.
#[derive(Clone, Copy)]
enum Caller {
    Operator,
    Guest,
    OperatorWithBashDenied,
}

/// A skills directory holding one opted-in skill, a consent file, a run
/// directory, and `$ALEPH_HOME` isolated (`$HOME` is not read on this path,
/// and moving it would race the unguarded `~/…` tests beside these).
struct Fixture {
    tmp: tempfile::TempDir,
    consent: Arc<ShellHookConsent>,
    skills: PathBuf,
    skill: String,
    run_dir: PathBuf,
    /// Held last: the published plugin dir is process-wide, and every test
    /// that publishes one holds this lock.
    _env: IsolatedAlephHome,
}

impl Fixture {
    /// Skill `skill` under `skills` (relative to the tempdir), whose body
    /// holds `commands`, with a data file `x`.
    fn new(skills: &str, skill: &str, commands: &[&str]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let env = IsolatedAlephHome::new();
        let skills = tmp.path().join(skills);
        let skill_dir = skills.join(skill);
        std::fs::create_dir_all(&skill_dir).unwrap();
        let body = commands
            .iter()
            .map(|c| format!("- !`{c}`\n"))
            .collect::<String>();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: demo\ndescription: d\nallow-inline-shell: true\n---\n{body}"),
        )
        .unwrap();
        std::fs::write(skill_dir.join("x"), "file-content-42").unwrap();
        let run_dir = tmp.path().join("run");
        std::fs::create_dir_all(&run_dir).unwrap();
        let consent = Arc::new(ShellHookConsent::with_path(tmp.path().join("consent.json")));
        Self {
            consent,
            skills,
            skill: skill.to_string(),
            run_dir,
            tmp,
            _env: env,
        }
    }

    /// The same, with `skills` published as plugin `plug`'s `skills/`
    /// (install root: its parent).
    fn plugin(skill: &str, commands: &[&str]) -> Self {
        let f = Self::new("plug-root/skills", skill, commands);
        publish_plugin_skill_dirs(vec![PublishedPluginSkillDir {
            dir: f.skills.clone(),
            plugin_id: "plug".into(),
            scope_key: ScopeKey::Global,
            plugin_root: f.plugin_root(),
        }]);
        f
    }

    fn plugin_root(&self) -> PathBuf {
        self.tmp.path().join("plug-root")
    }

    fn skill_dir(&self) -> PathBuf {
        self.skills.join(&self.skill)
    }

    fn tool(&self) -> ReadSkillTool {
        ReadSkillTool::new(self.skills.clone()).with_consent(Arc::clone(&self.consent))
    }

    /// `skill_read` of the skill through this chokepoint, inside a run whose
    /// directory is `run_dir`: the body the model gets.
    async fn read(&self, caller: Caller) -> String {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(SkillRead(self.tool())));
        let role = match caller {
            Caller::Guest => Some("guest".to_string()),
            Caller::Operator | Caller::OperatorWithBashDenied => None,
        };
        let mut svc = ScopedToolService::new(Arc::new(registry), BTreeSet::new())
            .with_turn_context(crate::tools::turn_context::TurnContext {
                session_key: crate::routing::session_key::SessionKey::ephemeral("p417"),
                run_id: String::new(),
                channel_id: String::new(),
                conversation_id: String::new(),
                caller_role: role,
                channel_tool_permissions: None,
                unattended: false,
                plan_gate: None,
                side_question: false,
            });
        if matches!(caller, Caller::OperatorWithBashDenied) {
            svc = svc.with_tool_permissions(ToolPermissionsConfig {
                default: PermissionAction::Allow,
                overrides: std::collections::HashMap::from([(
                    "bash".to_string(),
                    PermissionAction::Deny,
                )]),
            });
        }
        let out = crate::sandbox::context::with_exec_workspace(
            Some(self.run_dir.clone()),
            svc.execute(ReadSkillTool::NAME, json!({ "skill_id": self.skill })),
        )
        .await
        .expect("skill_read answers");
        content_of(&out.value)
    }

    /// Approve every pending entry, as `aleph-server hooks test` does (the
    /// root it shows is the one approved).
    fn approve_all(&self) {
        for entry in self.consent.entries() {
            if entry.status == ConsentStatus::Pending {
                self.consent
                    .approve(&entry.fingerprint, entry.plugin_root.as_deref())
                    .unwrap()
                    .expect("approved");
            }
        }
    }

    /// Every file named `M` under the tempdir.
    fn created_m(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if entry.file_name() == "M" {
                    out.push(path);
                }
            }
        }
        let mut out = Vec::new();
        walk(self.tmp.path(), &mut out);
        out
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        publish_plugin_skill_dirs(Vec::new());
    }
}

/// The `content` field of a `skill_read` result, however the chokepoint
/// wrapped it.
fn content_of(value: &Value) -> String {
    let parsed = match value {
        Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| value.clone()),
        other => other.clone(),
    };
    parsed["content"]
        .as_str()
        .unwrap_or_else(|| panic!("no content in {parsed}"))
        .to_string()
}

const READ_X: &str = r#"cat "${ALEPH_SKILL_DIR}/x""#;

/// The pending placeholder the command face leaves, for `cmd`.
fn pending(cmd: &str) -> String {
    format!(
        "[!`{cmd}` not run: {}]",
        crate::extension::inline_shell::PENDING_APPROVAL
    )
}

/// The one consent entry on file.
fn only_entry(consent: &ShellHookConsent) -> ConsentEntry {
    let entries = consent.entries();
    assert_eq!(entries.len(), 1, "{entries:?}");
    entries.into_iter().next().unwrap()
}

/// The brief's shape: a PLUGIN skill whose directory is named with a
/// command substitution and a quote. Its inline command reads
/// `${ALEPH_SKILL_DIR}/x`: that directory reaches the shell as one word of
/// data, the command waits for consent — filed under the plugin, the skill
/// and the text as written — and once approved it shows the file, with the
/// plugin's root and the run's directory in its environment.
#[cfg(unix)]
#[tokio::test]
async fn a_plugin_skill_dir_is_data_to_its_inline_commands_and_they_wait_for_consent() {
    let f = Fixture::plugin(
        r#"s $(touch M)"x"#,
        &[
            READ_X,
            r#"printf '%s|%s' "$CLAUDE_PLUGIN_ROOT" "$CLAUDE_PROJECT_DIR""#,
            "pwd -P",
        ],
    );

    let before = f.read(Caller::Operator).await;
    assert!(before.contains(&pending(READ_X)), "{before}");
    assert!(!before.contains("file-content-42"), "{before}");
    let entries = f.consent.entries();
    assert_eq!(entries.len(), 3, "{entries:?}");
    let read_x = entries.iter().find(|e| e.command == READ_X).unwrap();
    assert_eq!(read_x.status, ConsentStatus::Pending);
    assert_eq!(read_x.event, SKILL_INLINE_EVENT);
    assert_eq!(
        read_x.plugin_name,
        ShellHookConsent::skill_owner("plug", &f.skill)
    );
    assert_eq!(read_x.plugin_root.as_deref(), Some(f.skill_dir().as_path()));
    assert_eq!(read_x.project_root, None, "a global plugin's skill");

    f.approve_all();
    let after = f.read(Caller::Operator).await;
    assert!(after.contains("- file-content-42\n"), "{after}");
    assert!(
        after.contains(&format!(
            "- {}|{}\n",
            f.plugin_root().display(),
            f.run_dir.display()
        )),
        "{after}"
    );
    let skill_dir = std::fs::canonicalize(f.skill_dir()).unwrap();
    assert!(
        after.contains(&format!("- {}\n", skill_dir.display())),
        "runs in the skill dir: {after}"
    );
    assert_eq!(f.created_m(), Vec::<PathBuf>::new());
}

/// A USER skill needs consent too, keyed by the skills directory it sits in.
/// Its directory is named with a bare command substitution — the shape a
/// spliced `${ALEPH_SKILL_DIR}` runs. Whatever was filed is approved, as an
/// operator reviewing it would, before anything is asserted: a splice would
/// then run `touch M`.
#[cfg(unix)]
#[tokio::test]
async fn a_user_skill_waits_for_consent_and_its_dir_is_data() {
    let f = Fixture::new("user-skills", "s $(touch M)", &[READ_X]);

    let before = f.read(Caller::Operator).await;
    f.approve_all();
    let after = f.read(Caller::Operator).await;

    assert_eq!(f.created_m(), Vec::<PathBuf>::new());
    assert!(before.contains(&pending(READ_X)), "{before}");
    assert!(after.contains("- file-content-42\n"), "{after}");
    let entry = only_entry(&f.consent);
    assert_eq!(entry.command, READ_X);
    assert_eq!(entry.event, SKILL_INLINE_EVENT);
    assert_eq!(
        entry.plugin_name,
        ShellHookConsent::skill_owner("user", "s $(touch M)")
    );
    assert_eq!(
        entry.project_root,
        Some(crate::extension::visibility::canonical_root(&f.skills))
    );
}

/// Consent is not enough: a guest's call, and a call whose policy denies
/// `bash`, run no approved command — each gets the placeholder naming why —
/// while the operator's call runs it (the approval is live).
#[cfg(unix)]
#[tokio::test]
async fn an_approved_command_runs_only_for_an_operator_whose_turn_allows_bash() {
    const TOUCH: &str = r#"touch "$ALEPH_SKILL_DIR/ran""#;
    let f = Fixture::new("user-skills", "demo", &[TOUCH]);
    let _ = f.read(Caller::Operator).await;
    f.approve_all();
    let ran = f.skill_dir().join("ran");

    let guest = f.read(Caller::Guest).await;
    assert!(
        guest.contains(&format!(
            "[!`{TOUCH}` not run: inline commands run only for an operator]"
        )),
        "{guest}"
    );
    let denied = f.read(Caller::OperatorWithBashDenied).await;
    assert!(
        denied.contains(&format!(
            "[!`{TOUCH}` not run: this turn's permissions deny `bash`]"
        )),
        "{denied}"
    );
    assert!(!ran.exists(), "a refused call ran the command");

    let _ = f.read(Caller::Operator).await;
    assert!(ran.exists(), "the operator's call did not run it");
}

/// A call no tool gate judged — `skill_read` called directly, as the
/// `tools.invoke` RPC does — runs nothing, approved or not.
#[cfg(unix)]
#[tokio::test]
async fn a_call_outside_the_chokepoint_runs_no_inline_command() {
    const TOUCH: &str = r#"touch "$ALEPH_SKILL_DIR/ran""#;
    let f = Fixture::new("user-skills", "demo", &[TOUCH]);
    let _ = f.read(Caller::Operator).await;
    f.approve_all();

    let direct = AlephTool::call(
        &f.tool(),
        crate::builtin_tools::skill_reader::ReadSkillArgs {
            skill_id: f.skill.clone(),
            file_name: None,
        },
    )
    .await
    .unwrap()
    .content;
    assert!(
        direct.contains(&format!(
            "[!`{TOUCH}` not run: no tool gate judged this call]"
        )),
        "{direct}"
    );
    assert!(!f.skill_dir().join("ran").exists());
}

/// The reason a skill's command naming `word` is refused.
fn unbindable(cmd: &str, word: &str) -> String {
    format!(
        "[!`{cmd}` not run: {}]",
        crate::skill::preprocess::unbindable_reason(word)
    )
}

/// Review I-1, the swap probe: a PLUGIN skill whose command names its script
/// through `${CLAUDE_PLUGIN_ROOT}` — or `$CLAUDE_PROJECT_DIR` — which consent
/// cannot resolve for a skill, is never filed or run. Before and after the
/// script is swapped the model gets the refusal naming the word, nothing is
/// pending to approve, and neither version of the script ran.
#[tokio::test]
async fn a_skill_script_named_through_another_variable_is_never_filed_or_run() {
    const VIA_ROOT: &str = r#"sh "${CLAUDE_PLUGIN_ROOT}/x.sh""#;
    const VIA_PROJECT: &str = r#"sh "$CLAUDE_PROJECT_DIR/x.sh""#;
    let f = Fixture::plugin("demo", &[VIA_ROOT, VIA_PROJECT]);
    for dir in [f.plugin_root(), f.run_dir.clone()] {
        std::fs::write(dir.join("x.sh"), "echo v1-ran\n").unwrap();
    }

    let first = f.read(Caller::Operator).await;
    f.approve_all();
    for dir in [f.plugin_root(), f.run_dir.clone()] {
        std::fs::write(dir.join("x.sh"), "echo v2-swapped\n").unwrap();
    }
    let second = f.read(Caller::Operator).await;

    assert!(
        !second.contains("v2-swapped"),
        "the swapped script ran: {second}"
    );
    for body in [&first, &second] {
        assert!(
            !body.contains("v1-ran") && !body.contains("v2-swapped"),
            "{body}"
        );
        assert!(
            body.contains(&unbindable(VIA_ROOT, "${CLAUDE_PLUGIN_ROOT}/x.sh")),
            "{body}"
        );
        assert!(
            body.contains(&unbindable(VIA_PROJECT, "$CLAUDE_PROJECT_DIR/x.sh")),
            "{body}"
        );
    }
    assert!(f.consent.entries().is_empty(), "{:?}", f.consent.entries());
}

/// The control: the same script shipped in the skill's directory and named
/// relative to it is content-bound. Approved, it runs; swapped, its approval
/// no longer covers it and the model gets the pending placeholder.
#[tokio::test]
async fn a_skill_script_named_relative_to_its_dir_is_bound_to_its_content() {
    const RELATIVE: &str = "sh x.sh";
    let f = Fixture::plugin("demo", &[RELATIVE]);
    let script = f.skill_dir().join("x.sh");
    std::fs::write(&script, "echo v1-ran\n").unwrap();

    let _ = f.read(Caller::Operator).await;
    f.approve_all();
    let approved = f.read(Caller::Operator).await;
    std::fs::write(&script, "echo v2-swapped\n").unwrap();
    let swapped = f.read(Caller::Operator).await;

    assert!(approved.contains("- v1-ran\n"), "{approved}");
    assert!(!swapped.contains("v2-swapped"), "{swapped}");
    assert!(swapped.contains(&pending(RELATIVE)), "{swapped}");
}
