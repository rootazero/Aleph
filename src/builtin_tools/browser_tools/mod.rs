// Individual browser tools — focused, single-responsibility browser actions.
//
// Each tool wraps a ProfileManager and implements AlephTool for one operation.

pub mod click;
pub mod console;
pub mod cookies;
pub mod dialog;
pub mod drag;
pub mod emulate;
pub mod evaluate;
pub mod fill_form;
pub mod hover;
pub mod navigate;
pub mod network;
pub mod open;
pub mod pdf;
pub mod press_key;
pub mod profile_tool;
pub mod resize;
pub mod screenshot;
pub mod scroll;
pub mod select;
pub mod session;
pub mod snapshot;
pub mod tabs;
pub mod type_text;
pub mod upload;
pub mod wait_for;

use crate::browser::backend::BrowserBackend;
use crate::browser::chrome_mcp_backend::ChromeMcpBackend;
use crate::browser::error::BrowserError;
use crate::browser::manager::ProfileManager;
use crate::browser::playwright_cli_backend::PlaywrightCliBackend;
use crate::browser::profile::BrowserDriver;
use crate::security::content_sanitizer::{wrap_external_content, ContentSource};

use crate::approval::{ActionRequest, ActionType, ApprovalDecision, ApprovalPolicy};
use crate::sync_primitives::Arc;

/// Consult the approval policy for a sensitive browser action.
///
/// Returns `None` when the action may proceed — either the policy allows it, or
/// no policy is wired (byte-identical to the previous always-allow behavior, so
/// tests constructing tools via `new()` are unaffected). Returns `Some(message)`
/// when the action is denied or requires user confirmation; the caller surfaces
/// it as a failed tool result so the model relays the prompt to the user
/// (LLM-sovereign confirmation, R7 — the harness never blocks inline).
///
/// `Allow`/`Deny` decisions are recorded for the audit trail; `Ask` is left
/// unrecorded until the user actually responds, matching `DesktopTool`.
pub(crate) async fn check_browser_approval(
    policy: Option<&Arc<dyn ApprovalPolicy>>,
    action_type: ActionType,
    action: &str,
    target: &str,
) -> Option<String> {
    let policy = policy?;
    let (agent_id, context) = crate::approval::audit_identity("browser", action, target);
    let request = ActionRequest {
        action_type,
        target: target.to_string(),
        display_target: String::new(),
        agent_id,
        context,
        timestamp: chrono::Utc::now(),
    };

    let decision = policy.check(&request).await;
    match decision {
        ApprovalDecision::Allow => {
            policy.record(&request, &decision).await;
            None
        }
        ApprovalDecision::Deny { ref reason } => {
            policy.record(&request, &decision).await;
            Some(format!("Action denied by approval policy: {reason}"))
        }
        ApprovalDecision::Ask { ref prompt } => Some(format!("Approval required: {prompt}")),
    }
}

/// Parse one `list_tabs` line into `(id, url)`.
///
/// Handles both the Chrome `DevTools` MCP format `"N: URL"` and the Playwright
/// CLI format `"Tab N: URL"`, and strips a trailing annotation such as
/// `" [selected]"` from the URL. Returns `None` for lines without a numeric id.
fn parse_tab_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    // Normalize "Tab N: URL" → "N: URL" so both formats share one parser.
    let rest = line.strip_prefix("Tab ").unwrap_or(line);
    let colon = rest.find(": ")?;
    let id = rest.get(..colon)?.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let url_part = rest.get(colon + 2..)?.trim();
    // Strip a trailing " [selected]" / " [active]" style annotation so the URL
    // round-trips through a strict parser.
    let url = match url_part.rfind(" [") {
        Some(pos) if url_part.ends_with(']') => url_part.get(..pos).unwrap_or(url_part).trim(),
        _ => url_part,
    };
    Some((id.to_string(), url.to_string()))
}

/// The active (most recent) tab id, or `None` if no tabs are open.
/// Uses the last entry because newly opened tabs append to the list.
fn parse_active_tab_id(tabs_text: &str) -> Option<String> {
    tabs_text
        .lines()
        .filter_map(parse_tab_line)
        .map(|(id, _)| id)
        .next_back()
}

/// The current URL of `tab_id` as reported by `list_tabs`, if present.
fn extract_tab_url(tabs_text: &str, tab_id: &str) -> Option<String> {
    tabs_text
        .lines()
        .filter_map(parse_tab_line)
        .rfind(|(id, _)| id == tab_id)
        .map(|(_, url)| url)
}

