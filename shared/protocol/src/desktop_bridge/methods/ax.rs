//! Accessibility tree query schemas for the desktop bridge.
//!
//! Exposes three read-only AX operations:
//! - `ax.query_focused` — element currently holding keyboard focus
//! - `ax.query_tree`    — full subtree rooted at a given process (or the
//!   frontmost app if `pid` is omitted)
//! - `ax.query_by_role` — collect all elements matching an AX role string

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::screen::Region;

// ── Method name constants ────────────────────────────────────────────────────

pub const METHOD_QUERY_FOCUSED: &str = "ax.query_focused";
pub const METHOD_QUERY_TREE: &str = "ax.query_tree";
pub const METHOD_QUERY_BY_ROLE: &str = "ax.query_by_role";
pub const METHOD_SET_VALUE: &str = "ax.set_value";
pub const METHOD_PERFORM_ACTION: &str = "ax.perform_action";
pub const NOTIFY_MUTATION: &str = "ax.mutation";

// ── Deadlines ────────────────────────────────────────────────────────────────

/// Deadline for the AX calls that walk a tree.
///
/// A walk is bounded by `max_nodes` (see [`QueryTreeParams`]) rather than by the
/// clock, but each node costs several round trips into the *target* app, so a
/// merely-slow app must still be given room to answer. What this cap is for is
/// the app that has stopped answering at all.
pub const DEFAULT_TIMEOUT_MS: u64 = 15_000;

/// Deadline for [`METHOD_QUERY_FOCUSED`].
///
/// This one runs on the hot path — the `type_text` focus gate issues it before
/// every single keystroke batch — and it reads exactly one element. It has no
/// business taking as long as a tree walk.
pub const TIMEOUT_MS_QUERY_FOCUSED: u64 = 3_000;

pub const TIMEOUT_OVERRIDES_MS: &[(&str, u64)] =
    &[(METHOD_QUERY_FOCUSED, TIMEOUT_MS_QUERY_FOCUSED)];

// ── Request params ───────────────────────────────────────────────────────────

/// Params for `ax.query_focused`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct QueryFocusedParams {
    /// pid to ask about; `null` means "whatever the *system* currently focuses".
    ///
    /// The distinction is the whole point of the field. Aleph's default input
    /// rail on macOS delivers into a named process without bringing it forward,
    /// so the system-focused element routinely belongs to some *other* app — and
    /// a focus check that reads it is inspecting the wrong window. With a pid the
    /// helper asks that application for its own `AXFocusedUIElement`, which is
    /// where the keystrokes are actually going to land.
    ///
    /// A helper or platform that can only answer system-wide must still honour
    /// the contract below by filtering, never by widening the answer.
    ///
    /// # Contract
    ///
    /// When `pid` is `Some`, a returned element **belongs to that process**.
    /// "Some other app holds focus" is reported as `None`, not as that app's
    /// element.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
}

/// Params for `ax.query_tree`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QueryTreeParams {
    /// pid of the target application; `null` means "use the frontmost app".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// Maximum depth of the returned subtree (default 6 to bound response size).
    #[serde(default = "default_depth")]
    pub max_depth: u32,
    /// Maximum number of nodes to return, across the whole subtree.
    ///
    /// Depth alone does not bound a tree: a browser or a long document is wide,
    /// not deep, so `max_depth` can be satisfied while the walk still emits tens
    /// of thousands of nodes. Every one of them costs several round trips into
    /// the target app on the way out and a few hundred bytes of the model's
    /// context on the way in.
    ///
    /// When the budget is reached the walk stops and [`QueryResult::truncated`]
    /// says so — the caller is told it is holding a partial tree rather than
    /// being left to assume the app really is that small.
    #[serde(default = "default_max_nodes")]
    pub max_nodes: u32,
}

const fn default_depth() -> u32 {
    6
}

/// Node budget for one tree walk.
///
/// Enough to carry a real application window (a populated Chromium window
/// measures in the high hundreds), small enough that a pathological tree cannot
/// spend the call's whole deadline or the model's whole context.
///
/// This is the contract's answer, and it exists because there used to be three:
/// the macOS helper stopped at 10 000 nodes, Windows UI Automation at 4 000 and
/// the Linux AT-SPI walk at 1 500 — three private constants, none of them
/// visible to the caller, each silently cutting the tree at a different size.
/// "How much of an app may one query return" is a property of the protocol, not
/// of whichever limb happens to answer.
pub const DEFAULT_MAX_NODES: u32 = 1_500;

