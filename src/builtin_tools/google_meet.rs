//! `google_meet` — LLM-invoked tool for joining / creating / controlling a
//! Google Meet call through an **external transport bridge**.
//!
//! # Why this lives in core as a *thin contract* only
//!
//! A full Google Meet participant (joining a call, capturing duplex audio,
//! realtime transcription) is fundamentally platform automation — Chrome
//! browser control, CoreAudio/BlackHole/SoX device routing, Twilio dial-in.
//! Per the architectural redlines that work **must not** live in the Rust
//! core:
//!
//! - **R1 (Brain–Limb separation):** no platform system API (Chrome, `CoreAudio`)
//!   may be called from `src`; physical capability is provided by a native /
//!   external bridge over IPC.
//! - **R3 (Core minimalism):** a single non-core feature must not drag heavy
//!   dependencies into core — it belongs in a Skill / MCP server / plugin.
//!
//! The reference implementation (openclaw `extensions/google-meet`) is itself a
//! *plugin*, not core, for exactly these reasons.
//!
//! So this module is deliberately small: it defines the **capability contract**
//! (the actions an agent can request and their JSON schema, mirroring
//! openclaw's `google_meet` tool), validates the request, and forwards it as a
//! JSON-RPC call to whatever transport bridge the operator has configured. The
//! heavy lifting (Chrome/Twilio/audio/realtime) lives outside core. When no
//! bridge is configured the tool returns a structured, actionable
//! "not configured" result rather than failing hard (R5: AI degrades
//! gracefully, never crashes the loop).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{info, warn};

use crate::security::ssrf::SsrfPolicy;

use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Bridge configuration
// =============================================================================

/// Connection details for the external Google Meet transport bridge.
///
/// The bridge is an out-of-core process (Skill / MCP server / native helper)
/// that owns the platform automation. Core only needs its JSON-RPC endpoint.
#[derive(Clone, Debug)]
pub struct GoogleMeetBridge {
    /// JSON-RPC 2.0 HTTP endpoint of the transport bridge.
    pub url: String,
    /// Optional bearer token presented to the bridge.
    pub token: Option<String>,
    /// Per-request timeout.
    pub timeout: Duration,
}

impl GoogleMeetBridge {
    /// Build a bridge from environment variables, returning `None` when no
    /// endpoint is configured.
    ///
    /// - `ALEPH_GOOGLE_MEET_BRIDGE_URL` — required to enable the bridge.
    /// - `ALEPH_GOOGLE_MEET_BRIDGE_TOKEN` — optional bearer token.
    /// - `ALEPH_GOOGLE_MEET_BRIDGE_TIMEOUT_SECS` — optional, defaults to 30.
    ///
    /// Mirrors the `TAVILY_API_KEY` env-fallback precedent used by the search
    /// tool — an external endpoint is configured out-of-band, not baked into
    /// the core TOML.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("ALEPH_GOOGLE_MEET_BRIDGE_URL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        let token = std::env::var("ALEPH_GOOGLE_MEET_BRIDGE_TOKEN")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let timeout = std::env::var("ALEPH_GOOGLE_MEET_BRIDGE_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(30);
        Some(Self {
            url,
            token,
            timeout: Duration::from_secs(timeout),
        })
    }
}

// =============================================================================
// Args
// =============================================================================

/// Action to perform on a Google Meet call. Mirrors the openclaw `google_meet`
/// tool's core action surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoogleMeetAction {
    /// Join an explicit `https://meet.google.com/...` URL (or meeting code).
    Join,
    /// Create a new Meet space and join the returned URL.
    Create,
    /// Leave the active call.
    Leave,
    /// Speak text into the active call (agent talk-back).
    Speak,
    /// Report the bridge / call status without changing it.
    Status,
}

impl GoogleMeetAction {
    /// JSON-RPC method name forwarded to the bridge.
    const fn rpc_method(self) -> &'static str {
        match self {
            Self::Join => "googlemeet.join",
            Self::Create => "googlemeet.create",
            Self::Leave => "googlemeet.leave",
            Self::Speak => "googlemeet.speak",
            Self::Status => "googlemeet.status",
        }
    }
}