/// Returns `Some(violation)` if the active tab's current http(s) URL is blocked
/// by the SSRF policy. Non-http schemes (`about:blank`, `chrome://`, …) carry no
/// network target and are skipped.
async fn current_page_block(
    manager: &ProfileManager,
    tabs_text: &str,
    tab_id: &str,
) -> Option<String> {
    let url = extract_tab_url(tabs_text, tab_id)?;
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    manager.check_url(&url).await.err().map(|v| v.to_string())
}

/// Get the active (most recent) tab from the backend, or return an error if none open.
async fn get_active_tab(backend: &dyn BrowserBackend) -> Result<String, BrowserError> {
    let tabs_text = backend.list_tabs().await?;
    parse_active_tab_id(&tabs_text)
        .ok_or_else(|| BrowserError::ActionFailed("No tabs open. Use browser_open first.".into()))
}

/// Create the appropriate backend for the given profile.
///
/// Headless mode follows the profile-level override, falling back to the global
/// `playwright_cli.headless` default (resolved via `ProfileManager::resolve_headless`).
pub(crate) fn make_backend(manager: &ProfileManager, profile: &str) -> Box<dyn BrowserBackend> {
    manager.record_activity(profile);
    match manager.get_driver(profile) {
        Some(BrowserDriver::ExistingSession) => Box::new(ChromeMcpBackend::new(
            manager.get_chrome_mcp_driver(),
            profile.to_string(),
            manager.get_ssrf_guard(),
        )),
        Some(BrowserDriver::Managed) | None => Box::new(PlaywrightCliBackend::new(
            manager.get_playwright_cli_driver(),
            profile.to_string(),
            manager.get_ssrf_guard(),
            manager.resolve_headless(profile),
        )),
    }
}

/// Create the appropriate backend and resolve the active tab ID.
pub(crate) async fn make_backend_and_tab(
    manager: &ProfileManager,
    profile: &str,
) -> Result<(Box<dyn BrowserBackend>, String), BrowserError> {
    let backend = make_backend(manager, profile);
    let tab_id = get_active_tab(backend.as_ref()).await?;
    // Reset the per-tab idle timer (Managed profiles only — see `touch_tab`).
    manager.touch_tab(profile, &tab_id);
    Ok((backend, tab_id))
}

/// Like [`make_backend_and_tab`], but additionally asserts the active tab's
/// CURRENT url passes the SSRF policy before any page-content read.
///
/// Navigation-time guards (`browser_open` / `browser_navigate` goto) only vet
/// the URL being navigated *to*. A page can still reach a forbidden internal
/// origin afterwards via an HTTP redirect, a JS-driven `location` change, or
/// back/forward history — none of which re-pass the navigation guard. Content
/// reads (snapshot, console, network, screenshot, pdf, evaluate) would then
/// exfiltrate that internal content. This closes that read-time bypass
/// (openclaw #78526 / GHSA-2x93-h3hg-2xfp).
///
/// Interaction/navigation tools deliberately keep using [`make_backend_and_tab`]
/// so the agent can always navigate *away* from a blocked page.
pub(crate) async fn make_backend_and_tab_guarded(
    manager: &ProfileManager,
    profile: &str,
) -> Result<(Box<dyn BrowserBackend>, String), BrowserError> {
    let backend = make_backend(manager, profile);
    let tabs_text = backend.list_tabs().await?;
    let tab_id = parse_active_tab_id(&tabs_text).ok_or_else(|| {
        BrowserError::ActionFailed("No tabs open. Use browser_open first.".into())
    })?;
    if let Some(violation) = current_page_block(manager, &tabs_text, &tab_id).await {
        return Err(BrowserError::NavigationFailed(format!(
            "current page blocked by SSRF policy ({violation}); \
             navigate to an allowed URL before reading page content"
        )));
    }
    // Reset the per-tab idle timer (Managed profiles only — see `touch_tab`).
    manager.touch_tab(profile, &tab_id);
    Ok((backend, tab_id))
}

/// Default per-read size budget (in characters) for page-derived text flowing
/// back to the LLM. A large `console` / `network` dump or `evaluate` result would
/// otherwise flood the model context window unbounded; this caps every content
/// read at a sane ceiling. `browser_snapshot` overrides it via its `max_chars` arg.
pub(crate) const DEFAULT_CONTENT_MAX_CHARS: usize = 30_000;

