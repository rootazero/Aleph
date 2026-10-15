// browser_companion: start / status / stop an Aleph-managed, sandboxed
// companion process (dev server / design companion) whose page is published
// to the browser-live viewer through the existing producer.

use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::manager::{
    CallerIdentity, CompanionLauncher, CompanionManager, CompanionTabs, LaunchRequest, StartError,
    StopError,
};
use super::process::SandboxLauncher;
use super::protocol::{CanonicalEndpoint, HttpEndpointProver};
use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::backend::BrowserBackend;
use crate::browser::cdp_backend::{BrowserOperationRecord, CdpBackend};
use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sandbox::Sandbox;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

const MAX_PROGRAM_BYTES: usize = 256;
const MAX_ARGS: usize = 32;
const MAX_ARG_BYTES: usize = 1024;
const MAX_CWD_BYTES: usize = 256;
const MAX_READY_SECS: u64 = 60;
const DEFAULT_READY_SECS: u64 = 30;
const MAX_LIFETIME_SECS: u64 = 3600;
const DEFAULT_LIFETIME_SECS: u64 = 600;
const CHALLENGE_BUDGET: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompanionAction {
    Start,
    Status,
    Stop,
}

/// Arguments for the `browser_companion` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserCompanionArgs {
    /// `start`, `status` or `stop`.
    pub action: CompanionAction,
    /// start: executable to run (no shell; not a script line).
    #[serde(default)]
    pub program: Option<String>,
    /// start: argv for the program (at most 32 entries).
    #[serde(default)]
    pub args: Vec<String>,
    /// start: working directory relative to the session workspace.
    #[serde(default)]
    pub cwd: Option<String>,
    /// start: the exact loopback origin the service will listen on, canonical
    /// spelling with an explicit port, e.g. `http://127.0.0.1:5173/`.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// start: seconds to wait for the service to report ready (max 60).
    #[serde(default)]
    pub ready_timeout_secs: Option<u64>,
    /// start: seconds the companion may live before it is closed (max 3600).
    #[serde(default)]
    pub lifetime_secs: Option<u64>,
    /// start: browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// status / stop: the opaque id returned by `start`.
    #[serde(default)]
    pub companion_id: Option<String>,
}

/// Output from the `browser_companion` tool.
#[derive(Debug, Serialize)]
pub struct BrowserCompanionOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub companion_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl BrowserCompanionOutput {
    fn fail(message: impl Into<String>) -> Self {
        Self {
            success: false,
            companion_id: None,
            state: None,
            endpoint: None,
            message: Some(message.into()),
        }
    }
}

/// Real tab seam: the CDP backend's `open_tab_with_record` receipt.
struct BackendTabs {
    backend: Arc<dyn BrowserBackend>,
    profiles: Arc<ProfileManager>,
}

#[async_trait]
impl CompanionTabs for BackendTabs {
    async fn open_tab(
        &self,
        url: &str,
    ) -> std::result::Result<(String, BrowserOperationRecord), String> {
        let cdp = self
            .backend
            .as_any()
            .downcast_ref::<CdpBackend>()
            .ok_or_else(|| "the profile is not CDP-backed; no operation receipt".to_owned())?;
        cdp.open_tab_with_record(url)
            .await
            .map_err(|e| super::super::backend_error_text(&self.profiles, &e))
    }

    async fn close_tab(&self, tab_id: &str) -> std::result::Result<(), String> {
        self.backend
            .close_tab(tab_id)
            .await
            .map_err(|e| super::super::backend_error_text(&self.profiles, &e))
    }
}

#[derive(Clone)]
pub struct BrowserCompanionTool {
    profiles: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
    manager: Arc<CompanionManager>,
}

impl BrowserCompanionTool {
    /// `sandbox` is the registry's workspace sandbox (the SAME admission path
    /// as `bash`); `producer` the registry's existing browser-live producer.
    pub fn new(
        profiles: Arc<ProfileManager>,
        sandbox: Option<Arc<dyn Sandbox>>,
        producer: Option<super::super::present::BrowserLiveProducer>,
    ) -> Self {
        let launcher: Option<Arc<dyn CompanionLauncher>> =
            sandbox.map(|s| Arc::new(SandboxLauncher::new(s)) as Arc<dyn CompanionLauncher>);
        let prover = Arc::new(HttpEndpointProver {
            budget: CHALLENGE_BUDGET,
        });
        Self {
            profiles,
            approval_policy: None,
            manager: CompanionManager::new(producer, launcher, prover),
        }
    }

