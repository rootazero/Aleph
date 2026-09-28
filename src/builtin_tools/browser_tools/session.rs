// Browser session tool — persist and restore login/authentication state.
//
// Wraps `playwright-cli state-save` / `state-load`. A saved state file captures
// the managed browser context's cookies + localStorage (the authentication
// state), letting an agent log in once and reuse the session later without
// re-authenticating. State files live in a managed directory keyed by a safe
// name slug, so a caller can never write/read outside it.

use std::path::PathBuf;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Save or restore the browser's authentication/storage state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionAction {
    /// Capture the current cookies + localStorage to a named state file.
    Save,
    /// Restore cookies + localStorage from a previously-saved state file.
    Load,
    /// What each browser engine supports, and which engine to switch to for a
    /// verb this one cannot do. Reads a constant table: no browser is
    /// launched, no file is touched, no approval is consumed.
    Capabilities,
    /// Move this profile onto the other browser engine, carrying the cookies
    /// and the open tabs. The escape hatch: obscura renders fast and is the
    /// default; Chromium is what you switch to when a page needs it. Ask
    /// `action:"capabilities"` first if the question is which engine can do a
    /// particular verb.
    SwitchEngine,
}

/// Arguments for the `browser_session` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSessionArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Whether to save the current state or load a saved one.
    pub action: SessionAction,
    /// Name of the saved session (e.g. "github"). Stored under the managed
    /// browser state directory; must contain only letters, digits, '-', '_', '.'
    /// and may not start with '.' (no path separators or traversal).
    ///
    /// Required for `save` and `load`; ignored by `capabilities`.
    ///
    /// `Option` rather than `String` because `capabilities` has no session to
    /// name. A `save` that arrives without one is REFUSED with a message
    /// naming the field — never given a default, which would write somebody's
    /// whole authenticated identity to a filename the caller did not choose.
    /// Same shape, for the same reason, as `runtime_manage`'s `capability`.
    #[serde(default)]
    pub name: Option<String>,
    /// `switch_engine` only: which engine to move to.
    #[serde(default)]
    pub engine: Option<crate::browser::engine::Engine>,
    /// `switch_engine` only: carry cookies / tabs / localStorage across
    /// (default true). `false` starts the other engine clean.
    #[serde(default)]
    pub migrate: Option<bool>,
}

/// Output from the `browser_session` tool.
#[derive(Debug, Serialize)]
pub struct BrowserSessionOutput {
    pub success: bool,
    /// Absolute path of the state file that was written or read.
    pub path: Option<String>,
    pub message: Option<String>,
    /// The engine capability table, on `capabilities` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<serde_json::Value>,
    /// `switch_engine` only: the engine now serving this profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub switched_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cookies_moved: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tabs_reopened: Option<usize>,
    /// The new page tree. Every ref the model held died with the old process,
    /// so the replacement travels back in the same call rather than being
    /// something it must remember to re-request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_text: Option<String>,
}

impl BrowserSessionOutput {
    /// A refusal: no path, no capabilities, no engine, no counts. One
    /// constructor so a new field cannot be spelled `None` in six places and
    /// `Some` in none.
    fn failed(message: impl Into<String>) -> Self {
        Self {
            success: false,
            path: None,
            message: Some(message.into()),
            capabilities: None,
            switched_to: None,
            cookies_moved: None,
            tabs_reopened: None,
            snapshot_text: None,
        }
    }
}

