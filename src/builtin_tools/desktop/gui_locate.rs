//! `desktop_gui_locate` — natural-language → on-screen coordinates.
//!
//! Two-stage locator that translates a human-readable target string into the
//! center pixel of a clickable element. Pure deterministic dispatch — no LLM
//! roundtrip on the path. Drops into the standard `desktop` flow: caller
//! feeds the returned `center` into `desktop` tool's `click`/`hover`/etc.
//!
//! Stage 1 — accessibility tree (cheap, structured, exact bounds):
//!   Walk the frontmost app's AX subtree, collect elements whose `title` or
//!   `value` contains `target_text` (case-insensitive). Prefer matches inside
//!   `INTERACTABLE_ROLES` so we never return a static-text label as the click
//!   target.
//!
//! Stage 2 — OCR fallback (only when AX returned no hit):
//!   Take a screenshot, run it through Vision OCR, fuzzy-match `target_text`
//!   against recognized line text, return the matching line's center.
//!
//! On platforms without `AccessibilityCapability` (non-macOS) the locator
//! skips Stage 1 and goes straight to OCR.

use std::collections::HashSet;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use aleph_protocol::desktop_bridge::methods::ax::{AxElement, QueryTreeParams, DEFAULT_MAX_NODES};

use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::interactable::{affordance_fields, safe_value, INTERACTABLE_ROLES};
use super::types::DesktopOutput;

const SEARCH_MAX_DEPTH: u32 = 32;
const MAX_CANDIDATES: usize = 5;

/// LLM tool args for `desktop_gui_locate`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DesktopGuiLocateArgs {
    /// Visible text on the element to find (button label, link text, OCR
    /// content). Case-insensitive substring match.
    pub target_text: String,

    /// Target process id; `null` means the frontmost app.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,

    /// Optional AX role hint (e.g. `"AXButton"`). When set, candidates with
    /// this role outscore other matches. Ignored on non-macOS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefer_role: Option<String>,

    /// Skip the AX path and go straight to OCR. Useful for content that
    /// lives inside a webview / Electron app whose a11y tree is opaque.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force_ocr: bool,
}

/// LLM tool: natural-language → clickable center coordinates.
#[derive(Clone, Default)]
pub struct DesktopGuiLocate {
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl DesktopGuiLocate {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_platform(mut self, platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        self.platform = Some(platform);
        self
    }
}

#[async_trait]
impl AlephTool for DesktopGuiLocate {
    const NAME: &'static str = "desktop_gui_locate";
    const DESCRIPTION: &'static str =
        "Resolve a human-readable on-screen target (e.g. \"Send\", \"Login button\", \
         \"OK\") into pixel coordinates ready to feed into the `desktop` tool's \
         `click`/`double_click`/`hover` actions. Two-stage: (1) accessibility \
         tree fuzzy match on `title`/`value` of interactable roles — fast, \
         structured, exact bounds; (2) OCR fallback when AX has no hit (Web \
         content, custom-drawn UIs). Returns `{found, center:[x,y], width, \
         height, source:\"ax\"|\"ocr\", confidence, role?, text, candidates}`. \
         An AX hit also carries the element's own affordances: `actions` (the \
         exact AX action names it supports — feed one verbatim to `ax_action` \
         instead of guessing), `enabled:false` (greyed out), `settable:false` \
         (value not writable), `secure:true` (password field — its value is \
         never returned or matched on; locate it by its label). Pass \
         `prefer_role` (e.g. `\"AXButton\"`) to bias toward the right \
         widget type when the label is ambiguous. Pass `force_ocr:true` to \
         skip AX entirely (recommended for browser content).";

    type Args = DesktopGuiLocateArgs;
    type Output = DesktopOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let needle = args.target_text.trim();
        if needle.is_empty() {
            return Ok(DesktopOutput {
                success: false,
                data: None,
                message: Some("desktop_gui_locate: target_text must be non-empty".into()),
            });
        }

