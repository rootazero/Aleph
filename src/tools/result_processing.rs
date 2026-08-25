//! Pure helpers for applying the tool-result budget pipeline:
//! `compress → persist-if-large → truncate-if-small`.
//!
//! Consumed by the production `ScopedToolService::execute` path (Layer 2 of
//! the result-budget stack; the Phase-2 `ToolPipeline` decorator chain these
//! helpers were originally extracted for was deleted, this is the only home).
//!
//! Layering:
//! - `resolve_result_budget(name, explicit)` resolves the per-tool token
//!   budget. `read_file`-family tools always return `None` to break the
//!   read → marker → re-read → persist loop, even if a misconfigured tool
//!   declares its own budget.
//! - `apply_result_budget(...)` runs the reduce/persist/truncate cascade
//!   over a tool's text output and returns `ProcessedResult`.
//!
//! The *content-aware* half of the cleaning happens one step earlier, in
//! [`crate::tool_output::hygiene`], because it has to see the tool's structured
//! value while its text fields still carry real newlines — see that module for
//! why flattening first made both content-aware cleaners blind.

use std::path::PathBuf;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::context::budget::pressure::{chars_for_token_budget, estimate_tokens_smart};
use crate::context::retrieval::IndexOutcome;
use crate::session::events::ToolImage;
use crate::tools::result_store::{extract_persisted_ref, ToolResultStore};

const MAX_INLINE_IMAGE_BASE64_CHARS: usize = (20usize * 1024 * 1024).div_ceil(3) * 4;

/// Global default budget for tools that neither declare an explicit
/// `max_result_tokens` nor appear in the legacy name table. It descends from
/// the historical `MAX_TOOL_RESULT_TOKENS` constant, which lived in the
/// since-deleted `pipeline` module this one replaced.
pub const DEFAULT_RESULT_BUDGET_TOKENS: usize = 8_000;

/// Process-wide ceiling on every per-result budget, installed at boot from the
/// model's usable window (`turn_budget::budget_for_window`). Absent = no
/// ceiling, which is exactly today's behavior.
///
/// It lives here rather than as a `ToolService::execute` parameter on purpose:
/// that signature's callers are in `harness/agent/act.rs`, and that tree is over
/// its R10 line budget. A boot-installed ceiling costs the harness zero lines.
/// `IndistinguishableDefault`, and `reads_as` quotes what
/// [`result_budget_ceiling`] ACTUALLY falls back to — `usize::MAX`, i.e. no
/// ceiling at all. It is deliberately not the crate's `DEFAULT_RESULT_BUDGET_
/// TOKENS`: that constant is the per-result *default budget*, a different
/// number in a different role, and a diagnostic printing it here would tell an
/// operator reads are clamped to 8 000 tokens when in fact nothing is clamped.
///
/// ⚠️ This handle has TWO production ways to end up uninstalled and they read
/// identically:
///
/// 1. boot never called [`set_global_result_budget_ceiling`] (CLI one-shot,
///    tests, any deployment with no `context_budget_config`); and
/// 2. boot DID call it, with a large-window model's ceiling, and the setter
///    deliberately returned without installing — see its doc for why that is
///    the right behaviour.
///
/// Case 2 is a decline with a reason already written down, so it is the
/// clearest [`crate::capability::CapabilitySlot::decline`] candidate this batch
/// met. It is left for Task 14 on purpose: converting it stamps an outcome and
/// is a behaviour change, not a rewrite. ⚠️ Task 14's stated search shape is
/// "boot's conditional-install `else` arms" and this arm is NOT in boot — it is
/// an early `return` inside this library setter, one call away — so a walk of
/// boot's call sites will not find it.
static RESULT_BUDGET_CEILING: CapabilitySlot<usize> = CapabilitySlot::new(
    "tools/result-budget-ceiling",
    MissingSemantics::IndistinguishableDefault {
        reads_as: "usize::MAX — uncapped, byte-for-byte the pre-ceiling behaviour",
    },
);

/// Install the process-wide per-result ceiling. Called once at boot.
///
/// A ceiling at or above [`DEFAULT_RESULT_BUDGET_TOKENS`] is **ignored**: it
/// would clip the budgets tools declare above the default (`web_fetch`'s 10k)
/// without buying anything, and this knob exists solely to clamp *down* on
/// small-window models. So a large-window model installs nothing and behaves
/// byte-for-byte as it does today.
pub fn set_global_result_budget_ceiling(ceiling: usize) {
    if ceiling >= DEFAULT_RESULT_BUDGET_TOKENS {
        return;
    }
    let _ = RESULT_BUDGET_CEILING.install(ceiling);
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn result_budget_ceiling_slot() -> &'static dyn SlotStatus {
    &RESULT_BUDGET_CEILING
}

/// The installed ceiling, or `usize::MAX` (= uncapped) when boot installed none.
///
/// ⚠️ That `usize::MAX` is a legal value, not a signal: it is what a
/// large-window model's deployment is *supposed* to see, and it is also what a
/// boot that died before this line leaves behind. Ask
/// [`result_budget_ceiling_slot`]`().outcome()` to tell the two apart; this
/// function cannot and must not try.
fn result_budget_ceiling() -> usize {
    RESULT_BUDGET_CEILING.get().copied().unwrap_or(usize::MAX)
}

/// The token bound a read-family result is actually enforced against — the
/// global default, clamped by the boot-installed window ceiling.
///
/// Exposed because `file_read` sizes its own window to stay under this. Reading
/// the constant alone is not enough: on a small-window model the ceiling moves
/// the bound down to as little as 2 000 tokens, and a producer that ignored it
/// would hand the generic truncator a window to cut a hole in — the exact bug the
/// self-sizing exists to prevent.
#[must_use]
pub(crate) fn read_backstop_tokens() -> usize {
    DEFAULT_RESULT_BUDGET_TOKENS.min(result_budget_ceiling())
}

/// Resolve a tool's per-result token budget.
///
/// Lookup order:
/// 1. `read_file` / `Read` / `file_read` always return `None` (system
///    invariant — a `read_file` result is the only way the model can pull
///    a persisted marker file back into context, so persisting one would
///    create a loop).
/// 2. `explicit` (typically the tool's own `max_result_tokens()` value)
///    wins for every other name. Builtins declare their budget there now
///    (`bash`, `web_fetch`), so they never reach the table below.
/// 3. Otherwise a single remaining legacy entry (`search_files`/`Grep`,
///    which has no in-crate tool to carry the trait method).
/// 4. Otherwise fall back to [`DEFAULT_RESULT_BUDGET_TOKENS`].
///
/// Whatever that yields is then capped by the boot-installed window ceiling
/// (see [`set_global_result_budget_ceiling`]). The cap applies to *every*
/// branch, not just the fallback: a declared 10k budget on a 16k-window model is
/// exactly the value that has to come down, so treating the ceiling as a default
/// rather than a maximum would let the worst offenders through untouched.
///
/// `None` from this function means "do not persist this tool's output;
/// just truncate when it exceeds the global default".
#[must_use]
pub fn resolve_result_budget(name: &str, explicit: Option<usize>) -> Option<usize> {
    resolve_result_budget_under(name, explicit, result_budget_ceiling())
}