/// Coherently truncate `text` to at most `max_chars` characters, returning
/// `(text, truncated)`.
///
/// Cuts back to the last line boundary within the budget so a `[ref=eN]` token
/// — which the model needs intact to act on the element — is never split
/// mid-way. Char-safe: indexes only on `char_indices` boundaries, never raw byte
/// slices (P7 UTF-8 safety).
pub(crate) fn bound_content(text: &str, max_chars: usize) -> (String, bool) {
    // Byte index where the (max_chars)-th char starts; `None` => already within budget.
    let byte_cut = match text.char_indices().nth(max_chars) {
        Some((idx, _)) => idx,
        None => return (text.to_string(), false),
    };
    let head = &text[..byte_cut];
    // Prefer the last newline so we never emit a half line; fall back to the
    // char boundary when the budget contains no line break.
    let cut = head.rfind('\n').map_or(byte_cut, |p| p + 1);
    (text[..cut].to_string(), true)
}

/// Tail-preserving variant of [`bound_content`] for append-ordered logs
/// (console / network), where the NEWEST — and usually most relevant — entries
/// sit at the END. Head-only truncation would drop exactly the lines the
/// model's last action produced. Keeps ~40% head + ~60% tail on line
/// boundaries with an elision marker in between (mirrors the shell tool's
/// head+tail split).
pub(crate) fn bound_content_head_tail(text: &str, max_chars: usize) -> (String, bool) {
    let total_chars = text.chars().count();
    if total_chars <= max_chars {
        return (text.to_string(), false);
    }
    let head_budget = max_chars * 2 / 5;
    let tail_budget = max_chars - head_budget;
    let (head, _) = bound_content(text, head_budget);
    // Byte index where the last `tail_budget` chars start (char-safe; `skip`
    // is in 1..total_chars because total_chars > max_chars >= tail_budget).
    let skip = total_chars - tail_budget;
    let byte_start = text.char_indices().nth(skip).map_or(0, |(i, _)| i);
    // Advance to the next line start so the tail never opens mid-line.
    let tail = match text[byte_start..].find('\n') {
        Some(nl) => &text[byte_start + nl + 1..],
        None => &text[byte_start..],
    };
    let elided = total_chars
        .saturating_sub(head.chars().count())
        .saturating_sub(tail.chars().count());
    // `bound_content` ends on a line boundary when it found one; glue a
    // newline in only when it had to fall back to a mid-line char cut.
    let sep = if head.ends_with('\n') || head.is_empty() {
        ""
    } else {
        "\n"
    };
    (
        format!("{head}{sep}…[{elided} chars elided]…\n{tail}"),
        true,
    )
}

/// Redact embedded secrets, then fence the untrusted page content with the
/// prompt-injection boundary, in that order.
///
/// Operates on already-bounded text. Callers that need a size budget use
/// [`redact_and_wrap`] (default budget); `browser_snapshot` pairs
/// [`bound_content`] (with its own `max_chars`) with this directly.
pub(crate) fn redact_wrap(manager: &ProfileManager, text: &str) -> String {
    let redacted = manager.redact_content(text);
    wrap_external_content(&redacted, ContentSource::BrowserContent)
}

/// Single egress chokepoint for browser page content flowing back to the LLM.
///
/// Composes the three content-boundary transforms that every page-derived text
/// read must pass through, in order:
///
/// 1. **Size budget** ([`bound_content`]) — caps the read at
///    [`DEFAULT_CONTENT_MAX_CHARS`] so an oversized console / network / eval
///    payload cannot flood the model context. Truncation lands on a line boundary.
/// 2. **Secret redaction** (`ProfileManager::redact_content`) — the OUT half of
///    the secret-egress boundary: scrubs embedded credentials so they cannot
///    leak into the model context, memory, or provider requests. Symmetric to
///    the navigation-time `check_navigation` guard (the IN half).
/// 3. **Prompt-injection wrapping** (`wrap_external_content`) — fences the
///    untrusted page content so chat-template markers injected by a hostile page
///    cannot escape the boundary.
///
/// Used by `browser_evaluate` (front-loaded payloads, where the head of the
/// result matters most). Append-ordered logs go through
/// [`redact_and_wrap_log`] instead. Routing every content read through these
/// two functions keeps the ordering correct and guarantees future content
/// tools inherit all three guards.
pub(crate) fn redact_and_wrap(manager: &ProfileManager, text: &str) -> String {
    let (bounded, truncated) = bound_content(text, DEFAULT_CONTENT_MAX_CHARS);
    let wrapped = redact_wrap(manager, &bounded);
    if truncated {
        format!(
            "{wrapped}\n[content truncated to {DEFAULT_CONTENT_MAX_CHARS} chars; \
             refine the action or read the page in smaller sections]"
        )
    } else {
        wrapped
    }
}

