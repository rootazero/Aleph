//! `desktop_browser_operator` — declarative manifest of the tools that
//! together form a "browser operator" capability.
//!
//! R8 (Everything is a Tool): instead of building a heavyweight
//! `AbstractBrowserControlStrategy` hierarchy à la UI-TARS, Aleph exposes
//! the strategy *as data*. The model asks `desktop_browser_operator` for
//! the recommended tool set under `mode = dom | hybrid | vision` and then
//! invokes those existing tools directly. Zero new business logic — just a
//! curated index over what's already in the registry.
//!
//! R9 (Intelligence Lives in the Prompt): the manifest's text fields are
//! how-to hints the model reads at call time — there is no runtime
//! orchestration layer interpreting them.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::Result;
use crate::tools::AlephTool;

use super::types::DesktopOutput;

/// Strategy enum mirrors UI-TARS's three browser control strategies but is
/// trivially implemented as a discriminator over which tools to recommend.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BrowserOperatorMode {
    /// Structured DOM-first: relies on the in-process browser manager's DOM
    /// snapshot + click-by-selector tools. Fastest, most robust on pages
    /// the manager controls; fails on cross-origin frames it cannot reach.
    Dom,
    /// Default — combines DOM tools with screenshot/visual grounding as a
    /// fallback for content the DOM path can't reach (canvas widgets,
    /// shadow-DOM components, video, ads).
    Hybrid,
    /// Pure pixel — no DOM. Useful when driving a third-party browser via
    /// system input, or when the page actively defeats DOM automation.
    Vision,
}

