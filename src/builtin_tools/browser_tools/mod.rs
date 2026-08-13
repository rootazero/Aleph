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
pub mod exec;
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
use crate::browser::error::BrowserError;
use crate::browser::manager::ProfileManager;
use crate::browser::tab_registry;
use crate::security::content_sanitizer::{
    sanitize_external_text, wrap_external_content, ContentSource,
};

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
    // The prompt and the audit context get the *display* target, never the raw
    // one — see [`approval_display_target`]. Matching still runs against the
    // raw `target` below, so allow/blocklist patterns are unaffected.
    let display_target = approval_display_target(&action_type, action, target);
    let (agent_id, context) = crate::approval::audit_identity("browser", action, &display_target);
    let request = ActionRequest {
        action_type,
        target: target.to_string(),
        display_target,
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

/// The short, non-secret-bearing label a human (and, via the `Ask` prompt, the
/// model) is shown for a browser approval.
///
/// `ActionRequest::target` is the raw action payload: the cookie `name=value`
/// being written, the whole `browser_evaluate` script, the exact text being
/// typed into a form. `ConfigApprovalPolicy::check` falls back to `target` when
/// `display_target` is empty, so leaving it empty echoed all of that straight
/// back into model context — on the very call the gate exists to guard, and
/// unbounded. `pim` / `media` / `automation` all pass a display target for this
/// reason; browser is the last holdout.
///
/// Only the *presentation* narrows here. Allow/blocklist regexes keep matching
/// the real `target`, so no policy semantics change.
///
/// Navigation is the one action whose target the user genuinely needs to see,
/// so it keeps the URL — reduced to `scheme://host[:port]`, which drops the
/// query string where bearer tokens and session ids live. Anything that does
/// not parse as a URL is described by verb alone rather than echoed: the three
/// history verbs are literals we emit ourselves, and everything else is model-
/// supplied bytes we decline to reflect.
fn approval_display_target(action_type: &ActionType, action: &str, target: &str) -> String {
    if matches!(action_type, ActionType::BrowserNavigate) {
        return match url::Url::parse(target).map(|u| u.origin()) {
            Ok(origin) if origin.is_tuple() => {
                format!("navigate to {}", origin.ascii_serialization())
            }
            _ if matches!(target, "back" | "forward" | "refresh") => format!("navigate {target}"),
            _ => "navigate to a non-URL target".to_string(),
        };
    }
    format!("browser {action}")
}

/// Character budget for backend error text handed back to the model.
///
/// Errors are one-liners in the happy case, but `classify_stderr` embeds raw
/// playwright-cli stderr — which can carry a page-sized dump (a rejected
/// `evaluate` echoes the script, a navigation failure can echo the response
/// body). 2 000 chars is enough for a stack head and the failing message.
pub(crate) const MAX_BACKEND_ERROR_CHARS: usize = 2_000;

/// Egress chokepoint for **backend error text** — the fourth channel by which
/// page-adjacent bytes reach the model, alongside [`redact_and_wrap`],
/// [`redact_and_wrap_log`] and [`redact_wrap`].
///
/// `BrowserError` carries raw playwright-cli stderr verbatim, and every tool
/// `format!`s it into its `message` field. That text is neither ours nor
/// trusted: it can contain a credential the page put in a URL, and it is
/// unbounded. This bounds it (head+tail, so the *reason* at the end of a
/// stderr dump survives), then redacts secrets through the same
/// `ProfileManager` policy every content read uses.
///
/// Deliberately scrubbed with [`sanitize_external_text`] rather than fenced
/// with `wrap_external_content`: the result is interpolated into a sentence we
/// wrote (`"Snapshot failed: {…}"`), so a fence would open mid-line and the
/// two marker lines would no longer bracket a standalone block — and an
/// unbalanced fence is strictly worse than none. `sanitize_external_text` is
/// documented for exactly this case: the scrubbing without the ~150 bytes of
/// boundary.
pub(crate) fn backend_error_text(manager: &ProfileManager, err: &BrowserError) -> String {
    let raw = err.to_string();
    let (bounded, truncated) = bound_content_head_tail(&raw, MAX_BACKEND_ERROR_CHARS);
    let scrubbed = sanitize_external_text(&manager.redact_content(&bounded));
    if truncated {
        format!("{scrubbed} [backend error truncated to {MAX_BACKEND_ERROR_CHARS} chars]")
    } else {
        scrubbed
    }
}

/// Deterministic input-side secret gate for the form-input tools
/// (`browser_type` / `browser_fill_form` / `browser_select` / `browser_dialog`
/// prompt text) — the missing twin of the navigation-time
/// `block_secrets_in_url` scan. Without it a `Critical`-severity secret in the
/// model's context could be typed into a web form on an allowed host.
///
/// Callers run this BEFORE the approval check: deterministic policy beats
/// interactive approval (and is cheaper). Returns the refusal message when
/// `text` embeds a secret; the message names the matched rule but never echoes
/// the secret value itself back to the model.
pub(crate) fn check_input_secret_block(manager: &ProfileManager, text: &str) -> Option<String> {
    manager.check_input_secret(text).map(|rule| {
        format!(
            "Blocked: input embeds a secret ({rule}) — refusing to type a credential into the \
             page. If the user explicitly wants this, they can disable \
             browser.policy.block_secrets_in_input."
        )
    })
}

/// Returns `Some(violation)` if `tab_id`'s current http(s) URL is blocked by
/// the SSRF policy. Non-http schemes (`about:blank`, `chrome://`, …) carry no
/// network target and are skipped.
pub(crate) async fn current_page_block(
    manager: &ProfileManager,
    tabs_text: &str,
    tab_id: &str,
) -> Option<String> {
    let url = tab_registry::tab_url_for(tabs_text, tab_id)?;
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    manager.check_url(&url).await.err().map(|v| v.to_string())
}

/// Get the active tab from the backend, or return an error if none are open.
///
/// "Which tab is active" is answered in exactly one place —
/// [`tab_registry::active_tab_id`], which prefers the driver's own
/// `[selected]` marker and only falls back to last-listed when the listing
/// carries no marker at all. This module used to keep a second copy of that
/// question that answered it with `.next_back()` unconditionally, which
/// `browser_tabs {switch}` falsified: after a switch the selected tab is not
/// the last line, so the read-time SSRF re-check below could vet tab N while
/// the content read landed on tab M.
async fn get_active_tab(backend: &dyn BrowserBackend) -> Result<String, BrowserError> {
    let tabs_text = backend.list_tabs().await?;
    tab_registry::active_tab_id(&tabs_text)
        .ok_or_else(|| BrowserError::ActionFailed("No tabs open. Use browser_open first.".into()))
}

/// Create the appropriate backend for the given profile.
///
/// Single routing source: delegates to [`ProfileManager::get_backend`], which
/// owns the driver→backend mapping and headless resolution. Unknown profiles
/// yield [`BrowserError::ProfileNotFound`] — no silent fallback to a Managed
/// backend under a name the user never configured.
pub(crate) fn make_backend(
    manager: &ProfileManager,
    profile: &str,
) -> Result<Arc<dyn BrowserBackend>, BrowserError> {
    manager.record_activity(profile);
    manager.get_backend(profile)
}

/// Create the appropriate backend and resolve the active tab ID.
pub(crate) async fn make_backend_and_tab(
    manager: &ProfileManager,
    profile: &str,
) -> Result<(Arc<dyn BrowserBackend>, String), BrowserError> {
    let backend = make_backend(manager, profile)?;
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
) -> Result<(Arc<dyn BrowserBackend>, String), BrowserError> {
    let backend = make_backend(manager, profile)?;
    let tabs_text = backend.list_tabs().await?;
    let tab_id = tab_registry::active_tab_id(&tabs_text).ok_or_else(|| {
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
            // Name a lever that exists. "Read the page in smaller sections" was
            // not one: no browser tool takes an offset, and the dropped tail is
            // gone by the time the model reads this. What the callers of this
            // function (evaluate / cookies / tabs) can actually do is return
            // less from the action itself.
            "{wrapped}\n[content truncated to {DEFAULT_CONTENT_MAX_CHARS} chars; \
             the dropped tail is not recoverable — narrow the action so it \
             returns less]"
        )
    } else {
        wrapped
    }
}

/// Offload the FULL text of a page read to the tool-result store and return the
/// standard recovery footer (persist marker + `ctx_search` hint).
///
/// Truncating inside a tool is otherwise irreversible: the tail is dropped
/// *before* `tool_output` ingress ever sees the value, so the pipeline's own
/// "persist the pre-reduction original" contract (`tool_output::ingress`) cannot
/// apply — it only ever sees the already-cut text. Reusing the harness's own
/// spill pair (`ToolResultStore` + [`recovery_footer`]) rather than inventing a
/// second mechanism means the blob is indexed, so the model gets `ctx_search`
/// and not just a file path.
///
/// Redacted before it hits disk: everything persisted here is read back into
/// model context by `ctx_search` / `read_file`, so it must clear the same
/// secret-egress boundary the in-context copy clears. The injection fence is
/// deliberately NOT applied to the blob — `ctx_search` returns *excerpts*, and
/// an excerpt of a fenced blob carries at most one of the two marker lines; an
/// unbalanced fence is worse than none.
///
/// `tool_name` is the caller's, not a fixed one: the store is keyed by
/// `(tool_call_id, tool_name)` and every caller has its own call id, so a second
/// caller is a first writer of its own entry, not a second writer of someone
/// else's. `browser_exec` declined to offload for exactly that misreading and
/// told the model to "take a standalone browser_snapshot" instead — advice that
/// names a different page, since a procedure's later steps have moved on by the
/// time the model could act on it.
///
/// `None` when the store or the call id is unavailable (direct `tools.invoke`,
/// tests, non-gateway paths) — the caller then says so instead of pointing the
/// model at a file that does not exist.
pub(crate) fn offload_full_content(
    manager: &ProfileManager,
    tool_name: &str,
    full: &str,
) -> Option<String> {
    let call_id = crate::approval::current_tool_call_id()?;
    let store = crate::tools::result_store::global_tool_result_store()?;
    // Narrow to the session running this tool, because that is the scope
    // `ctx_search` resolves its own reads under; searching the unscoped handle
    // finds nothing. Same reasoning as `builtin_tools::ctx_search`.
    let store = match crate::tools::turn_context::current_session_key() {
        Some(session) => crate::tools::result_store::ToolResultStore::for_session(&store, session),
        None => store,
    };
    offload_content_to(&store, &call_id, manager, tool_name, full)
}

/// The store-facing half of [`offload_full_content`], split out so it can be
/// exercised against a temp-dir store — the process-wide store singleton is a
/// `OnceLock` and installing one from a test would leak into every other test in
/// the binary.
pub(crate) fn offload_content_to(
    store: &crate::tools::result_store::ToolResultStore,
    call_id: &str,
    manager: &ProfileManager,
    tool_name: &str,
    full: &str,
) -> Option<String> {
    let redacted = manager.redact_content(full);
    // Threshold 0: we already know the text overflowed the tool's own budget,
    // so persist unconditionally — same call shape the harness turn-spill uses.
    crate::tools::result_processing::recovery_footer(Some(store), call_id, tool_name, &redacted, 0)
        .map(|(footer, _)| footer)
}

pub(crate) fn process_evaluate_result(manager: &ProfileManager, raw: &str) -> serde_json::Value {
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

/// Maximum screenshot payload (bytes) handed back to the model. The edge cap
/// alone is not a byte budget: a dense capture (noise, dithered gradients,
/// photo-heavy pages) re-encodes to several MB even at 1568px, flooding the
/// provider request the same way an unbounded text read would. 5 MiB matches
/// openclaw's screenshot ceiling and sits comfortably under provider
/// per-image limits (Anthropic rejects images over ~5 MB on many endpoints).
pub(crate) const MAX_SCREENSHOT_BYTES: usize = 5 * 1024 * 1024;

/// Downscale a screenshot to fit BOTH budgets — longest edge
/// [`MAX_SCREENSHOT_EDGE`] px and payload [`MAX_SCREENSHOT_BYTES`] bytes —
/// re-encoding as PNG (the screenshot format contract every consumer —
/// base64 output and the vision bridge — assumes). Returns the input bytes
/// **unchanged** when the image is already within both caps (zero re-encode)
/// or when decoding/encoding fails: post-processing must never turn a
/// successful capture into a failure.
pub(crate) fn bound_screenshot_png(png_bytes: Vec<u8>) -> Vec<u8> {
    let Ok(decoded) = image::load_from_memory(&png_bytes) else {
        return png_bytes;
    };
    // Fast path: within both budgets → byte-for-byte passthrough.
    if decoded.width().max(decoded.height()) <= MAX_SCREENSHOT_EDGE
        && png_bytes.len() <= MAX_SCREENSHOT_BYTES
    {
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
        .is_err()
    {
        return png_bytes;
    }
    if buf.len() <= MAX_SCREENSHOT_BYTES {
        return buf;
    }
    // Still over the byte cap (dense content compresses poorly): iteratively
    // downscale the edge-capped image — 0.7 / 0.5 / 0.35 / 0.25 of its
    // dimensions, the 0.25 floor matching computer-use parity — re-encoding
    // until under the cap. If the floor is reached first, return the smallest
    // attempt: a slightly-over-budget screenshot beats dropping the capture.
    let mut smallest = buf;
    for scale in [0.7_f64, 0.5, 0.35, 0.25] {
        let w = (f64::from(fitted.width()) * scale).round().max(1.0) as u32;
        let h = (f64::from(fitted.height()) * scale).round().max(1.0) as u32;
        let shrunk = fitted.resize_exact(w, h, image::imageops::FilterType::Triangle);
        let mut attempt = Vec::new();
        if shrunk
            .write_to(
                &mut std::io::Cursor::new(&mut attempt),
                image::ImageFormat::Png,
            )
            .is_err()
        {
            break; // encode failure → fall through to the best attempt so far
        }
        if attempt.len() <= MAX_SCREENSHOT_BYTES {
            return attempt;
        }
        if attempt.len() < smallest.len() {
            smallest = attempt;
        }
    }
    smallest
}

pub use click::{BrowserClickArgs, BrowserClickOutput, BrowserClickTool};
pub use console::{BrowserConsoleArgs, BrowserConsoleOutput, BrowserConsoleTool};
pub use cookies::{BrowserCookiesArgs, BrowserCookiesOutput, BrowserCookiesTool};
pub use dialog::{BrowserDialogArgs, BrowserDialogOutput, BrowserDialogTool};
pub use drag::{BrowserDragArgs, BrowserDragOutput, BrowserDragTool};
pub use emulate::{BrowserEmulateArgs, BrowserEmulateOutput, BrowserEmulateTool};
pub use evaluate::{BrowserEvaluateArgs, BrowserEvaluateOutput, BrowserEvaluateTool};
pub use exec::{BrowserExecArgs, BrowserExecOutput, BrowserExecTool};
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

    /// The tool layer resolves the active tab through the same function the
    /// backends and the post-navigation audit use, so a `browser_tabs {switch}`
    /// cannot leave the read-time SSRF re-check vetting one tab while the read
    /// lands on another. (The parsing itself is covered where it lives, in
    /// `browser::tab_registry`; this pins the delegation, which is the part
    /// that was wrong here.)
    #[test]
    fn the_tool_layer_reads_the_selected_tab_not_the_last_line() {
        let switched = "1: https://a.com [selected]\n2: https://b.com";
        assert_eq!(
            tab_registry::active_tab_id(switched).as_deref(),
            Some("1"),
            "the marker wins over line order"
        );
        assert_eq!(
            tab_registry::tab_url_for(switched, "1").as_deref(),
            Some("https://a.com"),
            "and the URL the SSRF re-check reads belongs to that same tab"
        );
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

    /// Incompressible-noise PNG: solid-color fixtures deflate to nothing and
    /// would never exercise the byte budget. A xorshift PRNG keeps the test
    /// deterministic without pulling in a rand dependency.
    fn noise_png(size: u32) -> Vec<u8> {
        use image::{DynamicImage, ImageFormat, RgbaImage};
        let mut state: u32 = 0x1234_5678;
        let img = DynamicImage::ImageRgba8(RgbaImage::from_fn(size, size, |_, _| {
            // xorshift32
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            image::Rgba([
                (state & 0xff) as u8,
                ((state >> 8) & 0xff) as u8,
                ((state >> 16) & 0xff) as u8,
                255,
            ])
        }));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        buf
    }

    #[test]
    fn bound_screenshot_enforces_byte_cap_on_dense_capture() {
        // Noise does not compress: even after the 1568px edge clamp the
        // re-encoded PNG is ~10 MB, over the 5 MiB byte cap, so the iterative
        // downscale must kick in. (2000px keeps the fixture cheap while still
        // landing over the cap post-clamp.)
        let noisy = noise_png(2000);
        assert!(noisy.len() > super::MAX_SCREENSHOT_BYTES);
        let out = super::bound_screenshot_png(noisy);
        assert!(
            out.len() <= super::MAX_SCREENSHOT_BYTES,
            "dense capture must fit the byte budget: {} bytes",
            out.len()
        );
        let decoded = image::load_from_memory(&out).expect("still a valid png");
        assert!(decoded.width().max(decoded.height()) <= super::MAX_SCREENSHOT_EDGE);
    }

    #[tokio::test]
    async fn current_page_block_flags_internal_http_urls() {
        // Default browser SSRF policy blocks private/loopback/link-local.
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        let _scope = crate::security::ssrf::dns::test_hook::ResolverScope::install({
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
        assert!(current_page_block(&manager, "1: https://example.com/", "1")
            .await
            .is_none());

        // Non-http schemes carry no network target → skipped.
        assert!(current_page_block(&manager, "1: about:blank", "1")
            .await
            .is_none());

        // No matching tab → nothing to check.
        assert!(current_page_block(&manager, "1: http://127.0.0.1/", "2")
            .await
            .is_none());
    }

    // ---------------------------------------------------------------
    // The egress chokepoint itself: redact → fence, whole, on every path.
    // ---------------------------------------------------------------

    /// A page that renders a credential must not put it back into model
    /// context, and the untrusted region must be bracketed by BOTH marker
    /// lines — `split_external_fence` is the strict oracle for that (it refuses
    /// anything it cannot put back together byte-for-byte, so a half fence
    /// yields `None`).
    fn assert_masked_and_wholly_fenced(out: &str, secret: &str) {
        use crate::security::content_sanitizer::split_external_fence;
        assert!(!out.contains(secret), "secret reached the model: {out}");
        assert!(out.contains("[REDACTED:"), "no redaction marker: {out}");
        let fenced = split_external_fence(out).expect("exactly one well-formed fence");
        assert!(
            fenced.interior.contains("[REDACTED:"),
            "the page text must sit INSIDE the fence: {out}"
        );
    }

    const LEAKED_SECRET: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789";

    #[test]
    fn redact_wrap_masks_secrets_and_emits_a_whole_fence() {
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        let page = format!("- text \"API token: {LEAKED_SECRET}\" [ref=e1]");
        assert_masked_and_wholly_fenced(&redact_wrap(&manager, &page), LEAKED_SECRET);
    }

    #[test]
    fn redact_and_wrap_masks_secrets_and_emits_a_whole_fence_when_truncating() {
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        // Over the default budget, so the truncation footer is appended AFTER
        // the closing marker — the fence must still be whole and the footer
        // must sit outside it.
        let mut page = "- generic \"filler\" [ref=e0]\n".repeat(4_000);
        page.push_str(&format!("- text \"token {LEAKED_SECRET}\"\n"));
        let out = redact_and_wrap(&manager, &page);
        assert!(
            out.contains("content truncated to"),
            "expected a footer: {out}"
        );
        // The truncated head has no secret in it, so assert the fence shape on
        // a body that does.
        let short = format!("- text \"token {LEAKED_SECRET}\"");
        assert_masked_and_wholly_fenced(&redact_and_wrap(&manager, &short), LEAKED_SECRET);
        // And the footer is ours, not the page's: it must land outside the fence.
        use crate::security::content_sanitizer::split_external_fence;
        let fenced = split_external_fence(&out).expect("whole fence even with a footer");
        assert!(fenced.suffix.contains("content truncated to"));
    }

    #[test]
    fn redact_and_wrap_log_masks_secrets_and_emits_a_whole_fence() {
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        let line = format!("[error] auth failed for {LEAKED_SECRET}");
        assert_masked_and_wholly_fenced(&redact_and_wrap_log(&manager, &line), LEAKED_SECRET);
    }

    #[test]
    fn backend_error_text_bounds_and_redacts_stderr() {
        let manager = ProfileManager::new(BrowserSystemConfig::default());

        // Secrets in a stderr dump are the same egress as secrets in page text.
        let err = BrowserError::ActionFailed(format!("playwright: 401 for token {LEAKED_SECRET}"));
        let out = backend_error_text(&manager, &err);
        assert!(!out.contains("sk-ant-api03"), "secret in error text: {out}");
        assert!(out.contains("[REDACTED:"));

        // …and an unbounded dump is cut, keeping the tail where the reason is.
        let huge = BrowserError::ActionFailed(format!(
            "{}\nECONNREFUSED: the real reason\n",
            "stack frame noise\n".repeat(2_000)
        ));
        let out = backend_error_text(&manager, &huge);
        assert!(
            out.chars().count() < MAX_BACKEND_ERROR_CHARS * 2,
            "unbounded error text: {} chars",
            out.chars().count()
        );
        assert!(
            out.contains("ECONNREFUSED: the real reason"),
            "head+tail bounding must keep the reason: {out}"
        );
        assert!(out.contains("backend error truncated to"));
    }

    // ---------------------------------------------------------------
    // Approval prompts must not echo the payload back into context.
    // ---------------------------------------------------------------

    #[test]
    fn approval_display_target_never_echoes_the_payload() {
        // A cookie write, an eval script and typed text are all raw payloads.
        let cookie = "session=eyJhbGciOiJIUzI1NiJ9.super-secret-value";
        assert_eq!(
            approval_display_target(&ActionType::BrowserCookiesWrite, "cookies", cookie),
            "browser cookies"
        );
        let script = "fetch('/admin').then(r=>r.text())".repeat(2_000);
        let shown = approval_display_target(&ActionType::BrowserEvaluate, "evaluate", &script);
        assert_eq!(shown, "browser evaluate");
        assert!(!shown.contains("fetch("));
    }

    #[test]
    fn approval_display_target_keeps_navigation_origin_but_drops_the_query() {
        assert_eq!(
            approval_display_target(
                &ActionType::BrowserNavigate,
                "navigate",
                "https://example.com:8443/callback?access_token=abc123"
            ),
            "navigate to https://example.com:8443"
        );
        // The three history verbs are literals we emit ourselves.
        assert_eq!(
            approval_display_target(&ActionType::BrowserNavigate, "navigate", "back"),
            "navigate back"
        );
        // Anything else that is not a tuple origin is described, not echoed.
        let shown = approval_display_target(
            &ActionType::BrowserNavigate,
            "navigate",
            "javascript:fetch('/admin')",
        );
        assert!(!shown.contains("fetch("), "payload echoed: {shown}");
    }
}

/// Source-level census: a tool that consults the approval policy must be
/// *given* one at its production construction site.
///
/// The gate in [`check_browser_approval`] is opt-in — with no policy wired it
/// returns `None`, i.e. allow. That default is deliberate (tools built by
/// `new()` in tests must not need a policy), but it means a forgotten
/// `.with_approval_policy(..)` in the registry constructor produces a tool
/// whose gate exists only in its own unit tests. That is exactly what happened
/// to `browser_emulate` and `browser_session`: both grew a gate, both had
/// passing gate tests, and both shipped ungated because the two construction
/// sites were never updated. Nothing was red.
///
/// So the assertion is about the *effect* reaching production, not about the
/// gate being written: for every browser tool whose source calls
/// `check_browser_approval`, the registry constructor must construct it with
/// `.with_approval_policy`. The `pub mod` list is re-derived from this file's
/// own source, so a 27th browser tool reds by name here rather than silently
/// escaping the census (`no_catalog_entry_inlines_its_description` in
/// `builtin_registry::definitions` is the precedent for this shape).
#[cfg(test)]
mod approval_wiring_census {
    /// `(module name, module source)` for every file under this directory.
    ///
    /// `include_str!` needs literal paths, so this list cannot be globbed —
    /// which is precisely why the test below re-derives the expected set from
    /// `mod.rs`'s `pub mod` declarations and fails on any name missing here.
    const SOURCES: &[(&str, &str)] = &[
        ("exec", include_str!("exec.rs")),
        ("click", include_str!("click.rs")),
        ("console", include_str!("console.rs")),
        ("cookies", include_str!("cookies.rs")),
        ("dialog", include_str!("dialog.rs")),
        ("drag", include_str!("drag.rs")),
        ("emulate", include_str!("emulate.rs")),
        ("evaluate", include_str!("evaluate.rs")),
        ("fill_form", include_str!("fill_form.rs")),
        ("hover", include_str!("hover.rs")),
        ("navigate", include_str!("navigate.rs")),
        ("network", include_str!("network.rs")),
        ("open", include_str!("open.rs")),
        ("pdf", include_str!("pdf.rs")),
        ("press_key", include_str!("press_key.rs")),
        ("profile_tool", include_str!("profile_tool.rs")),
        ("resize", include_str!("resize.rs")),
        ("screenshot", include_str!("screenshot.rs")),
        ("scroll", include_str!("scroll.rs")),
        ("select", include_str!("select.rs")),
        ("session", include_str!("session.rs")),
        ("snapshot", include_str!("snapshot.rs")),
        ("tabs", include_str!("tabs.rs")),
        ("type_text", include_str!("type_text.rs")),
        ("upload", include_str!("upload.rs")),
        ("wait_for", include_str!("wait_for.rs")),
    ];

    /// This module's own source, used to derive the module list.
    const MOD_SOURCE: &str = include_str!("mod.rs");

    /// The registry constructor — the one production site that assembles these
    /// tools with their shared approval policy and vision bridge.
    const CONSTRUCTOR_SOURCE: &str =
        include_str!("../../executor/builtin_registry/builder/constructor/mod.rs");

    /// Everything before the first `#[cfg(test)]`.
    ///
    /// Not anchored to a line boundary: a Windows checkout is CRLF, so `\r` is
    /// stripped first and the token is matched bare. Anchoring on `"\n#[cfg(test)]\n"`
    /// silently matches nothing under CRLF, which turns the "production prefix"
    /// into the whole file and lets a test-module string literal satisfy the
    /// scan (CLAUDE.md §10 — this repo has shipped that bug twice).
    fn production_prefix(src: &str) -> String {
        let normalized = src.replace('\r', "");
        match normalized.find("#[cfg(test)]") {
            Some(i) => normalized[..i].to_string(),
            None => normalized,
        }
    }

    /// The struct a file implements `AlephTool` for, e.g. `BrowserClickTool`.
    fn tool_struct(src: &str) -> Option<String> {
        let idx = src.find("impl AlephTool for ")?;
        let rest = &src[idx + "impl AlephTool for ".len()..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        (!name.is_empty()).then_some(name)
    }

    #[test]
    fn every_browser_module_is_in_the_census() {
        let declared: Vec<&str> = production_prefix(MOD_SOURCE)
            .lines()
            .filter_map(|l| l.trim().strip_prefix("pub mod ")?.strip_suffix(';'))
            .map(|n| {
                SOURCES
                    .iter()
                    .find(|(m, _)| *m == n)
                    .unwrap_or_else(|| {
                        panic!(
                            "`pub mod {n};` is declared but absent from \
                             approval_wiring_census::SOURCES — add \
                             (\"{n}\", include_str!(\"{n}.rs\")) so the new tool is covered"
                        )
                    })
                    .0
            })
            .collect();
        assert!(
            declared.len() >= 26,
            "the module scan found only {} modules — the `pub mod` parse broke, \
             so this census is inspecting nothing",
            declared.len()
        );
    }

    #[test]
    fn every_gated_browser_tool_is_constructed_with_the_approval_policy() {
        let constructor = production_prefix(CONSTRUCTOR_SOURCE);
        let mut checked = 0usize;
        let mut ungated: Vec<String> = Vec::new();

        for (module, src) in SOURCES {
            let production = production_prefix(src);
            if !production.contains("check_browser_approval") {
                continue;
            }
            let Some(name) = tool_struct(&production) else {
                panic!("{module}.rs consults the approval policy but has no `impl AlephTool for`");
            };
            checked += 1;
            // The construction and the builder call sit on separate lines, so
            // the window has to span the whole `let … = X::new(..)…;` statement
            // — and it has to STOP at that statement's `;`. A fixed-size
            // character window instead reads into the NEXT tool's construction,
            // and since these are declared adjacently it happily finds that
            // neighbour's `.with_approval_policy` and passes. (Observed: with
            // a 400-char window, deleting the call from `browser_emulate` left
            // the census green because `browser_cookies` is the next line.)
            let Some(at) = constructor.find(&format!("{name}::new(")) else {
                panic!(
                    "{name} consults the approval policy but is never constructed in the \
                     registry constructor — either wire it or it is unreachable"
                );
            };
            let statement_end = constructor[at..]
                .find(';')
                .map_or(constructor.len(), |off| at + off);
            if !constructor[at..statement_end].contains(".with_approval_policy") {
                ungated.push(name);
            }
        }

        assert!(
            checked >= 15,
            "only {checked} gated tools found — the scan is not seeing production code"
        );
        assert!(
            ungated.is_empty(),
            "these browser tools consult the approval policy but are constructed without \
             one, so their gate is inert in production (it will only ever fire in their own \
             unit tests): {ungated:?}"
        );
    }
}