/// Persists / restores browser login sessions via storage-state files.
#[derive(Clone)]
pub struct BrowserSessionTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserSessionTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate save/load behind the approval policy, classified as
    /// [`ActionType::BrowserSessionState`].
    ///
    /// `browser_cookies set` is gated on the stated ground that "a cookie value
    /// is a credential by design" — and this tool moves EVERY cookie plus
    /// localStorage in one call: `save` writes the whole authenticated identity
    /// to a file on disk, `load` installs someone's whole authenticated
    /// identity into the live browser. Leaving the bulk operation ungated while
    /// the single-cookie one asks made the gate trivially avoidable, so both
    /// route through the same policy key. A dedicated `BrowserSession` variant
    /// would read better in a policy file but lives in `src/approval/types.rs`.
    ///
    /// With no policy wired the tool behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }

    /// The `switch_engine` arm.
    ///
    /// Gated the same way `save`/`load` are, and one level up for the same
    /// reason: this moves every cookie into a new process. Argument validation
    /// runs BEFORE the gate so a malformed call cannot consume a user approval
    /// — the rule [`validate_session_name`] already follows.
    async fn switch_engine(&self, args: BrowserSessionArgs) -> Result<BrowserSessionOutput> {
        if args.name.is_some() {
            return Ok(BrowserSessionOutput::failed(
                "browser_session switch_engine takes no `name` — it moves the live \
                 session, it does not read a saved one",
            ));
        }
        let Some(engine) = args.engine else {
            return Ok(BrowserSessionOutput::failed(
                "browser_session switch_engine requires `engine`: \"obscura\" or \"chromium\"",
            ));
        };

        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserSwitchEngine,
            "switch_engine",
            &format!("switch profile '{}' to {engine}", args.profile),
        )
        .await
        {
            return Ok(BrowserSessionOutput::failed(message));
        }

        let migrate = args.migrate.unwrap_or(true);
        let profile = match super::resolve_caller_profile(&self.manager, &args.profile) {
            Ok(p) => p,
            Err(e) => {
                return Ok(BrowserSessionOutput::failed(super::backend_error_text(
                    &self.manager,
                    &e,
                )));
            }
        };
        match self.manager.switch_engine(&profile, engine, migrate).await {
            Ok(report) => {
                let mut message = format!(
                    "Switched profile '{}' from {} to {}: {} cookie(s), {} tab(s), \
                     {} localStorage origin(s) carried across. Every earlier ref is dead; \
                     {}. This profile runs {} until the server restarts or you switch \
                     back — the config file is unchanged.",
                    args.profile,
                    report.from,
                    report.to,
                    report.cookies_moved,
                    report.tabs_reopened,
                    report.local_storage_origins,
                    // Not "the new page tree is in snapshot_text" unconditionally:
                    // the read runs after the point of no return and may fail on a
                    // switch that worked, and a sentence naming a field that is
                    // absent is the wrong label (判据 §17). The reason is in
                    // `warnings`, which is appended below.
                    if report.snapshot.is_some() {
                        "the new page tree is in snapshot_text"
                    } else {
                        "run browser_snapshot for the new page tree"
                    },
                    report.to,
                );
                // ONE label for the block, not a claim per line. Every
                // warning already says what happened in its own words, and
                // `"Not carried: "` per line was false for two of the six
                // producers — the post-switch read failure and the
                // `user_data_dir` substitution are not losses of migrated
                // state, and the first of them is the exact sentence the
                // point-of-no-return degrade exists to make readable
                // (判据 §17: the wrong label costs more than the vague one).
                if !report.warnings.is_empty() {
                    message.push_str("\nWarnings:");
                    for w in &report.warnings {
                        message.push_str("\n- ");
                        message.push_str(w);
                    }
                }
                Ok(BrowserSessionOutput {
                    success: true,
                    path: None,
                    message: Some(message),
                    capabilities: None,
                    switched_to: Some(report.to.as_str().to_string()),
                    cookies_moved: Some(report.cookies_moved),
                    tabs_reopened: Some(report.tabs_reopened),
                    // Page text is page-derived bytes: through the same egress
                    // chokepoint every other page read uses, never raw.
                    snapshot_text: report
                        .snapshot
                        .as_ref()
                        .map(|s| super::redact_and_wrap(&self.manager, &s.snapshot_text)),
                })
            }
            Err(e) => Ok(BrowserSessionOutput::failed(format!(
                "Switch to {engine} failed: {}",
                super::backend_error_text(&self.manager, &e)
            ))),
        }
    }
}

/// Reject empty names, leading dots, and any character outside `[A-Za-z0-9._-]`
/// so a caller can never escape the managed directory.
///
/// Split out from [`resolve_session_path`] because the two run at different
/// points: the name is a pure check that must precede the approval gate (a
/// malformed name is a model mistake and must not consume a user approval),
/// while resolving the path CREATES the sessions directory — a side effect a
/// denied call must not leave behind.
fn validate_session_name(name: &str) -> std::result::Result<(), String> {
    if name.is_empty() {
        return Err("session name must not be empty".into());
    }
    if name.starts_with('.')
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(format!(
            "invalid session name '{name}': use only letters, digits, '-', '_', '.' \
             and do not start with '.' (no path separators)"
        ));
    }
    Ok(())
}