/// Pure core of [`resolve_result_budget`] with the ceiling passed in, so the
/// cap semantics are unit-testable without touching the process-wide slot.
fn resolve_result_budget_under(
    name: &str,
    explicit: Option<usize>,
    ceiling: usize,
) -> Option<usize> {
    match name {
        "read_file" | "Read" | "file_read" => return None,
        _ => {}
    }
    // Tools whose `AlephTool::max_result_tokens()` never reaches this function
    // because they are registered through the executor `ToolRegistry` →
    // `RegistryToolAdapter` (which does not carry the trait value). Their budget
    // stays here until that adapter forwards declared budgets. `bash` (8k) ==
    // the default, so only the non-default ones need arms.
    let declared = explicit.or(match name {
        "Grep" | "search_files" => Some(6_000),
        "web_fetch" => Some(10_000),
        _ => Some(DEFAULT_RESULT_BUDGET_TOKENS),
    });
    declared.map(|n| n.min(ceiling))
}

/// Output of [`apply_result_budget`]. `text` is what the LLM should see.
/// `persisted_path` is `Some(path)` iff the original text was offloaded
/// to disk via `ToolResultStore::persist_if_large`.
#[derive(Debug, Clone)]
pub struct ProcessedResult {
    pub text: String,
    pub tokens_in_context: usize,
    pub persisted_path: Option<PathBuf>,
}

/// Apply Layer 2 of the budget pipeline to a successful tool output.
///
/// Caller is responsible for any tool-specific compression (e.g.
/// `compress_tool_output`) and for the field-wise ingress hygiene pass
/// ([`crate::tool_output::hygiene::clean_result_value`]) before invoking this
/// helper; this layer decides between "keep verbatim", "persist + marker", and
/// "truncate".
///
/// `reduced_from` carries the **untouched original** when hygiene shortened
/// `text`. Two things hang off it:
///
/// 1. The original — not the reduced body — is what gets persisted, so the lines
///    the reducer dropped stay recoverable via `ctx_search` / `read_file`.
///    Persisting the reduced copy would make the reduction irreversible.
/// 2. It is the signal that we *know what the content was*. Only then is the
///    reduced body inlined above the recovery marker: a type-routed reduction is
///    signal-dense by construction, so handing it to the model directly saves the
///    `ctx_search` round-trip (an extra LLM turn that re-sends the whole
///    context). Opaque output keeps the marker-only behaviour, because there we
///    cannot tell signal from noise and a head/tail slice would be a guess.
pub fn apply_result_budget(
    tool_call_id: &str,
    tool_name: &str,
    text: &str,
    store: Option<&ToolResultStore>,
    budget: Option<usize>,
    reduced_from: Option<&str>,
) -> ProcessedResult {
    let tokens = estimate_tokens_smart(text);
    let Some(budget) = budget else {
        // Budget = None ⟺ the read-file family (see `resolve_result_budget`).
        // A read result is *the exact lines the model asked for*, so it is only
        // ever kept verbatim or head/tail-truncated — never semantically
        // re-selected. Distilling here used to replace a large source file with
        // a grep of its "error"-looking lines (`pub enum Error {` lowercases to
        // a hit on the `"error "` marker), silently answering a different
        // question than the one asked. `file_read` sizes its own window under
        // this threshold, so this branch is now a backstop rather than a path.
        //
        // The boot-installed window ceiling applies here too. It used to be
        // bypassed on this branch, which handed a 2 400-token-window model an
        // 8 000-token read allowance — the one case the knob exists to prevent.
        let truncated = truncate_with_budget(text, read_backstop_tokens());
        let tokens_after = estimate_tokens_smart(&truncated);
        return ProcessedResult {
            text: truncated,
            tokens_in_context: tokens_after,
            persisted_path: None,
        };
    };

    // Only meaningful when hygiene actually changed something.
    let original = reduced_from.filter(|orig| *orig != text);

    if tokens <= budget {
        // Fits. Offload the untouched original when detail was dropped getting
        // here, so the reduction stays reversible; otherwise keep it verbatim.
        let Some(full) = original else {
            return ProcessedResult {
                text: text.to_string(),
                tokens_in_context: tokens,
                persisted_path: None,
            };
        };
        return match recovery_footer(store, tool_call_id, tool_name, full, budget) {
            Some((footer, path)) => {
                let body = format!("{text}\n{footer}");
                ProcessedResult {
                    tokens_in_context: estimate_tokens_smart(&body),
                    text: body,
                    persisted_path: path,
                }
            }
            None => ProcessedResult {
                text: text.to_string(),
                tokens_in_context: tokens,
                persisted_path: None,
            },
        };
    }

    // Over budget. Persist the original (or `text` when there was no hygiene
    // pass) and compose the inline body above the recovery footer.
    let persist_source = original.unwrap_or(text);
    if let Some((footer, path)) =
        recovery_footer(store, tool_call_id, tool_name, persist_source, budget)
    {
        let footer_tokens = estimate_tokens_smart(&footer);
        let body = match original {
            // Content-typed: inline the signal, sized so body + footer still
            // respect the tool's declared budget. `distill_or_truncate` rather
            // than a blind head/tail cut — a reduction that is *still* over budget
            // is usually a wall of diagnostics, and the middle is where the
            // failure is named.
            Some(_) => distill_or_truncate(text, budget.saturating_sub(footer_tokens)),
            // Opaque: a bounded error preview only, as before — visible without
            // a ctx_search round-trip, absent when there is no error signal.
            None => inline_error_digest(text, Some(budget)).unwrap_or_default(),
        };
        let composed = if body.is_empty() {
            footer
        } else {
            format!("{body}\n{footer}")
        };
        return ProcessedResult {
            tokens_in_context: estimate_tokens_smart(&composed),
            text: composed,
            persisted_path: path,
        };
    }

    // No store, or the persist failed (the store logs internally) — truncate.
    let truncated = distill_or_truncate(text, budget);
    let tokens_after = estimate_tokens_smart(&truncated);
    ProcessedResult {
        text: truncated,
        tokens_in_context: tokens_after,
        persisted_path: None,
    }
}