impl Default for BrowserOperatorMode {
    fn default() -> Self {
        Self::Hybrid
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DesktopBrowserOperatorArgs {
    /// Strategy preset: `dom`, `hybrid` (default), or `vision`.
    #[serde(default)]
    pub mode: BrowserOperatorMode,
}

#[derive(Clone, Default)]
pub struct DesktopBrowserOperator;

impl DesktopBrowserOperator {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AlephTool for DesktopBrowserOperator {
    const NAME: &'static str = "desktop_browser_operator";
    const DESCRIPTION: &'static str =
        "Return the curated tool manifest for driving a browser in `dom`, \
         `hybrid` (default), or `vision` mode. The manifest names existing \
         Aleph tools (`browser_*`, `desktop_*`) and the order/flow to use \
         them. Call this once at the start of any browser-driving task to \
         pick a strategy; then invoke the listed tools directly. No I/O — \
         this tool only describes a plan.";

    type Args = DesktopBrowserOperatorArgs;
    type Output = DesktopOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let manifest = build_manifest(args.mode);
        Ok(DesktopOutput {
            success: true,
            data: Some(manifest),
            message: None,
        })
    }
}

/// Build the curated manifest for a given mode. Pure function — exposed at
/// `pub(crate)` so docs/tests can compare to the reference text.
pub(crate) fn build_manifest(mode: BrowserOperatorMode) -> serde_json::Value {
    let (mode_name, flow, tools) = match mode {
        BrowserOperatorMode::Dom => (
            "dom",
            vec![
                "browser_open  — open or attach to the target URL",
                "browser_snapshot  — read the DOM accessibility tree, locate elements by name/role",
                "browser_click / browser_type / browser_select  — act on the snapshot's element refs",
                "browser_wait_for  — wait for DOM signals (selector, URL, network idle)",
                "browser_screenshot  — only for capturing evidence, not for grounding",
            ],
            vec![
                ("browser_open", "Open or attach to a URL in a managed browser profile."),
                ("browser_snapshot", "DOM accessibility snapshot — element refs with role/name/value."),
                ("browser_click", "Click a snapshot element by its `ref`."),
                ("browser_type", "Type into a snapshot element."),
                ("browser_select", "Pick an `<option>` from a `<select>` element."),
                ("browser_wait_for", "Wait for a selector / URL pattern / network idle."),
                ("browser_tabs", "List, switch, or close tabs."),
                ("browser_scroll", "Scroll the page (or a specific element) by a pixel delta."),
                ("browser_screenshot", "Capture the current viewport as PNG/JPEG."),
            ],
        ),
        BrowserOperatorMode::Hybrid => (
            "hybrid",
            vec![
                "browser_open  — open or attach to the target URL",
                "browser_snapshot  — try DOM first (fast, exact)",
                "On DOM miss: desktop_gui_locate  — find by visible label (AX + OCR)",
                "browser_click OR desktop click (using the located center)",
                "browser_wait_for OR desktop wait_visual after each mutation",
                "browser_screenshot  — periodic visual ground-truth",
            ],
            vec![
                ("browser_open", "Open or attach to a URL."),
                ("browser_snapshot", "Try this first — fast, structured."),
                ("desktop_gui_locate", "Fallback: visual grounding via AX tree + OCR."),
                ("browser_click", "Click via DOM ref when snapshot found it."),
                ("desktop", "Use `click {x,y}` from gui_locate's `center` when DOM didn't expose the element."),
                ("browser_type", "Type via DOM."),
                ("desktop", "`type_text` / `key_combo` for non-DOM input (modal dialogs, OS-level prompts)."),
                ("browser_wait_for", "DOM-side wait."),
                ("desktop", "`wait_visual` for pure-pixel settle (animations, canvas redraws)."),
                ("browser_screenshot", "Visual sanity check."),
                ("desktop_ax_snapshot", "Reach OS-level chrome (download bars, OS dialogs)."),
            ],
        ),
        BrowserOperatorMode::Vision => (
            "vision",
            vec![
                "browser_open OR launch a real OS browser via desktop launch_app",
                "desktop screenshot  — visual ground truth",
                "desktop_gui_locate {target_text, force_ocr:true}  — pure OCR grounding",
                "desktop click {x,y}  — act on returned center",
                "desktop wait_visual  — wait for the page to settle",
                "Repeat",
            ],
            vec![
                ("desktop", "`screenshot`, `click`, `type_text`, `scroll`, `wait_visual` — the pixel-level driver."),
                ("desktop_gui_locate", "With `force_ocr:true` for content inside Electron/canvas where AX is opaque."),
                ("desktop_ax_snapshot", "Still useful for OS chrome that lives outside the page (download bar, OS dialogs)."),
                ("browser_screenshot", "Only if attached to a managed profile."),
            ],
        ),
    };

    json!({
        "mode": mode_name,
        "flow": flow,
        "tools": tools.into_iter().map(|(name, why)| json!({ "name": name, "why": why })).collect::<Vec<_>>(),
        "notes": [
            "Always call desktop_browser_operator FIRST in a browser task — pick a mode by the page's hostility to automation.",
            "Hybrid is the safe default. Drop to vision only when DOM tools repeatedly fail.",
            "After any click/type/scroll, call wait_visual or browser_wait_for before reading state.",
            "When unsure, call desktop_ax_snapshot — its `center` coordinates are click-ready and never lie about coordinates.",
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hybrid_is_default() {
        let tool = DesktopBrowserOperator::new();
        let out = tool
            .call(DesktopBrowserOperatorArgs {
                mode: BrowserOperatorMode::default(),
            })
            .await
            .unwrap();
        assert!(out.success);
        let data = out.data.unwrap();
        assert_eq!(data["mode"], "hybrid");
        assert!(!data["tools"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn dom_mode_lists_dom_tools_first() {
        let tool = DesktopBrowserOperator::new();
        let out = tool
            .call(DesktopBrowserOperatorArgs {
                mode: BrowserOperatorMode::Dom,
            })
            .await
            .unwrap();
        let data = out.data.unwrap();
        let first_tool = data["tools"][0]["name"].as_str().unwrap();
        assert!(first_tool.starts_with("browser_"));
    }

    #[tokio::test]
    async fn vision_mode_lists_desktop_tools() {
        let tool = DesktopBrowserOperator::new();
        let out = tool
            .call(DesktopBrowserOperatorArgs {
                mode: BrowserOperatorMode::Vision,
            })
            .await
            .unwrap();
        let data = out.data.unwrap();
        // First entry is the `desktop` pixel-driver bundle.
        assert_eq!(data["tools"][0]["name"], "desktop");
    }

    #[test]
    fn manifest_has_flow_and_notes() {
        let m = build_manifest(BrowserOperatorMode::Hybrid);
        assert!(m["flow"].as_array().unwrap().len() >= 3);
        assert!(m["notes"].as_array().unwrap().len() >= 3);
    }
}