pub(crate) fn process_evaluate_result(
    manager: &ProfileManager,
    raw: &str,
) -> serde_json::Value {
    let text = match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(serde_json::Value::String(s)) => s,
        Ok(other) => serde_json::to_string(&other).unwrap_or_else(|_| raw.to_string()),
        Err(_) => raw.to_string(),
    };
    serde_json::Value::String(redact_and_wrap(manager, &text))
}

/// [`redact_and_wrap`] variant for append-ordered logs (`browser_console` /
/// `browser_network`): same redact → wrap pipeline, but truncation keeps both
/// the head and the tail so the newest entries — the ones the model's last
/// action produced — survive.
pub(crate) fn redact_and_wrap_log(manager: &ProfileManager, text: &str) -> String {
    let (bounded, truncated) = bound_content_head_tail(text, DEFAULT_CONTENT_MAX_CHARS);
    let wrapped = redact_wrap(manager, &bounded);
    if truncated {
        format!(
            "{wrapped}\n[log truncated to {DEFAULT_CONTENT_MAX_CHARS} chars: middle elided, \
             oldest and newest entries preserved]"
        )
    } else {
        wrapped
    }
}

/// Maximum screenshot edge (px) handed back to the model. Anthropic vision
/// downscales any image whose longest edge exceeds ~1568px server-side, so a
/// larger capture only burns request tokens for no added legibility — and a
/// `full_page` screenshot can be many thousands of px tall. Capping here is the
/// pixel-budget twin of [`DEFAULT_CONTENT_MAX_CHARS`] on text reads, closing the
/// gap where an oversized screenshot floods the model request while every text
/// read was already bounded. Same cap and image-crate path as
/// [`crate::builtin_tools::file_ops`]'s image-read downscale.
pub(crate) const MAX_SCREENSHOT_EDGE: u32 = 1568;

/// Downscale a screenshot's longest edge to [`MAX_SCREENSHOT_EDGE`], re-encoding
/// as PNG (the screenshot format contract every consumer — base64 output and the
/// vision bridge — assumes). Returns the input bytes **unchanged** when the
/// image is already within the cap (zero re-encode) or when decoding/encoding
/// fails: post-processing must never turn a successful capture into a failure.
pub(crate) fn bound_screenshot_png(png_bytes: Vec<u8>) -> Vec<u8> {
    let Ok(decoded) = image::load_from_memory(&png_bytes) else {
        return png_bytes;
    };
    if decoded.width().max(decoded.height()) <= MAX_SCREENSHOT_EDGE {
        return png_bytes;
    }
    let fitted = decoded.resize(
        MAX_SCREENSHOT_EDGE,
        MAX_SCREENSHOT_EDGE,
        image::imageops::FilterType::Triangle,
    );
    let mut buf = Vec::new();
    if fitted
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .is_ok()
    {
        buf
    } else {
        png_bytes
    }
}