/// Transport the bridge should use. Observability of which transports are
/// actually wired is the bridge's concern; core only relays the preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum GoogleMeetTransport {
    /// Signed-in local Chrome profile.
    Chrome,
    /// Chrome running on a paired node host.
    ChromeNode,
    /// Twilio Meet dial-in.
    Twilio,
}

/// Talk-back behaviour for the join.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoogleMeetMode {
    /// Realtime transcription + the Aleph agent answers via TTS.
    Agent,
    /// Direct realtime voice model.
    Bidi,
    /// Observe only — join without the talk-back bridge.
    Transcribe,
}

/// Arguments for the `google_meet` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GoogleMeetArgs {
    /// What to do: join, create, leave, speak, or query status.
    pub action: GoogleMeetAction,

    /// Meet URL or meeting code — required for `join`.
    #[serde(default)]
    pub meeting: Option<String>,

    /// Transport preference — optional; bridge picks its default when omitted.
    #[serde(default)]
    pub transport: Option<GoogleMeetTransport>,

    /// Talk-back mode — optional; bridge picks its default when omitted.
    #[serde(default)]
    pub mode: Option<GoogleMeetMode>,

    /// Text to speak into the call — required for `speak`.
    #[serde(default)]
    pub text: Option<String>,
}

// =============================================================================
// Output
// =============================================================================

/// Output from the `google_meet` tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoogleMeetOutput {
    /// Whether the bridge accepted and performed the action.
    pub ok: bool,
    /// The action that was requested.
    pub action: GoogleMeetAction,
    /// Short machine-readable status (e.g. "joined", "`bridge_not_configured`").
    pub status: String,
    /// Human/agent-readable detail.
    pub detail: String,
    /// Meeting URL when the bridge returns one (join/create).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meeting_url: Option<String>,
}

// =============================================================================
// Tool
// =============================================================================

/// Thin contract tool that forwards Google Meet actions to an external
/// transport bridge over JSON-RPC. Holds an optional bridge; when absent the
/// tool still exists (so the capability is always discoverable) but reports
/// that no transport is configured.
#[derive(Clone)]
pub struct GoogleMeetTool {
    bridge: Option<Arc<GoogleMeetBridge>>,
    /// SSRF policy for the pre-flight check on the meeting URL —
    /// the bridge is operator-hosted and could dereference a
    /// host-policy-allowed hostname that flips to a blocked IP
    /// (the same DNS-rebinding shape `web_fetch` was patched
    /// against). Sourced from the operator's `[ssrf]` config
    /// block at construction time.
    ssrf_policy: SsrfPolicy,
}

impl GoogleMeetTool {
    /// Construct with an optional configured bridge and the
    /// operator's SSRF policy.
    #[must_use]
    pub const fn new(bridge: Option<Arc<GoogleMeetBridge>>, ssrf_policy: SsrfPolicy) -> Self {
        Self { bridge, ssrf_policy }
    }