/// Offload `full` to the result store and build the recovery footer the model
/// uses to get the dropped detail back: the persist marker plus, when the blob
/// indexed into sections, a `ctx_search` hint.
///
/// `None` when there is no store or the persist did not happen (content under
/// `threshold`, or a write failure) — the caller then falls back to truncation.
///
/// `pub(crate)` for the harness Layer-3 turn spill (`harness/agent/act.rs`),
/// which offloads for the same reason and must hand the model the same recovery
/// handle. It used to call `persist_if_large` directly and so emitted a marker
/// with **no** `ctx_search` hint over a blob that was never indexed — the model
/// was pointed at a file it could only re-read whole, defeating the offload.
pub(crate) fn recovery_footer(
    store: Option<&ToolResultStore>,
    tool_call_id: &str,
    tool_name: &str,
    full: &str,
    threshold: usize,
) -> Option<(String, Option<PathBuf>)> {
    let store = store?;
    let marker = store.persist_if_large(tool_call_id, tool_name, full, threshold)?;
    let path = extract_persisted_ref(&marker).and_then(parse_marker_path);
    // Index the offloaded blob so the model can BM25-retrieve only the relevant
    // slices via `ctx_search` instead of re-reading the whole file (which would
    // defeat the offload). Best-effort: on failure the bare persist marker still
    // lets the model `read_file` it back.
    let indexed = store.index_output(tool_call_id, tool_name, full);
    let footer = match indexed.filter(|o| o.sections > 0) {
        Some(outcome) => format!("{marker}\n{}", search_hint(&outcome)),
        None => marker,
    };
    Some((footer, path))
}

/// Rescue inline image payloads from a structured tool-result value into the
/// out-of-band [`ToolImage`] channel, BEFORE the value is flattened to text and
/// truncated by the result budget.
///
/// Without this, a `desktop` screenshot's base64 (often megabytes) is
/// stringified into the tool-result text, blows the token budget, and is
/// truncated into an undecodable fragment — so the vision-capable model never
/// actually *sees* the screen it just acted on. Here we lift the base64 out,
/// replace it in the text channel with a short marker (keeping the surrounding
/// metadata: size, format, OCR text), and return the images for re-emission as
/// `ContentBlock::Image` when the tool result is rendered into the prompt.
///
/// Targets two shapes:
///
/// - `{ image_base64, format, .. }` — Aleph's own, whether at the top level (a
///   `desktop` screenshot, or a `file_read` of an image file) or nested under a
///   `data` wrapper (`DesktopOutput { data }`);
/// - `{ content: [ { type: "image", data, mimeType }, … ] }` — the MCP tool
///   result shape (`mcp/external/connection.rs::call_tool`). Every
///   browser-automation and screenshot MCP server returns images this way, and
///   until the adapter stopped pre-serializing its result there was nothing here
///   to recognize: the base64 arrived already stringified inside a JSON
///   envelope, got counted against the result budget, and was truncated into an
///   undecodable fragment. The model acted on a screen it never saw.
///
/// Non-matching values are left untouched, so this is a no-op for the ~all tool
/// calls that produce no image.
#[must_use]
pub fn hoist_inline_images(value: &mut serde_json::Value) -> Vec<ToolImage> {
    let mut images = Vec::new();
    hoist_walk(value, &mut images, 0);
    images
}

/// Recursion bound for [`hoist_walk`]. Tool results are serialized from Rust
/// structs or MCP payloads — both shallow — so this only guards against
/// pathologically deep JSON (e.g. a page that smuggled a nested document into
/// an `evaluate` result).
const MAX_HOIST_DEPTH: usize = 16;