        let platform = match self.platform.as_ref() {
            Some(p) => p,
            None => {
                return Ok(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(
                        "Desktop platform capability is not configured for this server build."
                            .into(),
                    ),
                });
            }
        };

        // Stage 1: AX path (skipped on --force_ocr or non-AX platforms).
        if !args.force_ocr {
            if let Some(ax) = platform.ax() {
                let params = QueryTreeParams {
                    pid: args.pid,
                    max_depth: SEARCH_MAX_DEPTH,
                    max_nodes: DEFAULT_MAX_NODES,
                };
                match ax.query_tree(params).await {
                    // A truncated walk needs no special handling here: this
                    // stage already falls through to OCR when it does not find
                    // the target, and "did not find it" is the same outcome
                    // whether the element was absent or simply never reached.
                    Ok(result) => {
                        if let Some(out) = result.element.as_ref().and_then(|root| {
                            ax_locate_output(root, needle, args.prefer_role.as_deref())
                        }) {
                            return Ok(out);
                        }
                    }
                    Err(e) => {
                        // AX failed (perm denied, bridge offline). Don't give
                        // up — try OCR before erroring.
                        tracing::warn!(error = %e, "gui_locate: AX query failed, falling back to OCR");
                    }
                }
            }
        }

        // Stage 2: OCR path.
        let screen = match platform.screen() {
            Some(s) => s,
            None => {
                return Ok(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(
                        "desktop_gui_locate: no AX match and ScreenCapability unavailable for OCR fallback"
                            .into(),
                    ),
                });
            }
        };

        let ocr_result = match screen.ocr(None).await {
            Ok(r) => r,
            Err(e) => {
                return Ok(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(super::recovery::with_hint(format!(
                        "desktop_gui_locate: OCR failed: {e}"
                    ))),
                });
            }
        };

        let ocr_matches = collect_ocr_matches(&ocr_result.lines, needle);
        if let Some(best) = ocr_matches.first() {
            return Ok(success_output_ocr(best, &ocr_matches));
        }

        Ok(DesktopOutput {
            success: true, // valid "not found" result, not a failure
            data: Some(json!({
                "found": false,
                "needle": needle,
                "hint": "neither accessibility tree nor OCR matched. Try a different label, a substring, force_ocr:true, or screenshot to inspect.",
            })),
            message: None,
        })
    }
}

// ── AX matching ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct AxMatch {
    role: String,
    text: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    score: f64,
    /// The element's own affordances (`actions` / `enabled` / `settable` /
    /// `secure`), projected by the single shared rule so a located element
    /// answers "what can I do to it" without a second AX round-trip.
    affordances: serde_json::Map<String, serde_json::Value>,
}

/// Score an AX element against `needle` (already lowercased by caller).
///
/// 1.0 — exact case-insensitive match on title/value
/// 0.9 — prefix match (UI uses "Send Message" when caller said "Send")
/// 0.8 — substring match
/// 0.0 — no hit
fn score_ax_text(text: &str, needle_lc: &str) -> f64 {
    let lc = text.to_lowercase();
    if lc == needle_lc {
        1.0
    } else if lc.starts_with(needle_lc) {
        0.9
    } else if lc.contains(needle_lc) {
        0.8
    } else {
        0.0
    }
}

/// Locate `needle` in an AX tree and render the model-facing result, or `None`
/// when nothing matched (the caller then falls through to OCR).
///
/// Split out of [`DesktopGuiLocate::call`] so the projection — the part that
/// decides what element text reaches the model — is exercisable without a live
/// desktop.
pub(super) fn ax_locate_output(
    root: &AxElement,
    needle: &str,
    prefer_role: Option<&str>,
) -> Option<DesktopOutput> {
    let candidates = collect_ax_matches(root, needle, prefer_role);
    let best = candidates.first()?;
    Some(success_output_ax(best, &candidates))
}