    /// Require a string field, returning a fixable validation error naming it.
    fn require<'a>(value: &'a Option<String>, field: &str, action: &str) -> Result<&'a str> {
        value
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                AlephError::tool(format!("`{field}` is required for the `{action}` action"))
            })
    }

    /// Validate action-specific required fields before touching the network.
    fn validate(args: &GoogleMeetArgs) -> Result<()> {
        match args.action {
            GoogleMeetAction::Join => {
                Self::require(&args.meeting, "meeting", "join")?;
            }
            GoogleMeetAction::Speak => {
                Self::require(&args.text, "text", "speak")?;
            }
            // create / leave / status carry no mandatory fields.
            GoogleMeetAction::Create | GoogleMeetAction::Leave | GoogleMeetAction::Status => {}
        }
        Ok(())
    }

    /// Structured result for the unconfigured case — actionable, not an error.
    fn not_configured(action: GoogleMeetAction) -> GoogleMeetOutput {
        GoogleMeetOutput {
            ok: false,
            action,
            status: "bridge_not_configured".to_string(),
            detail: "No Google Meet transport bridge is configured. Google Meet participation \
                     (Chrome/Twilio automation + audio) runs out-of-core as a bridge plugin. \
                     Set ALEPH_GOOGLE_MEET_BRIDGE_URL (and optionally \
                     ALEPH_GOOGLE_MEET_BRIDGE_TOKEN) to the bridge's JSON-RPC endpoint, then \
                     retry."
                .to_string(),
            meeting_url: None,
        }
    }

    /// Forward the validated request to the bridge as a JSON-RPC 2.0 call.
    async fn dispatch_to_bridge(
        bridge: &GoogleMeetBridge,
        args: &GoogleMeetArgs,
    ) -> Result<GoogleMeetOutput> {
        // PR-10 / BT-D-R4-24: stamp the caller's actor into the
        // JSON-RPC request so the bridge can implement per-caller
        // ownership. Previously the tool forwarded the call with no
        // caller identification, so any actor reaching this tool could
        // leave / speak into a call another actor had joined, with the
        // bridge having no way to know who the rightful owner was.
        // The actor is read from the TURN_CONTEXT task-local; a
        // separate process that does not set one (cron, headless
        // tooling) gets "system" so the bridge can decide.
        let actor = crate::builtin_tools::acting_agent::acting_agent_id("system");
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": args.action.rpc_method(),
            "params": {
                "actor": actor,
                "args": args,
            },
        });

        let client = reqwest::Client::builder()
            .timeout(bridge.timeout)
            .build()
            .map_err(|e| AlephError::tool(format!("failed to build HTTP client: {e}")))?;

        let mut req = client.post(&bridge.url).json(&request);
        if let Some(ref token) = bridge.token {
            req = req.bearer_auth(token);
        }

        let resp = req.send().await.map_err(|e| {
            AlephError::tool(format!(
                "Google Meet bridge request failed ({}): {}",
                bridge.url, e
            ))
        })?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await.map_err(|e| {
            AlephError::tool(format!("Google Meet bridge returned invalid JSON: {e}"))
        })?;

        // JSON-RPC error envelope.
        if let Some(err) = body.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown bridge error");
            warn!(action = ?args.action, %status, "Google Meet bridge error: {}", msg);
            return Ok(GoogleMeetOutput {
                ok: false,
                action: args.action,
                status: "bridge_error".to_string(),
                detail: msg.to_string(),
                meeting_url: None,
            });
        }

        // A non-2xx HTTP status with no JSON-RPC `error` envelope is still a
        // failure. Inferring success purely from the absence of `error` would
        // turn a 5xx (or an HTML-ish error body) into a false-positive "ok".
        if !status.is_success() {
            warn!(action = ?args.action, %status, "Google Meet bridge returned non-success HTTP status");
            return Ok(GoogleMeetOutput {
                ok: false,
                action: args.action,
                status: "bridge_error".to_string(),
                detail: format!("bridge returned HTTP {status}"),
                meeting_url: None,
            });
        }

        let result = body
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let meeting_url = result
            .get("meeting_url")
            .or_else(|| result.get("url"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let detail = result
            .get("detail")
            .or_else(|| result.get("message"))
            .and_then(|v| v.as_str())
            .map_or_else(
                || format!("{} completed", args.action.rpc_method()),
                str::to_string,
            );

        info!(action = ?args.action, "Google Meet bridge action ok");
        Ok(GoogleMeetOutput {
            ok: true,
            action: args.action,
            status: "ok".to_string(),
            detail,
            meeting_url,
        })
    }
}

#[async_trait]
impl AlephTool for GoogleMeetTool {
    const NAME: &'static str = "google_meet";
    const DESCRIPTION: &'static str =
        "Join, create, leave, speak into, or query a Google Meet call via the configured \
         transport bridge. Use `join` with an explicit meet.google.com URL or code; `create` \
         to open a new Meet space; `speak` to say text in the call; `leave` to hang up; \
         `status` to check readiness. Audio/Chrome/Twilio automation runs out-of-core in a \
         bridge plugin — if none is configured the tool reports how to set it up.";