/// Ceiling a caller may raise [`QueryTreeParams::max_nodes`] to.
///
/// The budget is adjustable because some trees genuinely are large, and fixed at
/// the top because every node is several round trips into another process: an
/// unbounded request is a request to spend the whole call deadline.
pub const MAX_MAX_NODES: u32 = 10_000;

/// Clamp a caller-supplied node budget into the range limbs will honour.
///
/// Zero is treated as "unspecified" rather than "return nothing": a budget of
/// none is never what a caller means, and an empty tree is the one answer a
/// model cannot tell apart from an inaccessible app.
#[must_use]
pub const fn clamp_max_nodes(requested: u32) -> u32 {
    if requested == 0 {
        DEFAULT_MAX_NODES
    } else if requested > MAX_MAX_NODES {
        MAX_MAX_NODES
    } else {
        requested
    }
}

const fn default_max_nodes() -> u32 {
    DEFAULT_MAX_NODES
}

/// Params for `ax.query_by_role`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QueryByRoleParams {
    /// AX role string to match, e.g. `"AXButton"`.
    pub role: String,
    /// pid of the target application; `null` means "use the frontmost app".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// Node budget for the walk this search runs over — see
    /// [`QueryTreeParams::max_nodes`]. Bounds the *search*, not the number of
    /// matches.
    #[serde(default = "default_max_nodes")]
    pub max_nodes: u32,
}

/// Stateless element locator for `ax.set_value` / `ax.perform_action`.
///
/// The bridge re-walks the AX tree on every call and picks the best match:
/// role filter → title match (exact beats contains, case-insensitive) →
/// nearest `center` tiebreak. No element handles cross the IPC boundary.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AxLocator {
    /// pid of the target application; `null` means "use the frontmost app".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// AX role filter, e.g. `"AXTextField"`. Optional but recommended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Title/label to match (exact beats contains, case-insensitive).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Global screen-point `[x, y]` used as a nearest-center tiebreak. The
    /// bridge compares this against AX bounds in global screen POINTS, so a
    /// `coord_space:"normalized"`-derived pixel center can be off by the
    /// display scale factor on Retina displays — supply `role`/`title` as
    /// the primary locator key; `center` only breaks ties.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub center: Option<[f64; 2]>,
}

/// Params for `ax.set_value`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SetValueParams {
    pub locator: AxLocator,
    /// New value written to the element's `AXValue` attribute.
    pub value: String,
}

/// Params for `ax.perform_action`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PerformActionParams {
    pub locator: AxLocator,
    /// AX action name passed through verbatim, e.g. `"AXPress"`.
    pub action: String,
}

/// Post-write verification outcome.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AxVerification {
    /// `"verified"` when the read-back value matches the written value,
    /// `"unverified"` otherwise (see `reason`).
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// First 200 chars of the value read back after the write.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_preview: Option<String>,
}

/// Result for `ax.set_value` and `ax.perform_action`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AxActionResult {
    /// Whether the native AX call was issued successfully.
    pub performed: bool,
    /// Always `"accessibility"` — mirrors orca's action-path metadata.
    pub path: String,
    /// The element acted on (children pruned), for model visibility.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<AxElement>,
    /// Present for `set_value`; absent for `perform_action`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<AxVerification>,
}

// ── Response types ───────────────────────────────────────────────────────────