/// Walk the whole result tree, applying both extractors at every object node.
///
/// This used to check exactly two positions — the top level and a `data`
/// child — because the producers then known (`desktop` screenshot, `file_read`
/// of an image) both placed the payload there. `browser_exec`'s `screenshot`
/// step broke that shape: its image arrives nested inside `results[]`, one
/// object per step, so a procedure's screenshot stayed in the text channel and
/// the result budget shredded it — the exact failure this function exists to
/// prevent, one level down. The walk is still bounded twice over:
/// [`MAX_HOISTED_IMAGES`] caps how much leaves the text channel (overflow keeps
/// its base64 in place), and the per-payload size guard inside the extractors
/// caps each one. Extraction replaces the payload with a short marker before
/// the descent, so a node is never hoisted twice.
fn hoist_walk(value: &mut serde_json::Value, out: &mut Vec<ToolImage>, depth: usize) {
    if out.len() >= MAX_HOISTED_IMAGES || depth >= MAX_HOIST_DEPTH {
        return;
    }
    if value.is_object() {
        extract_image_in_place(value, out);
        extract_mcp_content_images(value, out);
    }
    match value {
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                hoist_walk(v, out, depth + 1);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                hoist_walk(item, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// Cap on images lifted out of one tool result.
///
/// A tool that returns a page of thumbnails would otherwise attach dozens of
/// image blocks to a single request — each one billed in full, and none of them
/// individually over the size guard. Overflow keeps its base64 in the text
/// channel, where the result budget bounds it as usual.
const MAX_HOISTED_IMAGES: usize = 4;

/// Lift `{"type":"image","data":…,"mimeType":…}` blocks out of an MCP result's
/// `content` array, replacing each payload with the same short marker the
/// single-image path uses (which is also what makes a second pass a no-op).
fn extract_mcp_content_images(value: &mut serde_json::Value, out: &mut Vec<ToolImage>) {
    let Some(blocks) = value
        .get_mut("content")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    for block in blocks {
        if out.len() >= MAX_HOISTED_IMAGES {
            return;
        }
        let Some(obj) = block.as_object_mut() else {
            continue;
        };
        if obj.get("type").and_then(serde_json::Value::as_str) != Some("image") {
            continue;
        }
        let data = match obj.get("data").and_then(serde_json::Value::as_str) {
            Some(s) if s.len() > 256 && s.len() <= MAX_INLINE_IMAGE_BASE64_CHARS => s.to_string(),
            _ => continue,
        };
        // The server names the media type directly, but it is untrusted input:
        // only the types the providers actually accept are forwarded, and the
        // rest keep their base64 in the text channel rather than being handed to
        // a provider that will reject the whole request.
        let Some(mime_type) = supported_image_mime(obj.get("mimeType").and_then(|m| m.as_str()))
        else {
            continue;
        };
        let chars = data.len();
        out.push(ToolImage { data, mime_type });
        obj.insert(
            "data".to_string(),
            serde_json::Value::String(format!(
                "<{chars} base64 chars returned to the model as a viewable image block>"
            )),
        );
    }
}

/// The media types Aleph forwards as image blocks, from a MIME string.
fn supported_image_mime(mime: Option<&str>) -> Option<String> {
    let mime = mime?.trim().to_ascii_lowercase();
    matches!(
        mime.as_str(),
        "image/png" | "image/jpeg" | "image/webp" | "image/gif" | "image/avif"
    )
    .then_some(mime)
}

/// Extract a single `{ image_base64, format }` payload from an object in place,
/// replacing the base64 with a short marker. No-op for non-objects or objects
/// without a substantial `image_base64` string. The `> 256` guard also makes
/// this idempotent — the marker left behind is far shorter, so a second pass
/// never re-hoists it.
fn extract_image_in_place(value: &mut serde_json::Value, out: &mut Vec<ToolImage>) {
    // The recursive walk can reach many image-bearing objects in one result;
    // overflow keeps its base64 in the text channel (see MAX_HOISTED_IMAGES).
    if out.len() >= MAX_HOISTED_IMAGES {
        return;
    }
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    // Read phase: these immutable borrows end before the mutation below.
    let data = match obj.get("image_base64").and_then(serde_json::Value::as_str) {
        Some(s) if s.len() > 256 && s.len() <= MAX_INLINE_IMAGE_BASE64_CHARS => s.to_string(),
        _ => return,
    };
    let mime_type = match obj.get("format").and_then(serde_json::Value::as_str) {
        Some("png") => "image/png",
        Some("jpeg" | "jpg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("avif") => "image/avif",
        _ => return,
    }
    .to_string();
    let chars = data.len();
    out.push(ToolImage { data, mime_type });
    obj.insert(
        "image_base64".to_string(),
        serde_json::Value::String(format!(
            "<{chars} base64 chars returned to the model as a viewable image block>"
        )),
    );
}

/// Build the model-facing hint appended to a persist marker when the output
/// was also indexed for retrieval. Tells the model it can `ctx_search` the
/// offloaded blob instead of re-reading the whole file, and lists the first
/// few section titles as orientation. Kept to a few hundred bytes so the
/// offload's token saving is preserved.
fn search_hint(outcome: &IndexOutcome) -> String {
    // With a single section the "First sections:" preview is the head of the one
    // section — i.e. text the model already has immediately above this hint.
    // Orientation is only worth its bytes when there is something to choose
    // between.
    let preview = if outcome.sections > 1 {
        outcome.previews.join(" · ")
    } else {
        String::new()
    };
    if preview.is_empty() {
        format!(
            "[Indexed {} sections — use ctx_search(query=\"…\") to retrieve only \
             the relevant parts instead of re-reading the whole file]",
            outcome.sections
        )
    } else {
        format!(
            "[Indexed {} sections — use ctx_search(query=\"…\") to retrieve only the \
             relevant parts instead of re-reading the whole file. First sections: {}]",
            outcome.sections, preview
        )
    }
}

/// Reduce over-budget text to a salient digest when it carries error / path
/// signal, otherwise fall back to head+tail [`truncate_with_budget`].
///
/// This is the "only the key errors, paths, context" path: for command / log
/// output whose real signal sits in the *middle* of the stream (compile
/// errors, panics, failing assertions), head+tail truncation drops exactly
/// that middle. [`distill_output`](crate::tool_output::distill::distill_output)
/// extracts it locally. The digest is preferred only when it both carries an
/// error and fits the budget; signal-free output still truncates as before.
fn distill_or_truncate(text: &str, budget_tokens: usize) -> String {
    if let Some(digest) = crate::tool_output::distill::distill_output(text) {
        if digest.error_count > 0 {
            let cap = crate::tool_output::scale_to_budget(
                crate::tool_output::distill::MAX_SALIENT_LINES,
                crate::tool_output::hygiene::MIN_SALIENT_LINES,
                budget_tokens,
            );
            let rendered = digest.render(cap);
            if estimate_tokens_smart(&rendered) <= budget_tokens {
                return rendered;
            }
        }
    }
    truncate_with_budget(text, budget_tokens)
}

/// Inline error preview prepended to a persist marker, so the model sees the
/// key failures immediately instead of having to `ctx_search` the offloaded
/// blob first. Returns `None` when there is no error signal. Bounded to a
/// handful of lines to preserve the offload's token saving — exactly how
/// handful is budget-derived: 8 lines at the default result budget, scaled
/// down (floor 2 — fewer and the preview stops naming the failure) for a tool
/// that declared a tighter budget, never up.
fn inline_error_digest(text: &str, budget_tokens: Option<usize>) -> Option<String> {
    // A payload with no newline at all cannot be line-distilled — a flattened
    // tool envelope is exactly one line, and a prefix slice of it is a guess
    // dressed up as a signal. That precondition now lives on
    // [`distill_output`](crate::tool_output::distill::distill_output) itself, so
    // this arm and `tool_output::hygiene`'s tier-2 cannot disagree about it; the
    // recovery marker stands alone instead. Typed results get their signal
    // inlined through the other arm, where hygiene walked the value field by
    // field and kept the line shape intact.
    let digest = crate::tool_output::distill::distill_output(text)?;
    if digest.error_count == 0 {
        return None;
    }
    let cap = budget_tokens.map_or(8, |b| crate::tool_output::scale_to_budget(8, 2, b));
    Some(digest.render(cap))
}

/// Head + tail truncation under the budget.
#[must_use]
pub fn truncate_with_budget(text: &str, budget_tokens: usize) -> String {
    let estimated = estimate_tokens_smart(text);
    if estimated <= budget_tokens {
        return text.to_string();
    }
    // Content-aware char budget: invert `estimate_tokens_smart`'s own
    // chars-per-token ratio so the kept head+tail lands at ~budget_tokens for
    // CJK / code / prose alike. The prior fixed 4-chars/token assumption
    // diverged from the CJK/code-aware estimator — dense code/log output (the
    // common Bash-result case) stayed ~1.6x over budget, while CJK conflated
    // char counts with byte offsets. All slicing is on exact char counts via
    // `char_byte_offset`, so there is no char/byte unit mixing. Keep ~70 %
    // head + 30 % tail.
    let total_chars = text.chars().count();
    let target_chars = chars_for_token_budget(text, budget_tokens);
    if target_chars >= total_chars {
        return text.to_string();
    }
    let head_chars = target_chars.saturating_mul(7) / 10;
    let tail_chars = target_chars.saturating_sub(head_chars);

    let head_end = char_byte_offset(text, head_chars);
    let tail_start = char_byte_offset(text, total_chars.saturating_sub(tail_chars)).max(head_end);

    let omitted = estimated.saturating_sub(budget_tokens);
    format!(
        "{}\n... [output truncated, ~{} tokens omitted] ...\n{}",
        &text[..head_end],
        omitted,
        &text[tail_start..]
    )
}

/// Byte offset where the `n`-th char starts, clamped to `text.len()`. Lets the
/// truncator slice on exact char counts without ever mixing char and byte
/// units (the bug the old `floor`/`ceil_char_boundary` byte-index helpers hid).
fn char_byte_offset(text: &str, n: usize) -> usize {
    text.char_indices()
        .nth(n)
        .map_or(text.len(), |(byte_idx, _)| byte_idx)
}

fn parse_marker_path(line: &str) -> Option<PathBuf> {
    // Marker format: "[Full output persisted: <path> (<n> tokens, <tool>)]".
    let prefix = "[Full output persisted: ";
    let start = line.find(prefix)? + prefix.len();
    let rest = &line[start..];
    let end = rest.find(" (")?;
    Some(PathBuf::from(rest[..end].to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ========================================================================
    // The process-global handle, as a capability slot
    // ========================================================================

    /// See `session::service::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    ///
    /// The `reads_as` half is the part with teeth. This is the only
    /// `IndistinguishableDefault` in the batch, its sentence is what a
    /// diagnostic prints verbatim when `outcome()` is `None`, and the brief's
    /// table shipped a PLACEHOLDER for it (`"<compiled-in default ceiling>"`),
    /// which would have pointed a reader at `DEFAULT_RESULT_BUDGET_TOKENS` —
    /// a real constant, in a different role, that is not what an uninstalled
    /// read yields. So this asserts the sentence names the actual fallback and
    /// is not empty: a slot that lost it would still report an id and still
    /// look fine.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = result_budget_ceiling_slot();
        assert_eq!(slot.id(), "tools/result-budget-ceiling");
        match slot.missing() {
            MissingSemantics::IndistinguishableDefault { reads_as } => {
                assert!(
                    reads_as.contains("usize::MAX"),
                    "the sentence a diagnostic prints must name what \
                     `result_budget_ceiling()` really falls back to, got: \
                     {reads_as:?}"
                );
            }
            other => panic!("expected IndistinguishableDefault, got {other:?}"),
        }
    }

    /// A flattened builtin result must not get a fake "error preview".
    ///
    /// `Value::to_string()` puts the whole envelope on one line, so the
    /// line-oriented distiller saw exactly one "line", matched `"error"`
    /// somewhere inside the JSON, and presented a char-capped prefix of the
    /// envelope — `{"success":false,…` — under an `[Output digest: 1 lines, 1
    /// error]` header. The compiler errors and panic messages the preview
    /// exists to surface were all past the cap.
    #[test]
    fn a_flattened_envelope_gets_no_inline_error_preview() {
        let flat = serde_json::json!({
            "success": false,
            "exit_code": 101,
            "stdout": format!("running 2001 tests\n{}", "test foo ... ok\n".repeat(400)),
            "stderr": "error[E0308]: mismatched types\n  --> src/main.rs:4:9",
        })
        .to_string();
        assert!(!flat.contains('\n'), "the flattened envelope is one line");
        assert_eq!(
            inline_error_digest(&flat, None),
            None,
            "an opaque single-line payload cannot be line-distilled; the preview \
             would be the JSON envelope's head, not the error"
        );
    }

    /// The line-shaped case — a bare MCP text result — still gets its preview.
    #[test]
    fn a_line_shaped_payload_still_gets_its_error_preview() {
        let text = format!(
            "running 2001 tests\n{}error[E0308]: mismatched types\n  --> src/main.rs:4:9\n",
            "test foo ... ok\n".repeat(400)
        );
        let digest = inline_error_digest(&text, None).expect("line-shaped output distills");
        assert!(digest.contains("error[E0308]"), "got: {digest}");
    }

    /// The preview's line cap is a budget knob, not a constant: the default
    /// budget reproduces the historical 8 lines exactly, a tighter budget
    /// shrinks it, and the floor keeps it from shrinking past usefulness.
    #[test]
    fn the_error_preview_scales_with_the_budget() {
        let mut text = String::from("running 2001 tests\n");
        for i in 0..30 {
            text.push_str(&format!("error: failure number {i} at f{i}.rs:1\n"));
        }
        text.push_str(&"padding to exceed the distiller's size floor\n".repeat(40));

        let count_errors =
            |digest: &str| digest.lines().filter(|l| l.starts_with("error:")).count();
        let default_budget =
            inline_error_digest(&text, Some(DEFAULT_RESULT_BUDGET_TOKENS)).expect("distills");
        assert_eq!(
            count_errors(&default_budget),
            8,
            "the default budget reproduces the shipped 8-line cap:\n{default_budget}"
        );
        let no_budget = inline_error_digest(&text, None).expect("distills");
        assert_eq!(no_budget, default_budget, "None is the default behaviour");
        let tight = inline_error_digest(&text, Some(300)).expect("distills");
        let tight_n = count_errors(&tight);
        assert!(
            tight_n < 8,
            "a 300-token budget must tighten, got {tight_n}"
        );
        assert!(
            tight_n >= 2,
            "the floor keeps the preview useful, got {tight_n}"
        );
    }

    fn test_store(_name: &str) -> (tempfile::TempDir, ToolResultStore, PathBuf) {
        let (scratch, base) = crate::utils::scratch::scratch_root();
        std::fs::create_dir_all(&base).unwrap();
        let store = ToolResultStore::with_dir_for_tests(base.clone());
        (scratch, store, base)
    }

    // ---------------------------------------------------------------
    // hoist_inline_images — the perceive→act vision loop
    // ---------------------------------------------------------------

    #[test]
    fn hoists_desktop_screenshot_into_out_of_band_channel() {
        // Desktop screenshot shape: image nested under `data`.
        let big = "A".repeat(5000); // > 256 → a real image, not a marker
        let mut value = serde_json::json!({
            "success": true,
            "data": {
                "image_base64": big,
                "width": 1920,
                "height": 1080,
                "format": "png",
            }
        });
        let images = hoist_inline_images(&mut value);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].data.len(), 5000);
        assert_eq!(images[0].mime_type, "image/png");
        // Base64 is elided from the text channel (the budget-blowing blob is gone).
        let elided = value["data"]["image_base64"].as_str().unwrap();
        assert!(elided.len() < 256);
        // Surrounding metadata is preserved for the model to read.
        assert_eq!(value["data"]["width"], 1920);
    }

    #[test]
    fn hoist_maps_common_image_formats() {
        for (format, mime_type) in [
            ("png", "image/png"),
            ("jpeg", "image/jpeg"),
            ("jpg", "image/jpeg"),
            ("webp", "image/webp"),
            ("gif", "image/gif"),
            ("avif", "image/avif"),
        ] {
            let mut value = serde_json::json!({
                "image_base64": "A".repeat(400),
                "format": format,
            });

            let images = hoist_inline_images(&mut value);

            assert_eq!(images.len(), 1);
            assert_eq!(images[0].mime_type, mime_type);
        }
    }

    #[test]
    fn hoist_keeps_unknown_image_format_in_text() {
        let original = "A".repeat(400);
        let mut value = serde_json::json!({
            "image_base64": original,
            "format": "bmp",
        });

        assert!(hoist_inline_images(&mut value).is_empty());
        assert_eq!(value["image_base64"].as_str().unwrap().len(), 400);
    }

    #[test]
    fn hoist_rejects_oversized_image_before_copying() {
        let original = "A".repeat(MAX_INLINE_IMAGE_BASE64_CHARS + 1);
        let mut value = serde_json::json!({
            "image_base64": original,
            "format": "png",
        });

        assert!(hoist_inline_images(&mut value).is_empty());
        assert_eq!(
            value["image_base64"].as_str().unwrap().len(),
            MAX_INLINE_IMAGE_BASE64_CHARS + 1
        );
    }

    #[test]
    fn hoist_maps_jpeg_and_is_idempotent() {
        let mut value = serde_json::json!({
            "data": { "image_base64": "B".repeat(400), "format": "jpeg" }
        });
        let first = hoist_inline_images(&mut value);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].mime_type, "image/jpeg");
        // Second pass finds nothing — the short marker is below the 256 guard.
        assert!(hoist_inline_images(&mut value).is_empty());
    }

    /// `browser_exec`'s screenshot step nests its payload one object per step
    /// inside `results[]` — the shape the pre-recursion walk could not reach.
    #[test]
    fn hoists_an_image_nested_inside_a_results_array() {
        let mut value = serde_json::json!({
            "success": true,
            "results": [
                { "step": 1, "action": "navigate https://example.com", "status": "navigated" },
                { "step": 2, "action": "screenshot", "status": "captured",
                  "image_base64": "D".repeat(400), "format": "png" },
            ]
        });

        let images = hoist_inline_images(&mut value);

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].data.len(), 400);
        assert_eq!(images[0].mime_type, "image/png");
        // The marker replaces the payload in place; the sibling step is untouched.
        assert!(value["results"][1]["image_base64"]
            .as_str()
            .unwrap()
            .contains("viewable image block"));
        assert_eq!(value["results"][0]["status"], "navigated");
        assert!(hoist_inline_images(&mut value).is_empty());
    }

    /// The recursion cap, not the payload guards, is what bounds a hostile
    /// nesting depth: images past [`MAX_HOIST_DEPTH`] keep their base64.
    #[test]
    fn hoist_walk_stops_at_the_depth_bound() {
        let mut value = serde_json::json!({ "image_base64": "E".repeat(400), "format": "png" });
        for _ in 0..MAX_HOIST_DEPTH + 2 {
            value = serde_json::json!({ "wrap": value });
        }
        assert!(hoist_inline_images(&mut value).is_empty());
    }

    /// Every browser-automation / screenshot MCP server returns images this
    /// way. Until the adapter stopped pre-serializing its result there was
    /// nothing here to recognize, so the base64 was billed as text, truncated
    /// into an undecodable fragment, and the model acted on a screen it never
    /// saw.
    #[test]
    fn hoists_images_out_of_an_mcp_content_array() {
        let mut value = serde_json::json!({
            "content": [
                { "type": "text", "text": "clicked the button" },
                { "type": "image", "data": "C".repeat(9000), "mimeType": "image/png" },
            ]
        });

        let images = hoist_inline_images(&mut value);

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].data.len(), 9000);
        assert_eq!(images[0].mime_type, "image/png");
        // The text block is untouched; the base64 leaves the text channel.
        assert_eq!(value["content"][0]["text"], "clicked the button");
        assert!(value["content"][1]["data"].as_str().unwrap().len() < 256);
        // Idempotent: the marker is below the size guard.
        assert!(hoist_inline_images(&mut value).is_empty());
    }

    #[test]
    fn mcp_hoist_caps_the_number_of_images_and_rejects_unknown_media_types() {
        let blocks: Vec<_> = (0..MAX_HOISTED_IMAGES + 3)
            .map(|_| {
                serde_json::json!({
                    "type": "image", "data": "D".repeat(1000), "mimeType": "image/png",
                })
            })
            .collect();
        let mut value = serde_json::json!({ "content": blocks });
        assert_eq!(hoist_inline_images(&mut value).len(), MAX_HOISTED_IMAGES);

        // A media type no provider accepts keeps its payload in the text
        // channel rather than being handed over to be rejected wholesale.
        let mut exotic = serde_json::json!({
            "content": [ { "type": "image", "data": "E".repeat(1000), "mimeType": "image/tiff" } ]
        });
        assert!(hoist_inline_images(&mut exotic).is_empty());
        assert_eq!(exotic["content"][0]["data"].as_str().unwrap().len(), 1000);
    }

    #[test]
    fn hoist_ignores_non_image_and_tiny_outputs() {
        let mut rows = serde_json::json!({ "ok": true, "rows": [1, 2, 3] });
        assert!(hoist_inline_images(&mut rows).is_empty());
        // A tiny image_base64 (< 256) is not treated as a screenshot.
        let mut small = serde_json::json!({ "image_base64": "abc", "format": "png" });
        assert!(hoist_inline_images(&mut small).is_empty());
    }

    // ---------------------------------------------------------------
    // resolve_result_budget
    // ---------------------------------------------------------------

    #[test]
    fn read_file_family_always_returns_none() {
        assert_eq!(resolve_result_budget("read_file", None), None);
        assert_eq!(resolve_result_budget("Read", None), None);
        assert_eq!(resolve_result_budget("file_read", None), None);
        // Even an explicit setting cannot override the read-recursion guard.
        assert_eq!(resolve_result_budget("read_file", Some(99_999)), None);
    }

    #[test]
    fn explicit_wins_over_fallback_table() {
        assert_eq!(resolve_result_budget("bash", Some(123)), Some(123));
        assert_eq!(resolve_result_budget("custom_thing", Some(50)), Some(50));
    }

    #[test]
    fn explicit_budget_overrides_name_table() {
        // Explicit budget always wins over the name table.
        assert_eq!(
            resolve_result_budget("web_fetch", Some(10_000)),
            Some(10_000)
        );
        assert_eq!(resolve_result_budget("bash", Some(8_000)), Some(8_000));
    }

    #[test]
    fn fallback_table_keeps_grep() {
        // `search_files`/`Grep` has no in-crate tool to declare the trait
        // method, so it stays in the name table (alongside `web_fetch`).
        assert_eq!(resolve_result_budget("Grep", None), Some(6_000));
        assert_eq!(resolve_result_budget("search_files", None), Some(6_000));
    }

    #[test]
    fn unknown_tool_falls_back_to_default() {
        assert_eq!(
            resolve_result_budget("some_other_tool", None),
            Some(DEFAULT_RESULT_BUDGET_TOKENS)
        );
    }

    #[test]
    fn web_fetch_budget_is_10k_via_name_table() {
        // Production path: web_fetch is executor-registered, so its
        // AlephTool-declared 10k never arrives as `explicit`. The name table
        // must carry it (mirrors the search_files arm).
        assert_eq!(resolve_result_budget("web_fetch", None), Some(10_000));
    }

    #[test]
    fn explicit_budget_still_wins_over_name_table() {
        assert_eq!(resolve_result_budget("web_fetch", Some(4_000)), Some(4_000));
    }

    // ---------------------------------------------------------------
    // window ceiling (B14)
    // ---------------------------------------------------------------

    #[test]
    fn window_ceiling_caps_declared_budgets_not_just_the_default() {
        // A 16k-window model yields a 2_400 per-result ceiling. `web_fetch`'s
        // declared 10k and `Grep`'s 6k are exactly the values that must come
        // down — a ceiling applied only to the `None` fallback would leave the
        // biggest offenders untouched.
        let ceiling = 2_400;
        assert_eq!(
            resolve_result_budget_under("web_fetch", None, ceiling),
            Some(2_400)
        );
        assert_eq!(
            resolve_result_budget_under("Grep", None, ceiling),
            Some(2_400)
        );
        assert_eq!(
            resolve_result_budget_under("bash", Some(8_000), ceiling),
            Some(2_400)
        );
        assert_eq!(
            resolve_result_budget_under("unknown", None, ceiling),
            Some(2_400)
        );
        // A tool that already declares less than the ceiling keeps its value.
        assert_eq!(
            resolve_result_budget_under("tiny", Some(500), ceiling),
            Some(500)
        );
        // The read-recursion guard still wins over everything.
        assert_eq!(
            resolve_result_budget_under("read_file", None, ceiling),
            None
        );
    }

    #[test]
    fn uncapped_ceiling_is_todays_behavior() {
        // No ceiling installed (large window / no `[context_budget]`) → the
        // table is byte-for-byte what it was.
        assert_eq!(
            resolve_result_budget_under("web_fetch", None, usize::MAX),
            Some(10_000)
        );
        assert_eq!(
            resolve_result_budget_under("Grep", None, usize::MAX),
            Some(6_000)
        );
        assert_eq!(
            resolve_result_budget_under("bash", None, usize::MAX),
            Some(DEFAULT_RESULT_BUDGET_TOKENS)
        );
    }

    #[test]
    fn ceiling_at_or_above_the_default_is_refused() {
        // A large-window model must not install a ceiling at all — an 8_000 one
        // would silently clip `web_fetch`'s declared 10k, which is a regression,
        // not a fix. The installer drops it, so the global stays uncapped.
        set_global_result_budget_ceiling(DEFAULT_RESULT_BUDGET_TOKENS);
        set_global_result_budget_ceiling(50_000);
        assert_eq!(
            resolve_result_budget("web_fetch", None),
            Some(10_000),
            "a refused ceiling must leave the process uncapped"
        );
    }

    // ---------------------------------------------------------------
    // apply_result_budget
    // ---------------------------------------------------------------

    #[test]
    fn small_text_unchanged() {
        let (_scratch, store, _base) = test_store("small_unchanged");
        let out = apply_result_budget("c1", "bash", "hello", Some(&store), Some(10_000), None);
        assert_eq!(out.text, "hello");
        assert!(out.persisted_path.is_none());
    }

    #[test]
    fn budget_none_truncates_no_persist() {
        let (_scratch, store, base) = test_store("budget_none");
        let big = "x".repeat(60_000);
        let out = apply_result_budget("c2", "read_file", &big, Some(&store), None, None);
        assert!(
            out.persisted_path.is_none(),
            "must not persist when budget is None"
        );
        assert!(
            !out.text.starts_with("[Full output persisted:"),
            "should be truncated, got: {}",
            &out.text[..80.min(out.text.len())]
        );
        // The store directory should remain empty.
        let entries: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 0, "no file should be written");
    }

    #[test]
    fn large_text_persists_returns_marker() {
        let (_scratch, store, base) = test_store("large_persists");
        // Build text with retrievable structure so indexing produces sections.
        let big = (0..2000)
            .map(|i| format!("line {i} payload alpha beta gamma"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = apply_result_budget("c3", "bash", &big, Some(&store), Some(100), None);
        assert!(
            out.text.starts_with("[Full output persisted:"),
            "expected marker, got: {}",
            &out.text[..80.min(out.text.len())]
        );
        assert!(out.persisted_path.is_some());
        // The marker is now augmented with a ctx_search retrieval hint.
        assert!(
            out.text.contains("ctx_search"),
            "expected ctx_search hint in marker, got: {}",
            out.text
        );
        // Exactly one persisted blob (.txt); the FTS5 index.db lives alongside.
        let txt_count = std::fs::read_dir(&base)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "txt"))
            .count();
        assert_eq!(txt_count, 1, "exactly one .txt blob should be written");
        // The blob's content is searchable through the same store.
        let hits = store.search("payload alpha", 3);
        assert!(!hits.is_empty(), "offloaded blob should be searchable");
    }

    #[test]
    fn no_store_means_truncate_only() {
        let big = "z".repeat(40_000);
        let out = apply_result_budget("c4", "bash", &big, None, Some(100), None);
        assert!(out.persisted_path.is_none());
        assert!(!out.text.starts_with("[Full output persisted:"));
        assert!(
            out.text.contains("[output truncated"),
            "expected truncate marker: {}",
            &out.text[..120.min(out.text.len())]
        );
    }

    #[test]
    fn parse_marker_path_roundtrip() {
        let marker = "[Full output persisted: /tmp/aleph/x.txt (1234 tokens, bash)]";
        let path = parse_marker_path(marker).expect("parse");
        assert_eq!(path, PathBuf::from("/tmp/aleph/x.txt"));
    }

    /// A read result is the exact lines the model asked for. It may be
    /// head/tail-truncated when it overruns, but it must never be replaced by a
    /// *semantic re-selection* of itself — that answers a different question
    /// than the one asked, and it silently drops everything the model wanted.
    ///
    /// This test previously asserted the opposite (that the read-family branch
    /// distilled error lines). Two things made that wrong in production:
    /// `pub enum Error {` lowercases into a hit on the `"error "` marker, so any
    /// large Rust file with an error type was replaced by a grep of itself; and
    /// a real `file_read` result reaches this function as single-line JSON, so
    /// the "distilled errors" were in fact the first 400 chars of the JSON
    /// envelope.
    #[test]
    fn read_family_truncates_and_never_re_selects_content() {
        let (_scratch, store, _base) = test_store("budget_none_no_distill");
        let mut big = String::new();
        big.push_str("pub enum Error {\n");
        big.push_str("    NotFound,\n");
        big.push_str("}\n");
        for i in 0..3000 {
            big.push_str(&format!("fn helper_{i}() -> u32 {{ {i} }}\n"));
        }
        big.push_str("// the last line of the file\n");

        let out = apply_result_budget("c-distill", "read_file", &big, Some(&store), None, None);
        assert!(out.persisted_path.is_none(), "reads are never persisted");
        assert!(
            !out.text.contains("Output digest"),
            "a read must not be replaced by an error digest, got: {}",
            &out.text[..160.min(out.text.len())]
        );
        assert!(
            out.text.contains("[output truncated"),
            "over-long reads are head/tail truncated, got: {}",
            &out.text[..160.min(out.text.len())]
        );
        assert!(
            out.text.starts_with("pub enum Error {"),
            "the head the model asked for must survive"
        );
        assert!(
            out.text.ends_with("// the last line of the file\n"),
            "the tail must survive too"
        );
    }

    /// Content-typed output over budget: the model gets the reduced signal
    /// inline *and* the recovery handle, so it never has to spend a `ctx_search`
    /// round-trip just to see which test failed.
    #[test]
    fn reduced_content_is_inlined_above_the_recovery_marker() {
        let (_scratch, store, _base) = test_store("reduced_inline");
        let mut original = String::from("$ cargo test\n");
        for i in 0..2000 {
            original.push_str(&format!("test suite::case_{i} ... ok\n"));
        }
        original.push_str("test suite::case_boom ... FAILED\n");
        original.push_str("test result: FAILED. 2000 passed; 1 failed\n");
        let reduced = crate::tool_output::structured::reduce_within(&original, None)
            .expect("a cargo test log must classify")
            .render();

        let out = apply_result_budget(
            "c-inline",
            "bash",
            &reduced,
            Some(&store),
            Some(100),
            Some(&original),
        );

        assert!(
            out.persisted_path.is_some(),
            "the original must be offloaded"
        );
        assert!(
            out.text.contains("[Full output persisted:"),
            "recovery marker missing: {}",
            out.text
        );
        assert!(
            out.text.contains("FAILED. 2000 passed; 1 failed"),
            "the signal must be inline, not behind a ctx_search: {}",
            out.text
        );
        assert!(
            !out.text.contains("case_500"),
            "the passing-test noise must not be inlined"
        );
    }

    /// The reduced body is offloaded even when it already fits the budget —
    /// otherwise the lines the reducer dropped would be gone for good.
    #[test]
    fn fitting_reduced_content_still_offloads_the_original() {
        let (_scratch, store, _base) = test_store("reduced_fits");
        let mut original = String::new();
        for i in 0..3000 {
            original.push_str(&format!("src/lib.rs:{i}: let target = {i};\n"));
        }
        let reduced = crate::tool_output::structured::reduce_within(&original, None)
            .expect("a cargo test log must classify")
            .render();
        assert!(
            estimate_tokens_smart(&reduced) <= 8_000,
            "precondition: the reduction fits the budget"
        );

        let out = apply_result_budget(
            "c-fits",
            "bash",
            &reduced,
            Some(&store),
            Some(8_000),
            Some(&original),
        );
        assert!(
            out.persisted_path.is_some(),
            "the dropped lines must stay recoverable, got: {}",
            out.text
        );
        assert!(out.text.contains("[compacted search:"));
        assert!(out.text.contains("[Full output persisted:"));
    }

    /// Guard the no-op: with no hygiene pass, an over-budget opaque result keeps
    /// exactly the marker-only shape it has always had.
    #[test]
    fn opaque_over_budget_output_is_unchanged_by_the_new_path() {
        let (_scratch, store, _base) = test_store("opaque_unchanged");
        let big = (0..2000)
            .map(|i| format!("line {i} payload alpha beta gamma"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = apply_result_budget("c-opaque", "bash", &big, Some(&store), Some(100), None);
        assert!(
            out.text.starts_with("[Full output persisted:"),
            "marker must still lead for opaque content, got: {}",
            &out.text[..120.min(out.text.len())]
        );
    }

    #[test]
    fn persist_branch_prepends_inline_errors() {
        let (_scratch, store, _base) = test_store("persist_inline_errors");
        let mut big = String::new();
        big.push_str("error: linker failed with exit code 1\n");
        big.push_str("  --> src/net.rs:10:3\n");
        // Enough retrievable structure to index into sections + exceed budget.
        for i in 0..2000 {
            big.push_str(&format!("trace line {i} payload alpha beta gamma\n"));
        }
        let out = apply_result_budget("c-persist", "bash", &big, Some(&store), Some(100), None);
        assert!(out.persisted_path.is_some(), "should have persisted");
        // The marker is still present...
        assert!(out.text.contains("[Full output persisted:"));
        // ...but errors now lead so they are visible without a ctx_search.
        assert!(
            out.text.contains("Output digest") && out.text.contains("linker failed"),
            "expected inline error digest above marker, got: {}",
            &out.text[..160.min(out.text.len())]
        );
    }

    #[test]
    fn truncate_preserves_head_and_tail() {
        let text = format!("HEAD{}TAIL", "x".repeat(80_000));
        let out = truncate_with_budget(&text, 100);
        assert!(out.starts_with("HEAD"), "head missing: {}", &out[..80]);
        assert!(
            out.ends_with("TAIL"),
            "tail missing: {}",
            &out[out.len() - 80..]
        );
        assert!(out.contains("[output truncated"));
    }

    #[test]
    fn truncate_is_content_aware_and_lands_near_budget() {
        // CJK content far over budget. Because the kept head+tail is now sized
        // from the estimator's own chars-per-token ratio (not a fixed
        // 4-chars/token assumption), the truncated result's estimated tokens
        // land near the budget regardless of script — never the divergence the
        // old byte-vs-char math produced.
        let text = "数据分析报告".repeat(3000);
        let budget = 250;
        let out = truncate_with_budget(&text, budget);
        assert!(out.contains("[output truncated"), "should be truncated");
        let kept = estimate_tokens_smart(&out);
        assert!(
            kept <= budget * 2,
            "content-aware truncation kept {kept} tokens for budget {budget} (expected ~budget)"
        );
        assert!(
            kept >= budget / 4,
            "should not over-truncate to near-nothing: kept {kept}"
        );
        // Slicing stays on char boundaries (no panic, valid UTF-8 out).
        assert!(out.chars().count() > 0);
    }
}