    type Args = GoogleMeetArgs;
    type Output = GoogleMeetOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        Self::validate(&args)?;
        // BT-D-R4-21: SSRF pre-flight on the meeting URL before
        // dispatch. The bridge is operator-hosted and could
        // dereference a host-policy-allowed hostname that flips to
        // a blocked IP — the same DNS-rebinding shape `web_fetch`
        // was patched against. The previous shape forwarded the
        // URL to the bridge with no host check, so a steering
        // attempt could turn the bridge into a confused deputy
        // for any host on its network. Validate here.
        if let Some(meeting) = args.meeting.as_deref() {
            crate::security::ssrf::validate_url_async(meeting, &self.ssrf_policy)
                .await
                .map_err(|e| AlephError::tool(format!(
                    "google_meet: meeting URL blocked by SSRF policy: {e}"
                )))?;
        }
        match self.bridge {
            Some(ref bridge) => Self::dispatch_to_bridge(bridge, &args).await,
            None => Ok(Self::not_configured(args.action)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_deserializes_snake_case() {
        let args: GoogleMeetArgs = serde_json::from_value(serde_json::json!({
            "action": "join",
            "meeting": "https://meet.google.com/abc-defg-hij"
        }))
        .unwrap();
        assert_eq!(args.action, GoogleMeetAction::Join);
        assert_eq!(
            args.meeting.as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
        assert!(args.transport.is_none());
    }

    #[test]
    fn transport_uses_kebab_case() {
        let args: GoogleMeetArgs = serde_json::from_value(serde_json::json!({
            "action": "join",
            "meeting": "code",
            "transport": "chrome-node"
        }))
        .unwrap();
        assert_eq!(args.transport, Some(GoogleMeetTransport::ChromeNode));
    }

    #[test]
    fn rpc_method_mapping() {
        assert_eq!(GoogleMeetAction::Join.rpc_method(), "googlemeet.join");
        assert_eq!(GoogleMeetAction::Speak.rpc_method(), "googlemeet.speak");
        assert_eq!(GoogleMeetAction::Status.rpc_method(), "googlemeet.status");
    }

    #[test]
    fn join_requires_meeting() {
        let args = GoogleMeetArgs {
            action: GoogleMeetAction::Join,
            meeting: None,
            transport: None,
            mode: None,
            text: None,
        };
        let err = GoogleMeetTool::validate(&args).unwrap_err();
        assert!(err.to_string().contains("meeting"));
    }

    #[test]
    fn speak_requires_text() {
        let args = GoogleMeetArgs {
            action: GoogleMeetAction::Speak,
            meeting: None,
            transport: None,
            mode: None,
            text: Some("   ".to_string()),
        };
        let err = GoogleMeetTool::validate(&args).unwrap_err();
        assert!(err.to_string().contains("text"));
    }

    #[test]
    fn status_needs_no_fields() {
        let args = GoogleMeetArgs {
            action: GoogleMeetAction::Status,
            meeting: None,
            transport: None,
            mode: None,
            text: None,
        };
        assert!(GoogleMeetTool::validate(&args).is_ok());
    }

    #[tokio::test]
    async fn unconfigured_bridge_returns_actionable_status() {
        let tool = GoogleMeetTool::new(None, SsrfPolicy::default());
        let out = tool
            .call(GoogleMeetArgs {
                action: GoogleMeetAction::Status,
                meeting: None,
                transport: None,
                mode: None,
                text: None,
            })
            .await
            .unwrap();
        assert!(!out.ok);
        assert_eq!(out.status, "bridge_not_configured");
        assert!(out.detail.contains("ALEPH_GOOGLE_MEET_BRIDGE_URL"));
    }

    #[tokio::test]
    async fn validation_runs_before_bridge() {
        // Even with no bridge, a missing required field is a fixable validation
        // error (not a "not configured" status), so the model can correct it.
        let tool = GoogleMeetTool::new(None, SsrfPolicy::default());
        let err = tool
            .call(GoogleMeetArgs {
                action: GoogleMeetAction::Join,
                meeting: None,
                transport: None,
                mode: None,
                text: None,
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("meeting"));
    }

    #[test]
    fn output_omits_meeting_url_when_absent() {
        let out = GoogleMeetTool::not_configured(GoogleMeetAction::Join);
        let v = serde_json::to_value(&out).unwrap();
        assert!(v.get("meeting_url").is_none());
    }

    #[test]
    fn from_env_none_without_url() {
        // No env var set in this test process → bridge disabled.
        // (We avoid mutating process env to keep parallel tests isolated.)
        std::env::remove_var("ALEPH_GOOGLE_MEET_BRIDGE_URL");
        assert!(GoogleMeetBridge::from_env().is_none());
    }
}