/// Walk the AX subtree depth-first, collecting matches sorted best-first.
fn collect_ax_matches(root: &AxElement, needle: &str, prefer_role: Option<&str>) -> Vec<AxMatch> {
    let needle_lc = needle.to_lowercase();
    let role_allow: HashSet<&str> = INTERACTABLE_ROLES.iter().copied().collect();
    let mut out = Vec::<AxMatch>::new();
    walk_ax(root, &needle_lc, prefer_role, &role_allow, &mut out);
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(MAX_CANDIDATES);
    out
}

fn walk_ax(
    node: &AxElement,
    needle_lc: &str,
    prefer_role: Option<&str>,
    role_allow: &HashSet<&str>,
    out: &mut Vec<AxMatch>,
) {
    // Bounds must be present and non-degenerate to be clickable.
    let bounds = node
        .bounds
        .as_ref()
        .filter(|b| b.width > 1.0 && b.height > 1.0);

    if let Some(b) = bounds {
        let title_text = node.title.as_deref().unwrap_or("");
        // SECURITY: the value is read through the redaction accessor even here,
        // in the *scorer*. Scoring the raw value would rank a password field by
        // its contents, and the winning `text` is echoed back in the result —
        // the secret would leak through the ranking itself, not just through a
        // projection. A secure field is still locatable by its label.
        let value_text = safe_value(node).unwrap_or("");
        let title_score = if title_text.is_empty() {
            0.0
        } else {
            score_ax_text(title_text, needle_lc)
        };
        let value_score = if value_text.is_empty() {
            0.0
        } else {
            score_ax_text(value_text, needle_lc)
        };
        let (best_text, mut base_score) = if title_score >= value_score {
            (title_text, title_score)
        } else {
            (value_text, value_score)
        };

        if base_score > 0.0 {
            // Penalize non-interactable roles unless the caller hinted them.
            let is_interactable = role_allow.contains(node.role.as_str());
            if !is_interactable {
                base_score *= 0.5;
            }
            // Bonus when role matches `prefer_role`. Score is allowed past
            // 1.0 — it's an internal sort key, and a value > 1.0 in the
            // returned `confidence` is a useful "more certain than exact"
            // signal for the caller.
            if let Some(want) = prefer_role {
                if node.role == want {
                    base_score += 0.1;
                }
            }
            out.push(AxMatch {
                role: node.role.clone(),
                text: best_text.to_string(),
                x: b.x,
                y: b.y,
                width: b.width,
                height: b.height,
                score: base_score,
                affordances: affordance_fields(node),
            });
        }
    }

    for child in &node.children {
        walk_ax(child, needle_lc, prefer_role, role_allow, out);
    }
}

/// Third of the three model-facing element projections (with
/// `ax::element_to_json` and `set_of_marks::build_marks`). Values reached it
/// only through [`safe_value`] in the scorer above; the affordances ride along
/// so the caller can act on the located element without re-querying AX.
fn success_output_ax(best: &AxMatch, all: &[AxMatch]) -> DesktopOutput {
    let cx = best.x + best.width / 2.0;
    let cy = best.y + best.height / 2.0;
    let candidates: Vec<serde_json::Value> = all
        .iter()
        .map(|m| {
            let mut obj = json!({
                "x": m.x,
                "y": m.y,
                "width": m.width,
                "height": m.height,
                "center": [m.x + m.width / 2.0, m.y + m.height / 2.0],
                "text": m.text,
                "role": m.role,
                "score": m.score,
            });
            if let Some(map) = obj.as_object_mut() {
                map.extend(m.affordances.clone());
            }
            obj
        })
        .collect();
    let mut data = json!({
        "found": true,
        "source": "ax",
        "confidence": best.score,
        "center": [cx, cy],
        "x": best.x,
        "y": best.y,
        "width": best.width,
        "height": best.height,
        "role": best.role,
        "text": best.text,
        "candidates": candidates,
    });
    if let Some(map) = data.as_object_mut() {
        map.extend(best.affordances.clone());
    }
    DesktopOutput {
        success: true,
        data: Some(data),
        message: None,
    }
}