pub use click::{BrowserClickArgs, BrowserClickOutput, BrowserClickTool};
pub use console::{BrowserConsoleArgs, BrowserConsoleOutput, BrowserConsoleTool};
pub use cookies::{BrowserCookiesArgs, BrowserCookiesOutput, BrowserCookiesTool};
pub use dialog::{BrowserDialogArgs, BrowserDialogOutput, BrowserDialogTool};
pub use drag::{BrowserDragArgs, BrowserDragOutput, BrowserDragTool};
pub use emulate::{BrowserEmulateArgs, BrowserEmulateOutput, BrowserEmulateTool};
pub use evaluate::{BrowserEvaluateArgs, BrowserEvaluateOutput, BrowserEvaluateTool};
pub use fill_form::{BrowserFillFormArgs, BrowserFillFormOutput, BrowserFillFormTool};
pub use hover::{BrowserHoverArgs, BrowserHoverOutput, BrowserHoverTool};
pub use navigate::{BrowserNavigateArgs, BrowserNavigateOutput, BrowserNavigateTool};
pub use network::{BrowserNetworkArgs, BrowserNetworkOutput, BrowserNetworkTool};
pub use open::{BrowserOpenArgs, BrowserOpenOutput, BrowserOpenTool};
pub use pdf::{BrowserPdfArgs, BrowserPdfOutput, BrowserPdfTool};
pub use press_key::{BrowserPressKeyArgs, BrowserPressKeyOutput, BrowserPressKeyTool};
pub use profile_tool::{BrowserProfileArgs, BrowserProfileOutput, BrowserProfileTool};
pub use resize::{BrowserResizeArgs, BrowserResizeOutput, BrowserResizeTool};
pub use screenshot::{BrowserScreenshotArgs, BrowserScreenshotOutput, BrowserScreenshotTool};
pub use scroll::{BrowserScrollArgs, BrowserScrollOutput, BrowserScrollTool};
pub use select::{BrowserSelectArgs, BrowserSelectOutput, BrowserSelectTool};
pub use session::{BrowserSessionArgs, BrowserSessionOutput, BrowserSessionTool};
pub use snapshot::{BrowserSnapshotArgs, BrowserSnapshotOutput, BrowserSnapshotTool};
pub use tabs::{BrowserTabsArgs, BrowserTabsOutput, BrowserTabsTool};
pub use type_text::{BrowserTypeArgs, BrowserTypeOutput, BrowserTypeTool};
pub use upload::{BrowserUploadArgs, BrowserUploadOutput, BrowserUploadTool};
pub use wait_for::{BrowserWaitForArgs, BrowserWaitForOutput, BrowserWaitForTool};