    /// Same approval policy as `browser_open`: the page is opened in a tab.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

fn bounded(label: &str, value: &str, max: usize) -> std::result::Result<(), String> {
    if value.len() > max || value.chars().any(char::is_control) {
        return Err(format!("{label} is too long or has control characters"));
    }
    Ok(())
}

/// Validate a `start` call into a structured launch request.
fn build_request(args: &BrowserCompanionArgs) -> std::result::Result<LaunchRequest, String> {
    let program = args
        .program
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .ok_or("start requires `program`")?;
    bounded("program", program, MAX_PROGRAM_BYTES)?;
    if program.chars().any(char::is_whitespace) {
        return Err("`program` is one executable name or path, not a command line".into());
    }
    if args.args.len() > MAX_ARGS {
        return Err(format!("at most {MAX_ARGS} args"));
    }
    for arg in &args.args {
        bounded("arg", arg, MAX_ARG_BYTES)?;
    }
    if let Some(cwd) = &args.cwd {
        bounded("cwd", cwd, MAX_CWD_BYTES)?;
        let path = std::path::Path::new(cwd);
        if path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err("`cwd` must be relative to the workspace, without `..`".into());
        }
    }
    let raw = args
        .endpoint
        .as_deref()
        .ok_or("start requires `endpoint`")?;
    let endpoint = CanonicalEndpoint::parse(raw).map_err(|e| e.to_string())?;
    let ready = args.ready_timeout_secs.unwrap_or(DEFAULT_READY_SECS);
    let lifetime = args.lifetime_secs.unwrap_or(DEFAULT_LIFETIME_SECS);
    if ready == 0 || ready > MAX_READY_SECS {
        return Err(format!("ready_timeout_secs must be 1..={MAX_READY_SECS}"));
    }
    if lifetime == 0 || lifetime > MAX_LIFETIME_SECS {
        return Err(format!("lifetime_secs must be 1..={MAX_LIFETIME_SECS}"));
    }
    Ok(LaunchRequest {
        program: program.to_owned(),
        args: args.args.clone(),
        cwd: args.cwd.clone(),
        endpoint,
        ready_timeout: Duration::from_secs(ready),
        lifetime: Duration::from_secs(lifetime),
    })
}

#[async_trait]
impl AlephTool for BrowserCompanionTool {
    const NAME: &'static str = "browser_companion";
    const DESCRIPTION: &'static str = "Run a sandboxed companion service (e.g. a dev server) and show its page in the browser viewer. start: program, args, endpoint (loopback origin with port). status/stop: companion_id only.";
    type Args = BrowserCompanionArgs;
    type Output = BrowserCompanionOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args.action {
            CompanionAction::Start => Ok(self.start(&args).await),
            CompanionAction::Status | CompanionAction::Stop => Ok(self.inspect(&args).await),
        }
    }
}