/// A node in the AX element tree.
///
/// `children` is empty when the subtree has been pruned at the requested depth.
///
/// The affordance fields (`secure` / `enabled` / `settable` / `actions` / `url`)
/// are `Option` rather than plain values on purpose: `None` means *"the limb did
/// not tell us"*, which is **not** the same as `false` / empty. An older helper
/// binary predates them and simply omits them from the wire, so every consumer
/// must treat unknown as unknown — never as a negative. `skip_serializing_if`
/// keeps the serialized form byte-identical to the pre-affordance wire when a
/// field is absent.
///
/// `Default` exists so constructors (tests, limbs) can spell only the fields
/// they care about via `..Default::default()` and stay source-compatible when
/// further affordances are added.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct AxElement {
    /// AX role identifier, e.g. `"AXWindow"`, `"AXButton"`.
    pub role: String,
    /// Human-readable title / label (may be absent).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// String value of the element (may be absent).
    ///
    /// SECURITY: this is the **raw** value as reported by the limb and may be a
    /// password. Never render it straight into a model-visible payload — go
    /// through the shared redaction accessor
    /// (`builtin_tools::desktop::interactable::safe_value`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Bounding rectangle in screen-point coordinates, top-left origin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Region>,
    /// Process ID of the owning application.
    pub pid: i32,
    /// `true` when the element masks its content (a password field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secure: Option<bool>,
    /// `false` when the element is present but greyed out. A disabled element is
    /// still reported — "Submit is disabled" is the state a model needs in order
    /// to infer "fill the form first".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// `true` when the element's value can be written via `ax.set_value`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settable: Option<bool>,
    /// Raw AX action names the element supports, e.g. `["AXPress"]` — pass one
    /// verbatim to `ax.perform_action`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<String>>,
    /// Target URL for link-like elements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Child elements (empty when depth limit reached).
    #[serde(default)]
    pub children: Vec<Self>,
}

/// Result for `ax.query_focused` and `ax.query_tree`.
///
/// `element` is `null` when no element was found (e.g. no focused element,
/// or the process is not accessible).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct QueryResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element: Option<AxElement>,
    /// Nodes actually walked, whether or not they all made it into `element`.
    ///
    /// `0` from a helper that predates the budget — like the affordance fields,
    /// absent means "not told", never "none".
    #[serde(default)]
    pub node_count: u32,
    /// `true` when the walk stopped on [`QueryTreeParams::max_nodes`], i.e. the
    /// subtree is **incomplete**.
    ///
    /// A silent cut is the dangerous version of this: a model handed a pruned
    /// tree with no marker concludes the control it is looking for does not
    /// exist, and goes off to do something else. Say it out loud instead.
    #[serde(default)]
    pub truncated: bool,
}