/// Default browser profile name, used by serde `default` attributes across all browser tools.
pub(crate) fn default_profile() -> String {
    "default".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[test]
    fn parse_tab_line_handles_both_formats_and_annotations() {
        assert_eq!(
            parse_tab_line("1: https://example.com [selected]"),
            Some(("1".into(), "https://example.com".into()))
        );
        assert_eq!(
            parse_tab_line("Tab 2: http://10.0.0.1/x"),
            Some(("2".into(), "http://10.0.0.1/x".into()))
        );
        assert_eq!(
            parse_tab_line("3: about:blank"),
            Some(("3".into(), "about:blank".into()))
        );
        // Non-numeric / malformed lines are ignored.
        assert_eq!(parse_tab_line("no colon here"), None);
        assert_eq!(parse_tab_line("Tab x: http://a"), None);
    }

    #[test]
    fn parse_active_tab_id_picks_last() {
        let text = "1: https://a.com\n2: https://b.com\n3: https://c.com";
        assert_eq!(parse_active_tab_id(text).as_deref(), Some("3"));
        assert_eq!(parse_active_tab_id("").as_deref(), None);
    }

    #[test]
    fn extract_tab_url_matches_id() {
        let text = "1: https://a.com\n2: http://10.0.0.1/x [selected]";
        assert_eq!(
            extract_tab_url(text, "2").as_deref(),
            Some("http://10.0.0.1/x")
        );
        assert_eq!(extract_tab_url(text, "9"), None);
    }

    #[test]
    fn bound_content_keeps_short_text_intact() {
        let (out, truncated) = bound_content("a\nb\nc", 100);
        assert_eq!(out, "a\nb\nc");
        assert!(!truncated);
    }

    #[test]
    fn bound_content_truncates_on_line_boundary_without_splitting_refs() {
        // Three lines, each a snapshot element with a [ref=] token. A budget that
        // lands mid-third-line must cut back to the line boundary so no ref splits.
        let snap = "- button \"A\" [ref=e1]\n- button \"B\" [ref=e2]\n- button \"C\" [ref=e3]";
        // 30 chars lands inside the second line; expect only the first whole line.
        let (out, truncated) = bound_content(snap, 30);
        assert!(truncated);
        assert!(
            out.ends_with('\n'),
            "cut must land on a line boundary: {out:?}"
        );
        // Every emitted [ref=…] token is whole (balanced bracket).
        assert_eq!(out.matches("[ref=").count(), out.matches(']').count());
        // The ref count of the EMITTED text is what callers report to the model.
        assert_eq!(out.matches("[ref=").count(), 1);
    }

    #[test]
    fn bound_content_is_char_safe_on_multibyte() {
        // Budget falling between multibyte chars must never panic on a byte slice.
        let s = "héllo wörld 你好世界 café";
        let (out, truncated) = bound_content(s, 5);
        assert!(truncated);
        assert!(s.starts_with(&out) || out.is_empty());
    }

    #[test]
    fn bound_content_head_tail_keeps_short_text_intact() {
        let (out, truncated) = bound_content_head_tail("a\nb\nc", 100);
        assert_eq!(out, "a\nb\nc");
        assert!(!truncated);
    }

    #[test]
    fn bound_content_head_tail_preserves_newest_entries() {
        // 20 log lines, head-only would drop the tail — the head+tail split
        // must keep the FIRST and the LAST lines with a marker in between.
        let log: String = (1..=20)
            .map(|i| format!("[{i:02}] console line number {i}\n"))
            .collect();
        let (out, truncated) = bound_content_head_tail(&log, 200);
        assert!(truncated);
        assert!(out.starts_with("[01]"), "head dropped: {out:?}");
        assert!(
            out.contains("console line number 20"),
            "newest entry dropped: {out:?}"
        );
        assert!(out.contains("chars elided"), "missing marker: {out:?}");
        // Tail opens on a line boundary — the line right after the marker is whole.
        let after_marker = out.split("…\n").nth(1).expect("tail after marker");
        assert!(after_marker.starts_with('['), "tail mid-line: {out:?}");
    }

    #[test]
    fn bound_content_head_tail_is_char_safe_on_multibyte() {
        let s = "你好世界，这是一条很长的日志行。\n".repeat(50);
        let (out, truncated) = bound_content_head_tail(&s, 60);
        assert!(truncated);
        // No panic on byte slicing, and the tail still ends like the source.
        assert!(out.ends_with("。\n"));
    }

    fn png_of(w: u32, h: u32) -> Vec<u8> {
        use image::{DynamicImage, ImageFormat, RgbaImage};
        let img =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(w, h, image::Rgba([1, 2, 3, 255])));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        buf
    }

    #[test]
    fn bound_screenshot_downscales_oversized_capture() {
        // A full-page-style capture far over the edge cap is shrunk; the result
        // is still a valid PNG whose longest edge is within budget.
        let big = png_of(4000, 1000);
        let out = super::bound_screenshot_png(big.clone());
        assert_ne!(out, big, "oversized capture must be re-encoded smaller");
        let decoded = image::load_from_memory(&out).expect("still a valid png");
        assert!(decoded.width().max(decoded.height()) <= super::MAX_SCREENSHOT_EDGE);
    }

    #[test]
    fn bound_screenshot_leaves_within_budget_bytes_untouched() {
        // Already within the cap → returned byte-for-byte (no needless re-encode).
        let small = png_of(800, 600);
        let out = super::bound_screenshot_png(small.clone());
        assert_eq!(out, small);
    }

    #[test]
    fn bound_screenshot_passes_through_non_image_bytes() {
        // A decode failure must never drop the capture — bytes flow through.
        let junk = b"\x00\x01not a png".to_vec();
        assert_eq!(super::bound_screenshot_png(junk.clone()), junk);
    }

    #[tokio::test]
    async fn current_page_block_flags_internal_http_urls() {
        // Default browser SSRF policy blocks private/loopback/link-local.
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        crate::security::ssrf::dns::test_hook::clear();
        crate::security::ssrf::dns::test_hook::install({
            let mut m = std::collections::HashMap::new();
            m.insert("example.com".to_string(), vec!["8.8.8.8".parse().unwrap()]);
            m
        });

        // Cloud metadata endpoint reached via redirect → blocked.
        assert!(
            current_page_block(&manager, "1: http://169.254.169.254/latest/meta-data", "1")
                .await
                .is_some()
        );

        // Loopback → blocked.
        assert!(
            current_page_block(&manager, "1: http://127.0.0.1:9000/", "1")
                .await
                .is_some()
        );

        // Public URL → allowed.
        assert!(
            current_page_block(&manager, "1: https://example.com/", "1")
                .await
                .is_none()
        );

        // Non-http schemes carry no network target → skipped.
        assert!(
            current_page_block(&manager, "1: about:blank", "1")
                .await
                .is_none()
        );

        // No matching tab → nothing to check.
        assert!(
            current_page_block(&manager, "1: http://127.0.0.1/", "2")
                .await
                .is_none()
        );
        crate::security::ssrf::dns::test_hook::clear();
    }
}