/// Resolve a validated session name to an absolute path under the managed
/// browser state directory (`~/.aleph/data/browser/sessions/<name>.json`),
/// creating the directory.
async fn resolve_session_path(name: &str) -> std::result::Result<PathBuf, String> {
    validate_session_name(name)?;
    let dir = crate::discovery::aleph_home_dir()
        .map_err(|e| format!("cannot resolve aleph home: {e}"))?
        .join("data")
        .join("browser")
        .join("sessions");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("cannot create session dir: {e}"))?;
    Ok(dir.join(format!("{name}.json")))
}

#[async_trait]
impl AlephTool for BrowserSessionTool {
    const NAME: &'static str = "browser_session";
    // Two-of-three capability — `BrowserBackend::{save_state,load_state}` are
    // served by the managed Playwright backend AND by the CDP backend
    // (Task 13); the Chrome DevTools MCP backend takes the trait default, which
    // refuses. See `pdf.rs`.
    // One sentence, not the table (plan ruling R2'). `DESCRIPTION` is a
    // `const &'static str` and the builtin catalog is a `const` array fed from
    // it, so the generated table cannot be injected here at runtime and a
    // hand-written copy would be a second author for it. The action serves the
    // table; `BrowserError::UnsupportedByEngine` names the gap and its remedy
    // at the moment a model actually trips over one.
    const DESCRIPTION: &'static str =
        "Save or restore a browser login session (cookies + localStorage) by name, \
         so a logged-in state can be reused without re-authenticating \
         — managed or cdp profiles only (e.g. profile='default'). \
         action='capabilities' lists what each browser engine supports and which \
         engine to switch to for a verb the current one cannot do. \
         Switch this profile's engine with action='switch_engine', \
         engine='chromium'|'obscura' when action='capabilities' says the running engine \
         cannot do a verb you need. It carries every cookie, each open tab's URL and \
         scroll, and that origin's localStorage; page state is lost and every earlier ref \
         is dead — action='capabilities' lists exactly what moves.";
    type Args = BrowserSessionArgs;
    type Output = BrowserSessionOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // First, and deliberately: this action reads a constant, launches
        // nothing, writes nothing and moves no credential, so it must not pass
        // through the session-name check (it has no name), the approval gate
        // (there is nothing to approve) or the directory creation (a read must
        // not leave a directory behind).
        if matches!(args.action, SessionAction::Capabilities) {
            return Ok(BrowserSessionOutput {
                success: true,
                path: None,
                message: Some(crate::browser::engine::describe_for_tool()),
                capabilities: Some(crate::browser::engine::capabilities_json()),
                switched_to: None,
                cookies_moved: None,
                tabs_reopened: None,
                snapshot_text: None,
            });
        }
        // Second, and for the same reason: a switch has no session name, so it
        // must not reach the `name` refusal below. It DOES pass an approval
        // gate — unlike `capabilities`, it starts a process and moves every
        // cookie — but that gate lives inside `switch_engine`, after its own
        // argument validation, so a malformed call cannot spend a user's
        // approval.
        if matches!(args.action, SessionAction::SwitchEngine) {
            return self.switch_engine(args).await;
        }
        // `name` is optional on the wire so `capabilities` could omit it. Every
        // other action needs one, and the refusal names the field rather than
        // inventing a default — a defaulted name here writes a whole
        // authenticated identity to a path the caller did not choose.
        let Some(name) = args.name.clone() else {
            return Ok(BrowserSessionOutput::failed(format!(
                "{:?} needs a `name` (the saved session to write or read). \
                 action='capabilities' is the one action that does not.",
                args.action
            )));
        };
        if let Err(e) = validate_session_name(&name) {
            return Ok(BrowserSessionOutput::failed(e));
        }