// ── OCR matching ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct OcrMatch {
    text: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    score: f64,
}

fn collect_ocr_matches(lines: &[aleph_desktop::OcrLine], needle: &str) -> Vec<OcrMatch> {
    let needle_lc = needle.to_lowercase();
    let mut out: Vec<OcrMatch> = lines
        .iter()
        .filter_map(|l| {
            let score = score_ax_text(&l.text, &needle_lc);
            if score <= 0.0 {
                return None;
            }
            let b = l.bounding_box?;
            Some(OcrMatch {
                text: l.text.clone(),
                x: b.x,
                y: b.y,
                width: b.w,
                height: b.h,
                // OCR is lower-confidence than AX — knock the score down so
                // a callsite that sees both sources doesn't tie-break wrong.
                score: score * 0.75,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(MAX_CANDIDATES);
    out
}

fn success_output_ocr(best: &OcrMatch, all: &[OcrMatch]) -> DesktopOutput {
    let cx = best.x + best.width / 2.0;
    let cy = best.y + best.height / 2.0;
    let candidates: Vec<serde_json::Value> = all
        .iter()
        .map(|m| {
            json!({
                "x": m.x,
                "y": m.y,
                "width": m.width,
                "height": m.height,
                "center": [m.x + m.width / 2.0, m.y + m.height / 2.0],
                "text": m.text,
                "score": m.score,
            })
        })
        .collect();
    DesktopOutput {
        success: true,
        data: Some(json!({
            "found": true,
            "source": "ocr",
            "confidence": best.score,
            "center": [cx, cy],
            "x": best.x,
            "y": best.y,
            "width": best.width,
            "height": best.height,
            "text": best.text,
            "candidates": candidates,
        })),
        message: None,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_desktop::{BoundingBox, OcrLine};
    use aleph_protocol::desktop_bridge::methods::screen::Region;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Region {
        Region {
            x,
            y,
            width,
            height,
        }
    }

    fn leaf(
        role: &str,
        title: Option<&str>,
        value: Option<&str>,
        bounds: Option<Region>,
    ) -> AxElement {
        AxElement {
            role: role.to_string(),
            title: title.map(str::to_string),
            value: value.map(str::to_string),
            bounds,
            pid: 1,
            ..Default::default()
        }
    }

    fn window(children: Vec<AxElement>) -> AxElement {
        AxElement {
            role: "AXWindow".into(),
            pid: 1,
            children,
            ..Default::default()
        }
    }

    #[test]
    fn ax_exact_title_match_beats_substring() {
        let tree = window(vec![
            leaf(
                "AXButton",
                Some("Send Now"),
                None,
                Some(rect(0.0, 0.0, 60.0, 30.0)),
            ),
            leaf(
                "AXButton",
                Some("Send"),
                None,
                Some(rect(100.0, 0.0, 60.0, 30.0)),
            ),
        ]);
        let matches = collect_ax_matches(&tree, "Send", None);
        assert_eq!(matches[0].text, "Send"); // exact wins over "Send Now"
        assert!(matches[0].score >= matches[1].score);
    }

    #[test]
    fn ax_prefer_role_boosts_score() {
        let tree = window(vec![
            leaf(
                "AXLink",
                Some("Login"),
                None,
                Some(rect(0.0, 0.0, 60.0, 30.0)),
            ),
            leaf(
                "AXButton",
                Some("Login"),
                None,
                Some(rect(100.0, 0.0, 60.0, 30.0)),
            ),
        ]);
        let matches = collect_ax_matches(&tree, "Login", Some("AXButton"));
        assert_eq!(matches[0].role, "AXButton");
    }

    #[test]
    fn ax_static_text_penalized_vs_button() {
        let tree = window(vec![
            leaf(
                "AXStaticText",
                Some("Save"),
                None,
                Some(rect(0.0, 0.0, 60.0, 30.0)),
            ),
            leaf(
                "AXButton",
                Some("Save"),
                None,
                Some(rect(100.0, 0.0, 60.0, 30.0)),
            ),
        ]);
        let matches = collect_ax_matches(&tree, "Save", None);
        assert_eq!(matches[0].role, "AXButton");
    }

    #[test]
    fn ax_degenerate_bounds_dropped() {
        let tree = window(vec![leaf(
            "AXButton",
            Some("OK"),
            None,
            Some(rect(0.0, 0.0, 0.0, 0.0)),
        )]);
        let matches = collect_ax_matches(&tree, "OK", None);
        assert!(matches.is_empty());
    }

    #[test]
    fn ax_match_carries_the_elements_own_actions() {
        let mut btn = leaf(
            "AXButton",
            Some("Save"),
            None,
            Some(rect(0.0, 0.0, 60.0, 30.0)),
        );
        btn.actions = Some(vec!["AXPress".into()]);
        btn.enabled = Some(false);
        let tree = window(vec![btn]);

        let out = ax_locate_output(&tree, "Save", None).expect("found");
        let data = out.data.expect("data");
        assert_eq!(data["actions"], json!(["AXPress"]));
        assert_eq!(data["enabled"], json!(false));
        assert_eq!(data["candidates"][0]["actions"], json!(["AXPress"]));
    }

    #[test]
    fn ax_scorer_never_matches_a_secure_fields_contents() {
        // The scorer used to read `value` raw: searching for the password text
        // would have ranked the password box first and echoed the secret back
        // as the match `text`.
        let mut secret = leaf(
            "AXSecureTextField",
            Some("Password"),
            Some("hunter2"),
            Some(rect(0.0, 0.0, 60.0, 30.0)),
        );
        secret.secure = Some(true);
        let tree = window(vec![secret]);

        assert!(
            ax_locate_output(&tree, "hunter2", None).is_none(),
            "a secure value must be unmatchable"
        );

        // It is still findable by its label — and the response says it is
        // secure without ever quoting what is in it.
        let out = ax_locate_output(&tree, "Password", None).expect("found by label");
        let json = serde_json::to_string(&out).unwrap();
        assert!(!json.contains("hunter2"), "secret leaked: {json}");
        assert_eq!(out.data.expect("data")["secure"], json!(true));
    }

    #[test]
    fn ocr_matches_substring() {
        let lines = vec![
            OcrLine {
                text: "Welcome".to_string(),
                bounding_box: Some(BoundingBox {
                    x: 0.0,
                    y: 0.0,
                    w: 100.0,
                    h: 20.0,
                }),
                confidence: Some(0.95),
            },
            OcrLine {
                text: "Click Send to submit".to_string(),
                bounding_box: Some(BoundingBox {
                    x: 0.0,
                    y: 30.0,
                    w: 200.0,
                    h: 20.0,
                }),
                confidence: Some(0.9),
            },
        ];
        let matches = collect_ocr_matches(&lines, "Send");
        assert_eq!(matches.len(), 1);
        assert!(matches[0].text.contains("Send"));
    }

    #[test]
    fn ocr_skips_lines_without_bbox() {
        let lines = vec![OcrLine {
            text: "Send".to_string(),
            bounding_box: None,
            confidence: Some(0.95),
        }];
        let matches = collect_ocr_matches(&lines, "Send");
        assert!(matches.is_empty());
    }

    #[tokio::test]
    async fn empty_target_text_returns_error() {
        let tool = DesktopGuiLocate::new();
        let out = tool
            .call(DesktopGuiLocateArgs {
                target_text: "   ".into(),
                pid: None,
                prefer_role: None,
                force_ocr: false,
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.unwrap().contains("non-empty"));
    }

    #[tokio::test]
    async fn without_platform_returns_message() {
        let tool = DesktopGuiLocate::new();
        let out = tool
            .call(DesktopGuiLocateArgs {
                target_text: "Send".into(),
                pid: None,
                prefer_role: None,
                force_ocr: false,
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.is_some());
    }
}