impl BrowserCompanionTool {
    async fn start(&self, args: &BrowserCompanionArgs) -> BrowserCompanionOutput {
        let request = match build_request(args) {
            Ok(r) => r,
            Err(message) => return BrowserCompanionOutput::fail(message),
        };
        let Some(owner) = CallerIdentity::capture() else {
            return BrowserCompanionOutput::fail("no session or run in scope");
        };
        // Unsupported hosts / unconfigured publication fail before any card.
        if let Err(error) = self.manager.preflight() {
            return BrowserCompanionOutput::fail(error.to_string());
        }
        let endpoint = request.endpoint.as_string();
        // Private networks stay denied by default (managed companions have no
        // exemption); a loopback endpoint needs the operator's explicit
        // `block_private = false`.
        if let Err(violation) = self.profiles.check_navigation(&endpoint).await {
            return BrowserCompanionOutput::fail(format!("Blocked: {violation}"));
        }
        if let Some(message) = super::super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserOpen,
            "open",
            &endpoint,
        )
        .await
        {
            return BrowserCompanionOutput::fail(message);
        }
        let profile = match super::super::resolve_caller_profile(&self.profiles, &args.profile) {
            Ok(p) => p,
            Err(e) => {
                return BrowserCompanionOutput::fail(super::super::backend_error_text(
                    &self.profiles,
                    &e,
                ))
            }
        };
        if let Err(e) = self.profiles.prepare_engine(&profile, None).await {
            return BrowserCompanionOutput::fail(super::super::backend_error_text(
                &self.profiles,
                &e,
            ));
        }
        let backend = match super::super::backend_for_key(&self.profiles, &profile) {
            Ok(b) => b,
            Err(e) => {
                return BrowserCompanionOutput::fail(super::super::backend_error_text(
                    &self.profiles,
                    &e,
                ))
            }
        };
        if backend.as_any().downcast_ref::<CdpBackend>().is_none() {
            return BrowserCompanionOutput::fail(
                "browser_companion needs a CDP-backed profile; nothing was launched",
            );
        }
        let tabs = Arc::new(BackendTabs {
            backend,
            profiles: Arc::clone(&self.profiles),
        });
        match self.manager.start(owner, request, tabs).await {
            Ok((id, endpoint)) => BrowserCompanionOutput {
                success: true,
                companion_id: Some(id),
                state: Some("running".into()),
                endpoint: Some(endpoint),
                message: Some(
                    "Companion running; its page is published for explicit viewing.".into(),
                ),
            },
            Err(error @ StartError::Unsupported(_)) => {
                BrowserCompanionOutput::fail(error.to_string())
            }
            Err(error) => BrowserCompanionOutput::fail(error.to_string()),
        }
    }

    async fn inspect(&self, args: &BrowserCompanionArgs) -> BrowserCompanionOutput {
        let Some(id) = args.companion_id.as_deref().filter(|i| i.len() <= 64) else {
            return BrowserCompanionOutput::fail("companion_id is required");
        };
        let Some(owner) = CallerIdentity::capture() else {
            return BrowserCompanionOutput::fail("no session or run in scope");
        };
        if args.action == CompanionAction::Stop {
            return match self.manager.stop(&owner, id).await {
                Ok(()) => BrowserCompanionOutput {
                    success: true,
                    companion_id: Some(id.to_owned()),
                    state: Some("stopped".into()),
                    endpoint: None,
                    message: Some("Resource closed and the process exit was confirmed.".into()),
                },
                Err(StopError::NotFound) => BrowserCompanionOutput::fail("unknown companion"),
                Err(error) => BrowserCompanionOutput::fail(error.to_string()),
            };
        }
        match self.manager.status(&owner, id) {
            Ok((state, endpoint, _alive)) => BrowserCompanionOutput {
                success: true,
                companion_id: Some(id.to_owned()),
                state: Some(state),
                endpoint: Some(endpoint),
                message: None,
            },
            Err(_) => BrowserCompanionOutput::fail("unknown companion"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    fn start_args() -> BrowserCompanionArgs {
        serde_json::from_value(serde_json::json!({
            "action": "start",
            "program": "dev-server",
            "args": ["--port", "5173"],
            "endpoint": "http://127.0.0.1:5173/"
        }))
        .expect("args")
    }

    #[test]
    fn args_deny_unknown_fields_and_pid_or_target_addressing() {
        for extra in ["pid", "target_id", "process_id", "viewer_url"] {
            let mut value = serde_json::json!({ "action": "stop", "companion_id": "x" });
            value[extra] = serde_json::json!(1);
            assert!(
                serde_json::from_value::<BrowserCompanionArgs>(value).is_err(),
                "{extra} must be rejected"
            );
        }
        assert!(serde_json::from_value::<BrowserCompanionArgs>(
            serde_json::json!({ "action": "restart" })
        )
        .is_err());
    }

    #[test]
    fn start_request_is_structured_and_bounded() {
        let req = build_request(&start_args()).expect("valid");
        assert_eq!(req.program, "dev-server");
        assert_eq!(req.lifetime, Duration::from_secs(DEFAULT_LIFETIME_SECS));

        let mut shell = start_args();
        shell.program = Some("npm run dev && curl x".into());
        assert!(build_request(&shell).is_err(), "no command lines");
        let mut many = start_args();
        many.args = vec!["a".into(); MAX_ARGS + 1];
        assert!(build_request(&many).is_err());
        let mut abs = start_args();
        abs.cwd = Some("../outside".into());
        assert!(build_request(&abs).is_err());
        let mut long_life = start_args();
        long_life.lifetime_secs = Some(MAX_LIFETIME_SECS + 1);
        assert!(build_request(&long_life).is_err());
        let mut remote = start_args();
        remote.endpoint = Some("http://example.com:80/".into());
        assert!(build_request(&remote).is_err(), "only canonical loopback");
        let mut none = start_args();
        none.endpoint = None;
        assert!(build_request(&none).is_err());
    }

    #[tokio::test]
    async fn start_without_publication_or_sandbox_fails_closed_without_launching() {
        let tool = BrowserCompanionTool::new(
            Arc::new(ProfileManager::new(BrowserSystemConfig::default())),
            None,
            None,
        );
        // No session in scope is reported before anything else runs.
        let out = tool.call(start_args()).await.expect("output");
        assert!(!out.success && out.companion_id.is_none());
    }

    #[tokio::test]
    async fn status_and_stop_need_an_id_and_unknown_ids_look_identical() {
        let tool = BrowserCompanionTool::new(
            Arc::new(ProfileManager::new(BrowserSystemConfig::default())),
            None,
            None,
        );
        let missing: BrowserCompanionArgs =
            serde_json::from_value(serde_json::json!({ "action": "status" })).expect("args");
        let out = tool.call(missing).await.expect("output");
        assert!(!out.success);
        assert_eq!(out.message.as_deref(), Some("companion_id is required"));
    }

    #[test]
    fn description_names_no_unverified_capability() {
        assert!(!BrowserCompanionTool::DESCRIPTION.contains("shell"));
        assert_eq!(BrowserCompanionTool::NAME, "browser_companion");
    }
}