        // Gate AFTER name validation (a malformed name must not consume an
        // approval) and BEFORE both the sessions directory is created and the
        // backend is constructed, so a denied call leaves nothing behind. The
        // audit target names the session and direction, never the state file's
        // contents.
        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserSessionState,
            "session",
            &format!("{:?} auth state '{name}'", args.action),
        )
        .await
        {
            return Ok(BrowserSessionOutput::failed(message));
        }

        let path = match resolve_session_path(&name).await {
            Ok(p) => p,
            Err(e) => return Ok(BrowserSessionOutput::failed(e)),
        };
        let path_str = path.to_string_lossy().to_string();

        let backend = match super::make_backend(&self.manager, &args.profile) {
            Ok(b) => b,
            Err(e) => {
                return Ok(BrowserSessionOutput::failed(super::backend_error_text(
                    &self.manager,
                    &e,
                )));
            }
        };
        let result = match args.action {
            SessionAction::Save => backend.save_state(&path).await,
            SessionAction::Load => backend.load_state(&path).await,
            // Answered above, before the backend was even constructed. Spelled
            // as its own arm rather than folded into a `_ =>` so that the next
            // action added to this enum is a compile error here instead of
            // silently inheriting somebody else's behaviour (判据 §3 — count
            // the arms at the `match`, not at the test).
            SessionAction::Capabilities => {
                unreachable!("the capabilities action returns as call()'s first statement")
            }
            SessionAction::SwitchEngine => {
                unreachable!("switch_engine returns before the session-name check")
            }
        };
        match result {
            Ok(()) => Ok(BrowserSessionOutput {
                success: true,
                path: Some(path_str.clone()),
                message: Some(match args.action {
                    SessionAction::Save => {
                        format!("Saved session '{name}' to {path_str}")
                    }
                    SessionAction::Load => {
                        format!("Loaded session '{name}' from {path_str}")
                    }
                    SessionAction::Capabilities => {
                        unreachable!("the capabilities action returns as call()'s first statement")
                    }
                    SessionAction::SwitchEngine => {
                        unreachable!("switch_engine returns before the session-name check")
                    }
                }),
                capabilities: None,
                switched_to: None,
                cookies_moved: None,
                tabs_reopened: None,
                snapshot_text: None,
            }),
            Err(e) => Ok(BrowserSessionOutput::failed(format!(
                "Session {:?} failed: {}",
                args.action,
                super::backend_error_text(&self.manager, &e)
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_resolve_session_path_rejects_traversal() {
        assert!(resolve_session_path("").await.is_err());
        assert!(resolve_session_path("../etc/passwd").await.is_err());
        assert!(resolve_session_path("a/b").await.is_err());
        assert!(resolve_session_path("a\\b").await.is_err());
        assert!(resolve_session_path(".hidden").await.is_err());
        assert!(resolve_session_path("..").await.is_err());
        // Valid slugs resolve under the managed sessions directory.
        let p = resolve_session_path("github").await.unwrap();
        assert!(p.ends_with("browser/sessions/github.json"));
        assert!(resolve_session_path("my_site-1").await.is_ok());
    }

    #[tokio::test]
    async fn test_session_save_degrades_without_browser() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserSessionTool::new(manager);
        let result = tool
            .call(BrowserSessionArgs {
                profile: "default".into(),
                action: SessionAction::Save,
                name: Some("unit-test".into()),
                engine: None,
                migrate: None,
            })
            .await
            .unwrap();
        // Without a running browser the save fails gracefully.
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    fn deny_policy() -> Arc<crate::approval::ConfigApprovalPolicy> {
        use crate::approval::{ConfigApprovalPolicy, DefaultDecision, PolicyConfig};
        let mut defaults = std::collections::HashMap::new();
        defaults.insert(ActionType::BrowserSessionState, DefaultDecision::Deny);
        Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        }))
    }

    #[tokio::test]
    async fn test_session_save_is_gated_before_the_backend() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserSessionTool::new(manager).with_approval_policy(deny_policy());
        let result = tool
            .call(BrowserSessionArgs {
                profile: "default".into(),
                action: SessionAction::Save,
                name: Some("github".into()),
                engine: None,
                migrate: None,
            })
            .await
            .unwrap();
        assert!(!result.success);
        // The denial — not a "no browser running" error — proves the gate ran
        // before the backend was constructed. And nothing was written.
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("denied by approval policy")),
            "got: {:?}",
            result.message
        );
        assert!(result.path.is_none());
    }

    #[tokio::test]
    async fn test_session_load_is_gated_before_the_backend() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserSessionTool::new(manager).with_approval_policy(deny_policy());
        let result = tool
            .call(BrowserSessionArgs {
                profile: "default".into(),
                action: SessionAction::Load,
                name: Some("github".into()),
                engine: None,
                migrate: None,
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("denied by approval policy")),
            "got: {:?}",
            result.message
        );
    }

    #[tokio::test]
    async fn test_session_bad_name_does_not_consume_approval() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserSessionTool::new(manager).with_approval_policy(deny_policy());
        let result = tool
            .call(BrowserSessionArgs {
                profile: "default".into(),
                action: SessionAction::Load,
                name: Some("../evil".into()),
                engine: None,
                migrate: None,
            })
            .await
            .unwrap();
        assert!(!result.success);
        let message = result.message.unwrap();
        assert!(message.contains("invalid session name"), "got: {message}");
        assert!(!message.contains("denied"), "got: {message}");
    }

    #[tokio::test]
    async fn test_session_rejects_bad_name_before_backend() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserSessionTool::new(manager);
        let result = tool
            .call(BrowserSessionArgs {
                profile: "default".into(),
                action: SessionAction::Load,
                name: Some("../evil".into()),
                engine: None,
                migrate: None,
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.unwrap().contains("invalid session name"));
    }

    /// `capabilities` answers from a constant table. It must not require a
    /// session name, must not create the sessions directory, and must not
    /// consume an approval — it reads nothing and moves no credential.
    ///
    /// Driven through a DENY policy on purpose: a `capabilities` that fell
    /// through to the gate would be refused here, so this is the assertion
    /// that the short-circuit is really first (判据 §4 — the effect, not the
    /// call).
    #[tokio::test]
    async fn capabilities_needs_no_name_no_directory_and_no_approval() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserSessionTool::new(manager).with_approval_policy(deny_policy());
        let out = tool
            .call(BrowserSessionArgs {
                profile: "default".into(),
                action: SessionAction::Capabilities,
                name: None,
                engine: None,
                migrate: None,
            })
            .await
            .expect("capabilities must not error");
        assert!(out.success, "{:?}", out.message);
        assert!(out.path.is_none(), "nothing is written");
        let caps = out.capabilities.expect("the table must be in the output");
        assert_eq!(caps["obscura"]["js_dialogs"], "unsupported");
        assert_eq!(caps["chromium"]["js_dialogs"], "supported");
        let message = out.message.unwrap_or_default();
        assert!(
            message.contains("switch_engine"),
            "the prose must name the way across: {message}"
        );
        assert!(
            !message.contains("denied by approval policy"),
            "the approval gate ran for an action that moves no credential: {message}"
        );
    }

    /// `name` became optional so `capabilities` could omit it. A `save` with no
    /// name must therefore REFUSE and say which field is missing — not invent
    /// one, and not fall through to a path built from a default. Mirrors
    /// `runtime_manage`'s `install_without_a_capability_refuses_instead_of_guessing`.
    #[tokio::test]
    async fn save_without_a_name_refuses_instead_of_guessing() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserSessionTool::new(manager);
        for action in [SessionAction::Save, SessionAction::Load] {
            let out = tool
                .call(BrowserSessionArgs {
                    profile: "default".into(),
                    action,
                    name: None,
                    engine: None,
                    migrate: None,
                })
                .await
                .unwrap();
            assert!(!out.success, "{action:?}");
            let msg = out.message.unwrap_or_default();
            assert!(msg.contains("name"), "{action:?}: {msg}");
            assert!(out.path.is_none(), "{action:?}: {:?}", out.path);
        }
    }

    /// The description must name the action — a table with an owner and no verb
    /// is a producer nothing can reach (判据 §7) — and the catalog must serve
    /// the tool's own const rather than a literal of its own. That substitution
    /// is the defect `definitions.rs`'s module doc records, and it is invisible
    /// from every direction except this assertion.
    #[test]
    fn the_catalog_serves_this_tools_own_description() {
        let d = <BrowserSessionTool as AlephTool>::DESCRIPTION;
        assert!(d.contains("capabilities"), "{d}");
        let entry = crate::executor::BUILTIN_TOOL_DEFINITIONS
            .iter()
            .find(|e| e.name == <BrowserSessionTool as AlephTool>::NAME)
            .expect("browser_session must be in the catalog");
        assert_eq!(entry.description, d);
    }

    fn deny_switch_policy() -> Arc<crate::approval::ConfigApprovalPolicy> {
        use crate::approval::{ConfigApprovalPolicy, DefaultDecision, PolicyConfig};
        let mut defaults = std::collections::HashMap::new();
        defaults.insert(ActionType::BrowserSwitchEngine, DefaultDecision::Deny);
        Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        }))
    }

    #[tokio::test]
    async fn switch_engine_is_gated_before_the_manager() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserSessionTool::new(manager).with_approval_policy(deny_switch_policy());
        let result = tool
            .call(BrowserSessionArgs {
                profile: "default".into(),
                action: SessionAction::SwitchEngine,
                name: None,
                engine: Some(crate::browser::engine::Engine::Chromium),
                migrate: None,
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("denied by approval policy")),
            "got: {:?}",
            result.message
        );
        assert!(result.switched_to.is_none());
    }

    #[tokio::test]
    async fn switch_engine_without_an_engine_does_not_consume_an_approval() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserSessionTool::new(manager).with_approval_policy(deny_switch_policy());
        let result = tool
            .call(BrowserSessionArgs {
                profile: "default".into(),
                action: SessionAction::SwitchEngine,
                name: None,
                engine: None,
                migrate: None,
            })
            .await
            .unwrap();
        let message = result.message.unwrap();
        assert!(message.contains("requires `engine`"), "got: {message}");
        assert!(!message.contains("denied"), "got: {message}");
    }

    /// The tool's SUCCESS arm, which had no coverage at all.
    ///
    /// Two claims, and the second is the one this round is for:
    ///
    /// 1. a completed switch reports `switched_to` and the counts;
    /// 2. the warnings are labelled ONCE, for the block. They used to be
    ///    rendered `"Not carried: " + w` per line, and two of the producers are
    ///    not carry losses at all — the post-switch read failure (the sentence
    ///    the point-of-no-return degrade exists to make readable) and the
    ///    `user_data_dir` substitution. Telling a model that a COMPLETED switch
    ///    failed to carry the page tree is the wrong label (判据 §17).
    #[tokio::test]
    async fn a_completed_switch_labels_its_warnings_once_and_truthfully() {
        use crate::browser::engine::Engine;
        use crate::browser::testkit::switch_fixture;

        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());
        let fixture = switch_fixture(Engine::Obscura, Engine::Chromium, false).await;
        let tool = BrowserSessionTool::new(Arc::clone(&fixture.manager));

        let out = tool
            .call(BrowserSessionArgs {
                profile: "default".into(),
                action: SessionAction::SwitchEngine,
                name: None,
                engine: Some(Engine::Chromium),
                migrate: Some(true),
            })
            .await
            .expect("the tool never errors; it degrades");

        assert!(out.success, "{:?}", out.message);
        assert_eq!(out.switched_to.as_deref(), Some("chromium"));
        assert_eq!(out.cookies_moved, Some(1));
        let message = out.message.expect("a success says what it did");

        // This fake cannot serve a `Page.getLayoutMetrics`, so the page tree is
        // unreadable and the degrade fires — which is exactly the warning whose
        // label was wrong.
        assert!(
            message.contains("could not be read"),
            "the degraded read must be stated: {message}"
        );
        assert!(
            !message.contains("Not carried:"),
            "a completed switch did not fail to CARRY the page tree; the label \
             must not say it did: {message}"
        );
        assert!(
            message.contains("Warnings:"),
            "the block still needs one true label: {message}"
        );
        assert!(
            message.contains("run browser_snapshot"),
            "and it must name the verb that gets the tree: {message}"
        );
    }

    /// R2': the capability TABLE is the action's RESULT and must never be in
    /// the tool's `DESCRIPTION`, where it would cost prompt bytes on every turn
    /// for a fact most turns never need. A guard rather than a comment, because
    /// the natural "improvement" is to paste the helpful paragraph in.
    #[test]
    fn the_description_points_at_the_action_and_does_not_carry_the_table() {
        let d = <BrowserSessionTool as AlephTool>::DESCRIPTION;
        let table = crate::browser::engine::describe_for_tool();
        for line in table.lines().filter(|l| !l.trim().is_empty()) {
            assert!(
                !d.contains(line),
                "the capability table has leaked into DESCRIPTION (R2'): {line:?}"
            );
        }
        // And the specific facts it must not spend bytes on.
        for leaked in ["unsupported", "Measured on", "v0.2.2"] {
            assert!(!d.contains(leaked), "DESCRIPTION carries {leaked:?}: {d}");
        }
    }
}