/// Result for `ax.query_by_role`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct QueryListResult {
    pub elements: Vec<AxElement>,
    /// Nodes walked while searching (not the number of matches).
    #[serde(default)]
    pub node_count: u32,
    /// `true` when the search stopped on the node budget — there may be further
    /// matches this call never reached.
    #[serde(default)]
    pub truncated: bool,
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_tree_default_depth() {
        let p: QueryTreeParams = serde_json::from_str("{}").unwrap();
        assert_eq!(p.max_depth, 6);
    }

    #[test]
    fn query_focused_params_roundtrip() {
        let p = QueryFocusedParams { pid: Some(4242) };
        let json = serde_json::to_string(&p).unwrap();
        let back: QueryFocusedParams = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pid, Some(4242));
    }

    /// An older client sends `{}`, which must still mean "ask the system".
    #[test]
    fn query_focused_params_without_a_pid_are_the_system_wide_question() {
        let back: QueryFocusedParams = serde_json::from_str("{}").unwrap();
        assert_eq!(back.pid, None);
        assert_eq!(serde_json::to_string(&back).unwrap(), "{}");
    }

    #[test]
    fn query_by_role_params_roundtrip() {
        let p = QueryByRoleParams {
            role: "AXButton".to_string(),
            pid: Some(1234),
            max_nodes: DEFAULT_MAX_NODES,
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: QueryByRoleParams = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, "AXButton");
        assert_eq!(back.pid, Some(1234));
    }

    #[test]
    fn ax_element_children_default_empty() {
        let json = r#"{"role":"AXWindow","pid":42}"#;
        let el: AxElement = serde_json::from_str(json).unwrap();
        assert_eq!(el.role, "AXWindow");
        assert!(el.children.is_empty());
    }

    #[test]
    fn affordances_absent_deserialize_as_unknown() {
        // An older helper binary emits no affordance keys at all. Every one of
        // them must land as `None` ("not told"), never as `Some(false)`.
        let json = r#"{"role":"AXButton","pid":42}"#;
        let el: AxElement = serde_json::from_str(json).unwrap();
        assert_eq!(el.secure, None);
        assert_eq!(el.enabled, None);
        assert_eq!(el.settable, None);
        assert_eq!(el.actions, None);
        assert_eq!(el.url, None);
    }

    #[test]
    fn affordances_absent_serialize_to_the_pre_affordance_wire() {
        // Byte-identical to what the struct produced before the fields existed.
        let el = AxElement {
            role: "AXButton".into(),
            pid: 42,
            ..Default::default()
        };
        let json = serde_json::to_string(&el).unwrap();
        assert_eq!(json, r#"{"role":"AXButton","pid":42,"children":[]}"#);
    }

    #[test]
    fn affordances_roundtrip_when_present() {
        let json = r#"{"role":"AXTextField","pid":1,"secure":true,"enabled":false,
                       "settable":true,"actions":["AXPress","AXShowMenu"],
                       "url":"https://example.com"}"#;
        let el: AxElement = serde_json::from_str(json).unwrap();
        assert_eq!(el.secure, Some(true));
        assert_eq!(el.enabled, Some(false));
        assert_eq!(el.settable, Some(true));
        assert_eq!(
            el.actions.as_deref(),
            Some(["AXPress".to_string(), "AXShowMenu".to_string()].as_slice())
        );
        assert_eq!(el.url.as_deref(), Some("https://example.com"));

        let back: AxElement = serde_json::from_str(&serde_json::to_string(&el).unwrap()).unwrap();
        assert_eq!(back.enabled, Some(false));
        assert_eq!(back.secure, Some(true));
    }

    #[test]
    fn query_result_element_null() {
        let json = r#"{}"#;
        let r: QueryResult = serde_json::from_str(json).unwrap();
        assert!(r.element.is_none());
    }

    /// A helper that predates the node budget sends neither field. Absent must
    /// decode as "not told" — and `truncated: false` is the safe reading, since
    /// such a helper applies its own (larger) cap and never reports one.
    #[test]
    fn budget_fields_absent_decode_as_untruncated() {
        let r: QueryResult = serde_json::from_str(r#"{"element":null}"#).unwrap();
        assert_eq!(r.node_count, 0);
        assert!(!r.truncated);
    }

    #[test]
    fn a_max_nodes_request_is_clamped_into_range() {
        assert_eq!(clamp_max_nodes(0), DEFAULT_MAX_NODES);
        assert_eq!(clamp_max_nodes(1), 1);
        assert_eq!(clamp_max_nodes(DEFAULT_MAX_NODES), DEFAULT_MAX_NODES);
        assert_eq!(clamp_max_nodes(MAX_MAX_NODES + 1), MAX_MAX_NODES);
        assert_eq!(clamp_max_nodes(u32::MAX), MAX_MAX_NODES);
    }

    /// The tree params carry the budget by default, so a caller that spells only
    /// `pid` still gets a bounded walk rather than an unbounded one.
    #[test]
    fn tree_params_default_to_the_declared_budget() {
        let p: QueryTreeParams = serde_json::from_str(r#"{"pid":7}"#).unwrap();
        assert_eq!(p.max_nodes, DEFAULT_MAX_NODES);
        assert_eq!(p.max_depth, 6);
    }

    #[test]
    fn query_list_result_roundtrip() {
        let r = QueryListResult {
            elements: vec![AxElement {
                role: "AXButton".to_string(),
                title: Some("OK".to_string()),
                pid: 99,
                ..Default::default()
            }],
            node_count: 12,
            truncated: false,
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: QueryListResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.elements.len(), 1);
        assert_eq!(back.elements[0].role, "AXButton");
    }

    #[test]
    fn set_value_params_roundtrip() {
        let p = SetValueParams {
            locator: AxLocator {
                pid: None,
                role: Some("AXTextField".into()),
                title: Some("Email".into()),
                center: Some([100.0, 200.0]),
            },
            value: "a@b.c".into(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: SetValueParams = serde_json::from_str(&json).unwrap();
        assert_eq!(back.locator.role.as_deref(), Some("AXTextField"));
        assert_eq!(back.value, "a@b.c");
    }

    #[test]
    fn ax_action_result_verification_optional() {
        let json = r#"{"performed":true,"path":"accessibility"}"#;
        let r: AxActionResult = serde_json::from_str(json).unwrap();
        assert!(r.performed);
        assert!(r.verification.is_none());
    }
}
