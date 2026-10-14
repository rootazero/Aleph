//! Shared browser-live wire contract.
//!
//! Browser live data has one wire owner. Core services and thin clients must
//! consume these types rather than maintaining JSON-shaped local copies.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const VIEWER_PROTOCOL_VERSION: u16 = 1;
pub const VIEWER_AUTH_FIRST_FRAME_MAX_BYTES: usize = 256;
pub const VIEWER_WS_FRAME_HEADER_MAX_BYTES: usize = 16 * 1024;
pub const VIEWER_WS_FRAME_PAYLOAD_MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationBinding {
    pub session_key: String,
    pub run_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserTargetBinding {
    pub profile: String,
    pub browser_session_id: String,
    pub target_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "identity_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum IssuerIdentity {
    User { user_id: String },
    Connection { connection_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewerBinding {
    pub principal: IssuerIdentity,
    pub task: ConversationBinding,
    pub target: BrowserTargetBinding,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactBinding {
    pub principal: IssuerIdentity,
    pub task: ConversationBinding,
    pub artifact_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "binding_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceBinding {
    Browser { viewer: ViewerBinding },
    Artifact { artifact: ArtifactBinding },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PreviewKind {
    Page,
    DevPreview,
    DesignCompanion,
    Artifact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DisplayPolicy {
    AutoRevealCurrentRun,
    ExplicitOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PreviewSource {
    BrowserTool,
    ManagedTool,
    ArtifactAdapter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RedactionStatus {
    Pending,
    Safe,
    Redacted,
    Refused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompanionArrangement {
    Single,
    SideBySide,
    HorizontalScreens,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewportCssPx {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompanionScreen {
    pub id: String,
    pub label: String,
    pub viewport_css_px: ViewportCssPx,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnimationPreview {
    pub name: String,
    pub duration_ms: u64,
    pub looped: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompanionStateHint {
    pub active_screen_id: Option<String>,
    pub animation: Option<AnimationPreview>,
    pub prefers_reduced_motion: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompanionLayout {
    pub arrangement: CompanionArrangement,
    pub screens: Vec<CompanionScreen>,
}

/// A state/layout candidate inside one resource revision.
///
/// It deliberately has no resource, browser-session, or target identifier: a
/// resolver can only apply it to the containing resource's existing binding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompanionCandidate {
    pub id: String,
    pub label: String,
    pub layout: CompanionLayout,
    pub state: CompanionStateHint,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "provenance_type",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PreviewProvenance {
    NotManaged,
    Managed {
        launcher: String,
        process_id: u32,
        launched_at_ms: i64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewResource {
    pub resource_id: String,
    pub revision: u64,
    pub binding: ResourceBinding,
    pub kind: PreviewKind,
    pub display: DisplayPolicy,
    pub source: PreviewSource,
    pub provenance: PreviewProvenance,
    pub expires_at_ms: i64,
    pub redaction: RedactionStatus,
    pub companion_candidates: Vec<CompanionCandidate>,
}

impl PreviewResource {
    /// Validate the binding/kind invariant before publishing the resource.
    pub fn validate(&self) -> Result<(), BrowserLiveValidationError> {
        let is_artifact_kind = self.kind == PreviewKind::Artifact;
        let is_artifact_binding = matches!(self.binding, ResourceBinding::Artifact { .. });
        if is_artifact_kind != is_artifact_binding {
            return Err(BrowserLiveValidationError::ResourceKindBindingMismatch);
        }
        Ok(())
    }

    #[must_use]
    pub fn viewer_binding(&self) -> Option<&ViewerBinding> {
        match &self.binding {
            ResourceBinding::Browser { viewer } => Some(viewer),
            ResourceBinding::Artifact { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "controller_type",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Controller {
    Agent,
    Human {
        principal: IssuerIdentity,
        viewer_session_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserLiveSnapshot {
    pub resource: PreviewResource,
    pub connection_epoch: u64,
    pub stream_epoch: u64,
    pub last_frame_seq: u64,
    pub viewport_css_px: ViewportCssPx,
    pub controller: Controller,
    pub control_epoch: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewerOrigin {
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PointerButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum HumanInputEvent {
    PointerMove {
        x_css_px: f64,
        y_css_px: f64,
        modifiers: u8,
    },
    PointerDown {
        x_css_px: f64,
        y_css_px: f64,
        button: PointerButton,
        click_count: u8,
        modifiers: u8,
    },
    PointerUp {
        x_css_px: f64,
        y_css_px: f64,
        button: PointerButton,
        click_count: u8,
        modifiers: u8,
    },
    Wheel {
        x_css_px: f64,
        y_css_px: f64,
        delta_x: f64,
        delta_y: f64,
        modifiers: u8,
    },
    KeyDown {
        key: String,
        code: String,
        text: Option<String>,
        modifiers: u8,
    },
    KeyUp {
        key: String,
        code: String,
        modifiers: u8,
    },
    Text {
        text: String,
    },
    Navigate {
        url: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanInputRequest {
    pub connection_epoch: u64,
    pub stream_epoch: u64,
    pub control_epoch: u64,
    pub seq: u64,
    pub expires_at_ms: i64,
    pub event: HumanInputEvent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachRequest {
    pub last_frame_seq: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControlRequest {
    pub expected_control_epoch: u64,
    pub action_seq: u64,
    pub expires_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ViewerClientMessage {
    Attach { request: AttachRequest },
    Take { request: ControlRequest },
    Cede { request: ControlRequest },
    Input { request: HumanInputRequest },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputRejectReason {
    StaleConnection,
    StaleStream,
    StaleControl,
    Expired,
    ReplayedSeq,
    NotController,
    Revoked,
    PermissionDenied,
    DispatchBusy,
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanInputResult {
    pub seq: u64,
    pub accepted: bool,
    pub reason: Option<InputRejectReason>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControlResult {
    pub action_seq: u64,
    pub accepted: bool,
    pub control_epoch: u64,
    pub reason: Option<InputRejectReason>,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ViewerPermission {
    Frames,
    HumanInput,
    Navigate,
}

fn serialize_permissions<S>(
    permissions: &[ViewerPermission],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let mut canonical = permissions.to_vec();
    canonical.sort_unstable();
    if canonical.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(<S::Error as serde::ser::Error>::custom(
            "duplicate viewer permission",
        ));
    }
    canonical.serialize(serializer)
}

fn deserialize_permissions<'de, D>(deserializer: D) -> Result<Vec<ViewerPermission>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let permissions = Vec::<ViewerPermission>::deserialize(deserializer)?;
    if permissions.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(<D::Error as serde::de::Error>::custom(
            "viewer permissions must be unique and ordered frames, human_input, navigate",
        ));
    }
    Ok(permissions)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueViewerGrantRequest {
    pub resource_id: String,
    #[serde(
        serialize_with = "serialize_permissions",
        deserialize_with = "deserialize_permissions"
    )]
    pub permissions: Vec<ViewerPermission>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueViewerGrantResponse {
    pub launch_url: String,
    pub grant_id: String,
    pub expires_at_ms: i64,
}

impl fmt::Debug for IssueViewerGrantResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IssueViewerGrantResponse")
            .field("launch_url", &"[REDACTED]")
            .field("grant_id", &self.grant_id)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewerGrantInfo {
    pub grant_id: String,
    pub issuer: IssuerIdentity,
    pub binding: ViewerBinding,
    pub resource_id: String,
    #[serde(
        serialize_with = "serialize_permissions",
        deserialize_with = "deserialize_permissions"
    )]
    pub permissions: Vec<ViewerPermission>,
    pub origin: ViewerOrigin,
    pub expires_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeViewerGrantRequest {
    pub grant_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeViewerGrantResponse {
    pub revoked: bool,
}

fn deserialize_nonzero_revision<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let revision = u64::deserialize(deserializer)?;
    if revision == 0 {
        // Revisions start at 1: 0 can never name a published revision.
        return Err(serde::de::Error::custom("expected_revision must be >= 1"));
    }
    Ok(revision)
}

/// The user's explicit verdict on one exact resource revision.
///
/// `Safe` is a declaration by the authenticated user that the revision may be
/// displayed. It is NOT an automatic content inspection, NOT automatic
/// redaction, and never an LLM judgement. `Refused` closes the resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResourceDecision {
    Safe,
    Refused,
}

/// `browser.viewer.resource.decide` params. The caller identity comes from the
/// authenticated RPC; binding, user, target and expiry are deliberately absent
/// and rejected (`deny_unknown_fields`). `expected_revision` must be the exact
/// current revision (>= 1); a mismatch is denied like any other refusal.
///
/// The success response reuses the existing bus shapes: [`BusEvent::Resource`]
/// (decision `safe`, new revision, unchanged absolute expiry) or
/// [`BusEvent::Closed`] (decision `refused`). No separate wire shape exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecideResourceRequest {
    pub resource_id: String,
    #[serde(deserialize_with = "deserialize_nonzero_revision")]
    pub expected_revision: u64,
    pub decision: ResourceDecision,
}

/// `browser.viewer.resource.list` params: deliberately EMPTY. The caller
/// identity comes from the handshake-verified RPC user; a filter (user,
/// session, target, URL, ...) supplied by the client is rejected
/// (`deny_unknown_fields`) so the server never lists by a client-chosen key.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListResourcesRequest {}

/// `browser.viewer.resource.list` result: the server registry's exact current
/// view of the caller's own live, unexpired resources (Safe and Pending; a
/// Refused resource is never live). Same [`PreviewResource`] shape as the
/// `browser.live.resource` bus event, so a client applies both through one
/// acceptance path. Ordered by `resource_id` (opaque, deterministic).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListResourcesResponse {
    pub resources: Vec<PreviewResource>,
}

macro_rules! secret_wrapper {
    ($name:ident) => {
        #[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    };
}

secret_wrapper!(FragmentNonce);
secret_wrapper!(ExchangeTicket);
secret_wrapper!(ViewerSecret);

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsumeFragmentNonceRequest {
    pub grant_id: String,
    pub nonce: FragmentNonce,
}

impl fmt::Debug for ConsumeFragmentNonceRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConsumeFragmentNonceRequest")
            .field("grant_id", &self.grant_id)
            .field("nonce", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsumeFragmentNonceResponse {
    pub ticket: ExchangeTicket,
    pub expires_at_ms: i64,
}

impl fmt::Debug for ConsumeFragmentNonceResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConsumeFragmentNonceResponse")
            .field("ticket", &"[REDACTED]")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExchangeViewerGrantRequest {
    pub ticket: ExchangeTicket,
}

impl fmt::Debug for ExchangeViewerGrantRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExchangeViewerGrantRequest")
            .field("ticket", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExchangeViewerGrantResponse {
    pub viewer_session_id: String,
    pub grant_id: String,
    pub rotation_generation: u64,
    pub current_secret: ViewerSecret,
    pub ws_path: String,
    pub expires_at_ms: i64,
    pub snapshot: BrowserLiveSnapshot,
}

impl fmt::Debug for ExchangeViewerGrantResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExchangeViewerGrantResponse")
            .field("viewer_session_id", &self.viewer_session_id)
            .field("grant_id", &self.grant_id)
            .field("rotation_generation", &self.rotation_generation)
            .field("current_secret", &"[REDACTED]")
            .field("ws_path", &self.ws_path)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("snapshot", &self.snapshot)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthHello {
    pub protocol_version: u16,
    pub grant_id: String,
    pub rotation_generation: u64,
    pub secret: String,
}

impl AuthHello {
    pub fn validate_shape(&self) -> Result<(), BrowserLiveValidationError> {
        if self.grant_id.len() > 64 || !self.grant_id.is_ascii() {
            return Err(BrowserLiveValidationError::InvalidGrantId);
        }
        if self.secret.len() != 43
            || !self
                .secret
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(BrowserLiveValidationError::InvalidSecretShape);
        }
        Ok(())
    }
}

impl fmt::Debug for AuthHello {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthHello")
            .field("protocol_version", &self.protocol_version)
            .field("grant_id", &self.grant_id)
            .field("rotation_generation", &self.rotation_generation)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ViewerAuthClientMessage {
    AuthHello { hello: AuthHello },
}

impl fmt::Debug for ViewerAuthClientMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AuthHello { hello } => formatter
                .debug_struct("AuthHello")
                .field("hello", hello)
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthOk {
    pub protocol_version: u16,
    pub ws_generation: u64,
    pub next_secret: String,
}

impl fmt::Debug for AuthOk {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthOk")
            .field("protocol_version", &self.protocol_version)
            .field("ws_generation", &self.ws_generation)
            .field("next_secret", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthErrorCode {
    Unauthorized,
    Conflict,
    Malformed,
    UnsupportedVersion,
    Timeout,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthError {
    pub code: AuthErrorCode,
    pub close: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ViewerAuthServerMessage {
    AuthOk { ok: AuthOk },
    AuthError { error: AuthError },
}

impl fmt::Debug for ViewerAuthServerMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AuthOk { ok } => formatter.debug_struct("AuthOk").field("ok", ok).finish(),
            Self::AuthError { error } => formatter
                .debug_struct("AuthError")
                .field("error", error)
                .finish(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewerAuthDecodeError {
    Empty,
    Oversize,
    Malformed(String),
}

impl fmt::Display for ViewerAuthDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("empty viewer auth frame"),
            Self::Oversize => formatter.write_str("viewer auth frame exceeds 256 bytes"),
            Self::Malformed(error) => write!(formatter, "malformed viewer auth frame: {error}"),
        }
    }
}

impl std::error::Error for ViewerAuthDecodeError {}

/// Decode the one permitted pre-auth text payload.
///
/// Transport code remains responsible for refusing binary frames and for the
/// one-message/2-second state machine; this function owns the byte cap and
/// strict serde shape.
pub fn decode_viewer_auth_client_message(
    bytes: &[u8],
) -> Result<ViewerAuthClientMessage, ViewerAuthDecodeError> {
    if bytes.is_empty() {
        return Err(ViewerAuthDecodeError::Empty);
    }
    if bytes.len() > VIEWER_AUTH_FIRST_FRAME_MAX_BYTES {
        return Err(ViewerAuthDecodeError::Oversize);
    }
    serde_json::from_slice(bytes)
        .map_err(|error| ViewerAuthDecodeError::Malformed(error.to_string()))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewerWsFrameHeader {
    pub version: u8,
    pub resource_id: String,
    pub binding: ViewerBinding,
    pub connection_epoch: u64,
    pub stream_epoch: u64,
    pub seq: u64,
    pub captured_at_ms: i64,
    pub viewport_css_px: ViewportCssPx,
    pub content_type: String,
}

/// One binary viewer message. The JPEG bytes deliberately have no serde form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewerWsFrame {
    pub header: ViewerWsFrameHeader,
    pub jpeg_payload: Vec<u8>,
}

impl ViewerWsFrame {
    pub fn encode(&self) -> Result<Vec<u8>, ViewerWsFrameCodecError> {
        validate_frame_header(&self.header)?;
        if self.jpeg_payload.is_empty() {
            return Err(ViewerWsFrameCodecError::ZeroPayload);
        }
        if self.jpeg_payload.len() > VIEWER_WS_FRAME_PAYLOAD_MAX_BYTES {
            return Err(ViewerWsFrameCodecError::PayloadTooLarge);
        }
        let header = serde_json::to_vec(&self.header)
            .map_err(|error| ViewerWsFrameCodecError::InvalidHeaderJson(error.to_string()))?;
        if header.is_empty() {
            return Err(ViewerWsFrameCodecError::ZeroHeader);
        }
        if header.len() > VIEWER_WS_FRAME_HEADER_MAX_BYTES {
            return Err(ViewerWsFrameCodecError::HeaderTooLarge);
        }
        let total = 8usize
            .checked_add(header.len())
            .and_then(|length| length.checked_add(self.jpeg_payload.len()))
            .ok_or(ViewerWsFrameCodecError::LengthOverflow)?;
        let header_len =
            u32::try_from(header.len()).map_err(|_| ViewerWsFrameCodecError::LengthOverflow)?;
        let payload_len = u32::try_from(self.jpeg_payload.len())
            .map_err(|_| ViewerWsFrameCodecError::LengthOverflow)?;
        let mut encoded = Vec::with_capacity(total);
        encoded.extend_from_slice(&header_len.to_be_bytes());
        encoded.extend_from_slice(&payload_len.to_be_bytes());
        encoded.extend_from_slice(&header);
        encoded.extend_from_slice(&self.jpeg_payload);
        Ok(encoded)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ViewerWsFrameCodecError> {
        if bytes.len() < 8 {
            return Err(ViewerWsFrameCodecError::ShortPrefix);
        }
        let header_len = u32::from_be_bytes(
            bytes[0..4]
                .try_into()
                .map_err(|_| ViewerWsFrameCodecError::ShortPrefix)?,
        ) as usize;
        let payload_len = u32::from_be_bytes(
            bytes[4..8]
                .try_into()
                .map_err(|_| ViewerWsFrameCodecError::ShortPrefix)?,
        ) as usize;
        if header_len == 0 {
            return Err(ViewerWsFrameCodecError::ZeroHeader);
        }
        if payload_len == 0 {
            return Err(ViewerWsFrameCodecError::ZeroPayload);
        }
        if header_len > VIEWER_WS_FRAME_HEADER_MAX_BYTES {
            return Err(ViewerWsFrameCodecError::HeaderTooLarge);
        }
        if payload_len > VIEWER_WS_FRAME_PAYLOAD_MAX_BYTES {
            return Err(ViewerWsFrameCodecError::PayloadTooLarge);
        }
        let expected_len = 8usize
            .checked_add(header_len)
            .and_then(|length| length.checked_add(payload_len))
            .ok_or(ViewerWsFrameCodecError::LengthOverflow)?;
        if bytes.len() != expected_len {
            return Err(ViewerWsFrameCodecError::LengthMismatch {
                declared: expected_len,
                actual: bytes.len(),
            });
        }
        let header_end = 8 + header_len;
        let header: ViewerWsFrameHeader = serde_json::from_slice(&bytes[8..header_end])
            .map_err(|error| ViewerWsFrameCodecError::InvalidHeaderJson(error.to_string()))?;
        validate_frame_header(&header)?;
        Ok(Self {
            header,
            jpeg_payload: bytes[header_end..].to_vec(),
        })
    }

    pub fn decode_for(
        bytes: &[u8],
        resource_id: &str,
        binding: &ViewerBinding,
        connection_epoch: u64,
        stream_epoch: u64,
    ) -> Result<Self, ViewerWsFrameCodecError> {
        let frame = Self::decode(bytes)?;
        if frame.header.resource_id != resource_id
            || &frame.header.binding != binding
            || frame.header.connection_epoch != connection_epoch
            || frame.header.stream_epoch != stream_epoch
        {
            return Err(ViewerWsFrameCodecError::AttachedScopeMismatch);
        }
        Ok(frame)
    }
}

fn validate_frame_header(header: &ViewerWsFrameHeader) -> Result<(), ViewerWsFrameCodecError> {
    if header.content_type != "image/jpeg" {
        return Err(ViewerWsFrameCodecError::WrongContentType);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewerWsFrameCodecError {
    ShortPrefix,
    ZeroHeader,
    ZeroPayload,
    HeaderTooLarge,
    PayloadTooLarge,
    LengthOverflow,
    LengthMismatch { declared: usize, actual: usize },
    InvalidHeaderJson(String),
    WrongContentType,
    AttachedScopeMismatch,
}

impl fmt::Display for ViewerWsFrameCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ShortPrefix => formatter.write_str("viewer frame is shorter than its prefix"),
            Self::ZeroHeader => formatter.write_str("viewer frame header is empty"),
            Self::ZeroPayload => formatter.write_str("viewer frame payload is empty"),
            Self::HeaderTooLarge => formatter.write_str("viewer frame header exceeds 16 KiB"),
            Self::PayloadTooLarge => formatter.write_str("viewer frame payload exceeds 8 MiB"),
            Self::LengthOverflow => formatter.write_str("viewer frame length overflows"),
            Self::LengthMismatch { declared, actual } => write!(
                formatter,
                "viewer frame length mismatch: declared {declared}, actual {actual}"
            ),
            Self::InvalidHeaderJson(error) => {
                write!(formatter, "invalid frame header JSON: {error}")
            }
            Self::WrongContentType => formatter.write_str("viewer frame is not image/jpeg"),
            Self::AttachedScopeMismatch => {
                formatter.write_str("viewer frame does not match attachment")
            }
        }
    }
}

impl std::error::Error for ViewerWsFrameCodecError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserLiveState {
    pub resource_id: String,
    pub binding: ViewerBinding,
    pub connection_epoch: u64,
    pub stream_epoch: u64,
    pub viewport_css_px: ViewportCssPx,
    pub controller: Controller,
    pub control_epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClosedReason {
    TargetClosed,
    EngineExited,
    ServiceExited,
    RunEnded,
    Revoked,
    Expired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserLiveClosed {
    pub resource_id: String,
    pub binding: ResourceBinding,
    pub reason: ClosedReason,
    pub at_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InterventionEnd {
    Cede,
    Revoked,
    Expired,
    TargetClosed,
    EngineExited,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InterventionFacts {
    pub input_event_count: u64,
    pub typed_text_chars: u64,
    pub navigation_count: u64,
    pub final_origin: Option<ViewerOrigin>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InterventionSummary {
    pub intervention_id: String,
    pub destination: ConversationBinding,
    pub target: BrowserTargetBinding,
    pub principal: IssuerIdentity,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    pub control_epoch_before: u64,
    pub control_epoch_after: u64,
    pub ended_by: InterventionEnd,
    pub facts: InterventionFacts,
}

/// Gateway event topic carrying a [`PreviewResource`] (`BusEvent::Resource`).
pub const RESOURCE_TOPIC: &str = "browser.live.resource";
/// Gateway event topic carrying a [`BrowserLiveClosed`] (`BusEvent::Closed`).
pub const CLOSED_TOPIC: &str = "browser.live.closed";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum BusEvent {
    #[serde(rename = "browser.live.resource")]
    Resource { resource: PreviewResource },
    #[serde(rename = "browser.live.closed")]
    Closed { closed: BrowserLiveClosed },
    #[serde(rename = "browser.live.intervention")]
    Intervention { summary: InterventionSummary },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ViewerServerMessage {
    Snapshot { snapshot: BrowserLiveSnapshot },
    State { state: BrowserLiveState },
    InputResult { result: HumanInputResult },
    ControlResult { result: ControlResult },
    Closed { closed: BrowserLiveClosed },
    Error { code: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserLiveValidationError {
    ResourceKindBindingMismatch,
    InvalidGrantId,
    InvalidSecretShape,
}

impl fmt::Display for BrowserLiveValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResourceKindBindingMismatch => {
                formatter.write_str("preview kind and resource binding disagree")
            }
            Self::InvalidGrantId => formatter.write_str("grant id must be at most 64 ASCII bytes"),
            Self::InvalidSecretShape => {
                formatter.write_str("viewer secret must be 43 base64url bytes")
            }
        }
    }
}

impl std::error::Error for BrowserLiveValidationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::{json, Value};

    fn task() -> ConversationBinding {
        ConversationBinding {
            session_key: "agent:operator:browser".to_string(),
            run_id: "run-7".to_string(),
        }
    }

    fn target() -> BrowserTargetBinding {
        BrowserTargetBinding {
            profile: "default".to_string(),
            browser_session_id: "browser-session-3".to_string(),
            target_id: "target-9".to_string(),
        }
    }

    fn viewer_binding() -> ViewerBinding {
        ViewerBinding {
            principal: IssuerIdentity::User {
                user_id: "user-42".to_string(),
            },
            task: task(),
            target: target(),
        }
    }

    fn candidate() -> CompanionCandidate {
        CompanionCandidate {
            id: "state-mobile".to_string(),
            label: "Mobile".to_string(),
            layout: CompanionLayout {
                arrangement: CompanionArrangement::Single,
                screens: vec![CompanionScreen {
                    id: "screen-main".to_string(),
                    label: "Main".to_string(),
                    viewport_css_px: ViewportCssPx {
                        width: 390,
                        height: 844,
                    },
                }],
            },
            state: CompanionStateHint {
                active_screen_id: Some("screen-main".to_string()),
                animation: Some(AnimationPreview {
                    name: "pulse".to_string(),
                    duration_ms: 250,
                    looped: true,
                }),
                prefers_reduced_motion: false,
            },
        }
    }

    fn resource() -> PreviewResource {
        PreviewResource {
            resource_id: "resource-5".to_string(),
            revision: 2,
            binding: ResourceBinding::Browser {
                viewer: viewer_binding(),
            },
            kind: PreviewKind::Page,
            display: DisplayPolicy::AutoRevealCurrentRun,
            source: PreviewSource::BrowserTool,
            provenance: PreviewProvenance::Managed {
                launcher: "browser.open".to_string(),
                process_id: 17,
                launched_at_ms: 1_799_000_000_000,
            },
            expires_at_ms: 1_799_000_060_000,
            redaction: RedactionStatus::Safe,
            companion_candidates: vec![candidate()],
        }
    }

    fn snapshot() -> BrowserLiveSnapshot {
        BrowserLiveSnapshot {
            resource: resource(),
            connection_epoch: 11,
            stream_epoch: 13,
            last_frame_seq: 17,
            viewport_css_px: ViewportCssPx {
                width: 1280,
                height: 720,
            },
            controller: Controller::Agent,
            control_epoch: 19,
        }
    }

    fn round_trip<T>(value: &T)
    where
        T: Serialize + DeserializeOwned + PartialEq + fmt::Debug,
    {
        let encoded = serde_json::to_vec(value).expect("serialize wire value");
        let decoded = serde_json::from_slice::<T>(&encoded).expect("deserialize wire value");
        assert_eq!(&decoded, value);
    }

    #[test]
    fn viewer_binding_round_trip_preserves_every_axis() {
        let original = ViewerGrantInfo {
            grant_id: "grant-public".to_string(),
            issuer: IssuerIdentity::Connection {
                connection_id: "connection-8".to_string(),
            },
            binding: viewer_binding(),
            resource_id: "resource-5".to_string(),
            permissions: vec![ViewerPermission::Frames, ViewerPermission::HumanInput],
            origin: ViewerOrigin {
                scheme: "https".to_string(),
                host: "example.test".to_string(),
                port: 443,
            },
            expires_at_ms: 1_799_000_060_000,
        };
        let wire = serde_json::to_value(&original).unwrap();
        let decoded: ViewerGrantInfo = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(decoded.binding, original.binding);
        assert_ne!(decoded.issuer, decoded.binding.principal);
        assert_eq!(wire["issuer"]["identity_type"], "connection");
        assert_eq!(wire["binding"]["principal"]["identity_type"], "user");

        for field in [
            "grant_id",
            "issuer",
            "binding",
            "resource_id",
            "permissions",
            "origin",
            "expires_at_ms",
        ] {
            let mut missing_axis = wire.clone();
            missing_axis.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<ViewerGrantInfo>(missing_axis).is_err(),
                "missing root binding axis {field} was accepted"
            );
        }
        for (object_pointer, field) in [
            ("/issuer", "connection_id"),
            ("/binding", "principal"),
            ("/binding", "task"),
            ("/binding", "target"),
            ("/binding/principal", "user_id"),
            ("/binding/task", "session_key"),
            ("/binding/task", "run_id"),
            ("/binding/target", "profile"),
            ("/binding/target", "browser_session_id"),
            ("/binding/target", "target_id"),
            ("/origin", "scheme"),
            ("/origin", "host"),
            ("/origin", "port"),
        ] {
            let mut missing_axis = wire.clone();
            missing_axis
                .pointer_mut(object_pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(
                serde_json::from_value::<ViewerGrantInfo>(missing_axis).is_err(),
                "missing binding axis {object_pointer}/{field} was accepted"
            );
        }

        let mut unknown_axis = wire.clone();
        unknown_axis["binding"]["target"]["context_id"] = json!("forbidden");
        assert!(serde_json::from_value::<ViewerGrantInfo>(unknown_axis).is_err());

        let collision = json!({"identity_type":"user","connection_id":"connection-8"});
        assert!(serde_json::from_value::<IssuerIdentity>(collision).is_err());
    }

    #[test]
    fn schema_has_epoch_and_expiry() {
        let schemas = [
            serde_json::to_value(schemars::schema_for!(BrowserLiveSnapshot)).unwrap(),
            serde_json::to_value(schemars::schema_for!(HumanInputRequest)).unwrap(),
            serde_json::to_value(schemars::schema_for!(ControlRequest)).unwrap(),
            serde_json::to_value(schemars::schema_for!(ViewerGrantInfo)).unwrap(),
            serde_json::to_value(schemars::schema_for!(ViewerWsFrameHeader)).unwrap(),
            serde_json::to_value(schemars::schema_for!(BrowserLiveState)).unwrap(),
        ];
        let rendered = schemas
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        for field in [
            "connection_epoch",
            "stream_epoch",
            "control_epoch",
            "last_frame_seq",
            "action_seq",
            "expires_at_ms",
            "seq",
        ] {
            assert!(rendered.contains(field), "schema omitted {field}");
        }
    }

    #[test]
    fn auth_wire_is_bounded_strict_and_redacted() {
        let hello = ViewerAuthClientMessage::AuthHello {
            hello: AuthHello {
                protocol_version: VIEWER_PROTOCOL_VERSION,
                grant_id: "g".repeat(64),
                rotation_generation: u64::MAX,
                secret: "A".repeat(43),
            },
        };
        let encoded = serde_json::to_vec(&hello).unwrap();
        assert!(encoded.len() < VIEWER_AUTH_FIRST_FRAME_MAX_BYTES);
        assert_eq!(decode_viewer_auth_client_message(&encoded).unwrap(), hello);
        assert!(!format!("{hello:?}").contains(&"A".repeat(43)));
        let ViewerAuthClientMessage::AuthHello { hello: shape } = &hello;
        assert!(shape.validate_shape().is_ok());
        for invalid in [
            AuthHello {
                protocol_version: VIEWER_PROTOCOL_VERSION,
                grant_id: "g".repeat(65),
                rotation_generation: 0,
                secret: "A".repeat(43),
            },
            AuthHello {
                protocol_version: VIEWER_PROTOCOL_VERSION,
                grant_id: "not-ascii-å".to_string(),
                rotation_generation: 0,
                secret: "A".repeat(43),
            },
            AuthHello {
                protocol_version: VIEWER_PROTOCOL_VERSION,
                grant_id: "grant-public".to_string(),
                rotation_generation: 0,
                secret: "A".repeat(42),
            },
            AuthHello {
                protocol_version: VIEWER_PROTOCOL_VERSION,
                grant_id: "grant-public".to_string(),
                rotation_generation: 0,
                secret: format!("{}+", "A".repeat(42)),
            },
        ] {
            assert!(invalid.validate_shape().is_err());
        }

        let mut unknown = serde_json::to_value(&hello).unwrap();
        unknown["hello"]["cookie"] = json!("secret");
        assert!(serde_json::from_value::<ViewerAuthClientMessage>(unknown).is_err());
        assert!(decode_viewer_auth_client_message(&[0xff, 0xfe]).is_err());
        assert!(decode_viewer_auth_client_message(&vec![b'x'; 257]).is_err());
        let duplicate = [encoded.as_slice(), encoded.as_slice()].concat();
        assert!(decode_viewer_auth_client_message(&duplicate).is_err());
        assert!(serde_json::from_value::<ViewerAuthClientMessage>(json!({
            "type": "attach",
            "request": {"last_frame_seq": null}
        }))
        .is_err());

        let ok = ViewerAuthServerMessage::AuthOk {
            ok: AuthOk {
                protocol_version: VIEWER_PROTOCOL_VERSION,
                ws_generation: 1,
                next_secret: "B".repeat(43),
            },
        };
        assert!(!format!("{ok:?}").contains(&"B".repeat(43)));
        round_trip(&ok);
        round_trip(&ViewerAuthServerMessage::AuthError {
            error: AuthError {
                code: AuthErrorCode::Timeout,
                close: true,
            },
        });
    }

    #[test]
    fn permissions_and_resource_binding_fail_closed() {
        let request = IssueViewerGrantRequest {
            resource_id: "resource-5".to_string(),
            permissions: vec![ViewerPermission::Navigate, ViewerPermission::Frames],
        };
        let wire = serde_json::to_value(&request).unwrap();
        assert_eq!(wire["permissions"], json!(["frames", "navigate"]));
        assert!(serde_json::from_value::<IssueViewerGrantRequest>(json!({
            "resource_id": "resource-5",
            "permissions": ["frames", "frames"]
        }))
        .is_err());
        assert!(serde_json::from_value::<IssueViewerGrantRequest>(json!({
            "resource_id": "resource-5",
            "permissions": ["navigate", "frames"]
        }))
        .is_err());

        let mut page = resource();
        assert!(page.validate().is_ok());
        page.binding = ResourceBinding::Artifact {
            artifact: ArtifactBinding {
                principal: IssuerIdentity::User {
                    user_id: "user-42".to_string(),
                },
                task: task(),
                artifact_id: "artifact-1".to_string(),
            },
        };
        assert_eq!(
            page.validate(),
            Err(BrowserLiveValidationError::ResourceKindBindingMismatch)
        );
        page.kind = PreviewKind::Artifact;
        assert!(page.validate().is_ok());
        assert!(page.viewer_binding().is_none());
    }

    #[test]
    fn companion_and_bus_shapes_cannot_cross_transport_boundaries() {
        let candidate_wire = serde_json::to_value(candidate()).unwrap();
        let object = candidate_wire.as_object().unwrap();
        for forbidden in ["resource_id", "target_id", "browser_session_id"] {
            assert!(!object.contains_key(forbidden));
        }
        let mut injected = candidate_wire;
        injected["target_id"] = json!("other-target");
        assert!(serde_json::from_value::<CompanionCandidate>(injected).is_err());

        let events = [
            serde_json::to_value(BusEvent::Resource {
                resource: resource(),
            })
            .unwrap(),
            serde_json::to_value(BusEvent::Closed {
                closed: BrowserLiveClosed {
                    resource_id: "resource-5".to_string(),
                    binding: ResourceBinding::Browser {
                        viewer: viewer_binding(),
                    },
                    reason: ClosedReason::TargetClosed,
                    at_ms: 1_799_000_000_000,
                },
            })
            .unwrap(),
            serde_json::to_value(BusEvent::Intervention {
                summary: InterventionSummary {
                    intervention_id: "intervention-1".to_string(),
                    destination: task(),
                    target: target(),
                    principal: IssuerIdentity::User {
                        user_id: "user-42".to_string(),
                    },
                    started_at_ms: 1_799_000_000_000,
                    ended_at_ms: 1_799_000_001_000,
                    control_epoch_before: 1,
                    control_epoch_after: 2,
                    ended_by: InterventionEnd::Cede,
                    facts: InterventionFacts {
                        input_event_count: 3,
                        typed_text_chars: 2,
                        navigation_count: 0,
                        final_origin: None,
                    },
                },
            })
            .unwrap(),
        ];
        assert_eq!(
            events.map(|event| event["type"].as_str().unwrap().to_string()),
            [
                "browser.live.resource",
                "browser.live.closed",
                "browser.live.intervention"
            ]
        );
        // The topic constants are the same literals, pinned independently.
        assert_eq!(RESOURCE_TOPIC, "browser.live.resource");
        assert_eq!(CLOSED_TOPIC, "browser.live.closed");
        for forbidden in ["browser.live.frame", "browser.live.state"] {
            assert!(serde_json::from_value::<BusEvent>(json!({"type": forbidden})).is_err());
        }
    }

    #[test]
    fn frame_codec_round_trip_and_bounds_are_exact() {
        let frame = ViewerWsFrame {
            header: ViewerWsFrameHeader {
                version: 1,
                resource_id: "resource-5".to_string(),
                binding: viewer_binding(),
                connection_epoch: 11,
                stream_epoch: 13,
                seq: 1,
                captured_at_ms: 1_799_000_000_000,
                viewport_css_px: ViewportCssPx {
                    width: 1280,
                    height: 720,
                },
                content_type: "image/jpeg".to_string(),
            },
            jpeg_payload: vec![0xff, 0xd8, 0xff, 0xd9],
        };
        let encoded = frame.encode().unwrap();
        assert_eq!(ViewerWsFrame::decode(&encoded).unwrap(), frame);
        assert_eq!(
            ViewerWsFrame::decode_for(&encoded, "resource-5", &viewer_binding(), 11, 13).unwrap(),
            frame
        );
        assert_eq!(
            ViewerWsFrame::decode_for(&encoded, "resource-other", &viewer_binding(), 11, 13),
            Err(ViewerWsFrameCodecError::AttachedScopeMismatch)
        );

        assert_eq!(
            ViewerWsFrame::decode(&encoded[..7]),
            Err(ViewerWsFrameCodecError::ShortPrefix)
        );
        let mut zero_header = encoded.clone();
        zero_header[0..4].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(
            ViewerWsFrame::decode(&zero_header),
            Err(ViewerWsFrameCodecError::ZeroHeader)
        );
        let mut zero_payload = encoded.clone();
        zero_payload[4..8].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(
            ViewerWsFrame::decode(&zero_payload),
            Err(ViewerWsFrameCodecError::ZeroPayload)
        );
        let mut huge_header = encoded.clone();
        huge_header[0..4].copy_from_slice(&((16 * 1024 + 1) as u32).to_be_bytes());
        assert_eq!(
            ViewerWsFrame::decode(&huge_header),
            Err(ViewerWsFrameCodecError::HeaderTooLarge)
        );
        let mut huge_payload = encoded.clone();
        huge_payload[4..8].copy_from_slice(&((8 * 1024 * 1024 + 1) as u32).to_be_bytes());
        assert_eq!(
            ViewerWsFrame::decode(&huge_payload),
            Err(ViewerWsFrameCodecError::PayloadTooLarge)
        );
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(matches!(
            ViewerWsFrame::decode(&trailing),
            Err(ViewerWsFrameCodecError::LengthMismatch { .. })
        ));
        assert!(matches!(
            ViewerWsFrame::decode(&encoded[..encoded.len() - 1]),
            Err(ViewerWsFrameCodecError::LengthMismatch { .. })
        ));

        let header_len = u32::from_be_bytes(encoded[0..4].try_into().unwrap()) as usize;
        let mut non_eof_json = Vec::new();
        let invalid_header = [encoded[8..8 + header_len].to_vec(), b"{}".to_vec()].concat();
        non_eof_json.extend_from_slice(&(invalid_header.len() as u32).to_be_bytes());
        non_eof_json.extend_from_slice(&4u32.to_be_bytes());
        non_eof_json.extend_from_slice(&invalid_header);
        non_eof_json.extend_from_slice(&frame.jpeg_payload);
        assert!(matches!(
            ViewerWsFrame::decode(&non_eof_json),
            Err(ViewerWsFrameCodecError::InvalidHeaderJson(_))
        ));

        let mut wrong_content_type = frame.clone();
        wrong_content_type.header.content_type = "image/png".to_string();
        assert_eq!(
            wrong_content_type.encode(),
            Err(ViewerWsFrameCodecError::WrongContentType)
        );

        let mut max_header = frame.clone();
        max_header.header.resource_id.clear();
        let base_header_len = serde_json::to_vec(&max_header.header).unwrap().len();
        max_header.header.resource_id =
            "r".repeat(VIEWER_WS_FRAME_HEADER_MAX_BYTES - base_header_len);
        let max_header_encoded = max_header.encode().unwrap();
        assert_eq!(
            u32::from_be_bytes(max_header_encoded[0..4].try_into().unwrap()) as usize,
            VIEWER_WS_FRAME_HEADER_MAX_BYTES
        );
        assert_eq!(
            ViewerWsFrame::decode(&max_header_encoded).unwrap(),
            max_header
        );
        max_header.header.resource_id.push('r');
        assert_eq!(
            max_header.encode(),
            Err(ViewerWsFrameCodecError::HeaderTooLarge)
        );

        let mut max_payload = frame.clone();
        max_payload.jpeg_payload = vec![0; VIEWER_WS_FRAME_PAYLOAD_MAX_BYTES];
        let max_payload_encoded = max_payload.encode().unwrap();
        assert_eq!(
            ViewerWsFrame::decode(&max_payload_encoded)
                .unwrap()
                .jpeg_payload
                .len(),
            VIEWER_WS_FRAME_PAYLOAD_MAX_BYTES
        );
        max_payload.jpeg_payload.push(0);
        assert_eq!(
            max_payload.encode(),
            Err(ViewerWsFrameCodecError::PayloadTooLarge)
        );
    }

    #[test]
    fn every_contract_family_has_schema_and_round_trip_evidence() {
        macro_rules! schema {
            ($($ty:ty),+ $(,)?) => {
                $(
                    let schema = schemars::schema_for!($ty);
                    assert!(!serde_json::to_value(schema).unwrap().is_null());
                )+
            };
        }
        schema!(
            ConversationBinding,
            BrowserTargetBinding,
            IssuerIdentity,
            ViewerBinding,
            ArtifactBinding,
            ResourceBinding,
            PreviewKind,
            DisplayPolicy,
            PreviewSource,
            RedactionStatus,
            CompanionArrangement,
            ViewportCssPx,
            CompanionScreen,
            AnimationPreview,
            CompanionStateHint,
            CompanionLayout,
            CompanionCandidate,
            PreviewProvenance,
            PreviewResource,
            Controller,
            BrowserLiveSnapshot,
            ViewerOrigin,
            PointerButton,
            HumanInputEvent,
            HumanInputRequest,
            AttachRequest,
            ControlRequest,
            ViewerClientMessage,
            InputRejectReason,
            HumanInputResult,
            ControlResult,
            ViewerPermission,
            IssueViewerGrantRequest,
            IssueViewerGrantResponse,
            ViewerGrantInfo,
            RevokeViewerGrantRequest,
            RevokeViewerGrantResponse,
            FragmentNonce,
            ExchangeTicket,
            ViewerSecret,
            ConsumeFragmentNonceRequest,
            ConsumeFragmentNonceResponse,
            ExchangeViewerGrantRequest,
            ExchangeViewerGrantResponse,
            AuthHello,
            ViewerAuthClientMessage,
            AuthOk,
            AuthErrorCode,
            AuthError,
            ViewerAuthServerMessage,
            ViewerWsFrameHeader,
            BrowserLiveState,
            ClosedReason,
            BrowserLiveClosed,
            InterventionEnd,
            InterventionFacts,
            InterventionSummary,
            BusEvent,
            ViewerServerMessage,
        );

        round_trip(&ResourceBinding::Artifact {
            artifact: ArtifactBinding {
                principal: IssuerIdentity::User {
                    user_id: "user-42".to_string(),
                },
                task: task(),
                artifact_id: "artifact-1".to_string(),
            },
        });
        round_trip(&snapshot());
        round_trip(&ViewerClientMessage::Attach {
            request: AttachRequest {
                last_frame_seq: Some(17),
            },
        });
        round_trip(&ViewerClientMessage::Take {
            request: ControlRequest {
                expected_control_epoch: 19,
                action_seq: 1,
                expires_at_ms: 1_799_000_060_000,
            },
        });
        round_trip(&ViewerClientMessage::Input {
            request: HumanInputRequest {
                connection_epoch: 11,
                stream_epoch: 13,
                control_epoch: 19,
                seq: 1,
                expires_at_ms: 1_799_000_060_000,
                event: HumanInputEvent::Navigate {
                    url: "https://example.test/".to_string(),
                },
            },
        });
        round_trip(&ViewerServerMessage::Snapshot {
            snapshot: snapshot(),
        });
        round_trip(&HumanInputResult {
            seq: 1,
            accepted: false,
            reason: Some(InputRejectReason::PermissionDenied),
        });
        round_trip(&ControlResult {
            action_seq: 1,
            accepted: true,
            control_epoch: 20,
            reason: None,
        });

        let grant_info = ViewerGrantInfo {
            grant_id: "grant-public".to_string(),
            issuer: IssuerIdentity::Connection {
                connection_id: "connection-8".to_string(),
            },
            binding: viewer_binding(),
            resource_id: "resource-5".to_string(),
            permissions: vec![
                ViewerPermission::Frames,
                ViewerPermission::HumanInput,
                ViewerPermission::Navigate,
            ],
            origin: ViewerOrigin {
                scheme: "https".to_string(),
                host: "example.test".to_string(),
                port: 443,
            },
            expires_at_ms: 1_799_000_060_000,
        };
        round_trip(&IssueViewerGrantRequest {
            resource_id: "resource-5".to_string(),
            permissions: vec![ViewerPermission::Frames, ViewerPermission::HumanInput],
        });
        round_trip(&IssueViewerGrantResponse {
            launch_url: "https://example.test/viewer#nonce=redacted".to_string(),
            grant_id: "grant-public".to_string(),
            expires_at_ms: 1_799_000_060_000,
        });
        round_trip(&grant_info);
        round_trip(&RevokeViewerGrantRequest {
            grant_id: "grant-public".to_string(),
        });
        round_trip(&RevokeViewerGrantResponse { revoked: true });
        round_trip(&FragmentNonce("A".repeat(43)));
        round_trip(&ExchangeTicket("B".repeat(43)));
        round_trip(&ViewerSecret("C".repeat(43)));
        round_trip(&ConsumeFragmentNonceRequest {
            grant_id: "grant-public".to_string(),
            nonce: FragmentNonce("A".repeat(43)),
        });
        round_trip(&ConsumeFragmentNonceResponse {
            ticket: ExchangeTicket("B".repeat(43)),
            expires_at_ms: 1_799_000_060_000,
        });
        round_trip(&ExchangeViewerGrantRequest {
            ticket: ExchangeTicket("B".repeat(43)),
        });
        round_trip(&ExchangeViewerGrantResponse {
            viewer_session_id: "viewer-session-1".to_string(),
            grant_id: "grant-public".to_string(),
            rotation_generation: 0,
            current_secret: ViewerSecret("C".repeat(43)),
            ws_path: "/viewer/ws".to_string(),
            expires_at_ms: 1_799_000_060_000,
            snapshot: snapshot(),
        });
        round_trip(&AuthHello {
            protocol_version: VIEWER_PROTOCOL_VERSION,
            grant_id: "grant-public".to_string(),
            rotation_generation: 0,
            secret: "C".repeat(43),
        });

        let frame_header = ViewerWsFrameHeader {
            version: 1,
            resource_id: "resource-5".to_string(),
            binding: viewer_binding(),
            connection_epoch: 11,
            stream_epoch: 13,
            seq: 17,
            captured_at_ms: 1_799_000_000_000,
            viewport_css_px: ViewportCssPx {
                width: 1280,
                height: 720,
            },
            content_type: "image/jpeg".to_string(),
        };
        round_trip(&frame_header);
        round_trip(&BrowserLiveState {
            resource_id: "resource-5".to_string(),
            binding: viewer_binding(),
            connection_epoch: 11,
            stream_epoch: 13,
            viewport_css_px: ViewportCssPx {
                width: 1280,
                height: 720,
            },
            controller: Controller::Agent,
            control_epoch: 19,
        });

        let closed = BrowserLiveClosed {
            resource_id: "resource-5".to_string(),
            binding: ResourceBinding::Browser {
                viewer: viewer_binding(),
            },
            reason: ClosedReason::TargetClosed,
            at_ms: 1_799_000_000_000,
        };
        let summary = InterventionSummary {
            intervention_id: "intervention-1".to_string(),
            destination: task(),
            target: target(),
            principal: IssuerIdentity::User {
                user_id: "user-42".to_string(),
            },
            started_at_ms: 1_799_000_000_000,
            ended_at_ms: 1_799_000_001_000,
            control_epoch_before: 1,
            control_epoch_after: 2,
            ended_by: InterventionEnd::Cede,
            facts: InterventionFacts {
                input_event_count: 3,
                typed_text_chars: 2,
                navigation_count: 0,
                final_origin: None,
            },
        };
        round_trip(&BusEvent::Resource {
            resource: resource(),
        });
        round_trip(&BusEvent::Closed { closed });
        round_trip(&BusEvent::Intervention { summary });

        assert!(serde_json::from_value::<PreviewKind>(json!("unknown_kind")).is_err());
        assert!(serde_json::from_value::<HumanInputEvent>(json!({
            "action": "unknown_action"
        }))
        .is_err());
        assert!(serde_json::from_value::<ViewerClientMessage>(json!({
            "type": "unknown_message"
        }))
        .is_err());
    }

    #[test]
    fn secrets_are_absent_from_resources_and_debug_output() {
        let value = serde_json::to_value(resource()).unwrap();
        let rendered = value.to_string();
        for forbidden in ["secret", "nonce", "ticket", "grant_id"] {
            assert!(!rendered.contains(forbidden));
        }

        let secret = "top-secret-value";
        let issued = IssueViewerGrantResponse {
            launch_url: format!("https://example.test/viewer#nonce={secret}"),
            grant_id: "grant-public".to_string(),
            expires_at_ms: 1_799_000_060_000,
        };
        assert!(!format!("{issued:?}").contains(secret));

        let request = ConsumeFragmentNonceRequest {
            grant_id: "grant-public".to_string(),
            nonce: FragmentNonce(secret.to_string()),
        };
        assert!(!format!("{request:?}").contains(secret));
        assert!(!format!("{:?}", ExchangeTicket(secret.to_string())).contains(secret));
        assert!(!format!("{:?}", ViewerSecret(secret.to_string())).contains(secret));
        assert!(!format!(
            "{:?}",
            ConsumeFragmentNonceResponse {
                ticket: ExchangeTicket(secret.to_string()),
                expires_at_ms: 1_799_000_060_000,
            }
        )
        .contains(secret));
        assert!(!format!(
            "{:?}",
            ExchangeViewerGrantRequest {
                ticket: ExchangeTicket(secret.to_string()),
            }
        )
        .contains(secret));

        let response = ExchangeViewerGrantResponse {
            viewer_session_id: "viewer-session-1".to_string(),
            grant_id: "grant-public".to_string(),
            rotation_generation: 0,
            current_secret: ViewerSecret(secret.to_string()),
            ws_path: "/viewer/ws".to_string(),
            expires_at_ms: 1_799_000_060_000,
            snapshot: snapshot(),
        };
        assert!(!format!("{response:?}").contains(secret));
        assert!(!response.ws_path.contains(secret));
    }

    #[test]
    fn list_request_is_empty_and_rejects_any_client_filter() {
        let decoded: ListResourcesRequest = serde_json::from_value(json!({})).unwrap();
        assert_eq!(decoded, ListResourcesRequest {});
        assert_eq!(serde_json::to_value(decoded).unwrap(), json!({}));
        for filter in [
            json!({"user_id": "someone"}),
            json!({"session_key": "s"}),
            json!({"target_id": "t"}),
            json!({"url": "https://example.test"}),
        ] {
            assert!(serde_json::from_value::<ListResourcesRequest>(filter).is_err());
        }
    }

    #[test]
    fn list_response_carries_exactly_the_bus_resource_shape() {
        let response = ListResourcesResponse {
            resources: vec![resource()],
        };
        let value = serde_json::to_value(&response).unwrap();
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, vec!["resources"]);
        assert_eq!(
            value["resources"][0],
            serde_json::to_value(resource()).unwrap()
        );
        let back: ListResourcesResponse = serde_json::from_value(value).unwrap();
        assert_eq!(back, response);
        assert!(serde_json::from_value::<ListResourcesResponse>(
            json!({"resources": [], "cursor": 1})
        )
        .is_err());
    }
}
