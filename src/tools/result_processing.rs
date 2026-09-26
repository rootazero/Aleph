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
use crate::tool_output::render::Rendered;
use crate::tools::result_store::{extract_persisted_path, ToolResultStore};

const MAX_INLINE_IMAGE_BASE64_CHARS: usize = (20usize * 1024 * 1024).div_ceil(3) * 4;

/// Global default budget for tools that declare no
/// [`crate::tools::AlephTool::MAX_RESULT_TOKENS`] — every tool but the read
/// family. A result over it is offloaded and indexed (Layer 2), and the model
/// retrieves the part it needs with `ctx_search`; so the gate is set to what a
/// result typically needs in context, not to what it might contain.
pub const DEFAULT_RESULT_BUDGET_TOKENS: usize = 4_000;

/// The read family's window ([`read_backstop_tokens`]), and so the largest
/// per-result budget any result gets. A read returns exactly the lines the
/// model asked for (`offset`/`limit`) and is never offloaded — offloading a
/// read would hand back a marker to read again — so cutting its window below
/// what was asked only turns one read into several.
pub const MAX_RESULT_BUDGET_TOKENS: usize = 8_000;

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
/// operator results are clamped to it when in fact nothing is clamped.
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
/// Case 2 now stamps [`crate::capability::CapabilitySlot::decline`] inside the
/// setter, so the two are no longer indistinguishable: case 1 leaves NO outcome
/// (nothing reached the slot) and case 2 leaves `Declined` naming the window.
/// ⚠️ It had to be closed here rather than in boot: the arm is an early
/// `return` inside this library setter, one call away from the boot call site,
/// so a walk of boot's `else` arms does not reach it. Case 1 is still silent by
/// construction and correctly so — a CLI one-shot never boots a gateway.
static RESULT_BUDGET_CEILING: CapabilitySlot<usize> = CapabilitySlot::new(
    "tools/result-budget-ceiling",
    MissingSemantics::IndistinguishableDefault {
        reads_as: "usize::MAX — uncapped, byte-for-byte the pre-ceiling behaviour",
    },
);

/// Install the process-wide per-result ceiling. Called once at boot.
///
/// A ceiling at or above [`MAX_RESULT_BUDGET_TOKENS`] is **ignored**: it
/// clamps no budget in play, and this knob exists solely to clamp *down* on
/// small-window models. So a large-window model installs nothing.
pub fn set_global_result_budget_ceiling(ceiling: usize) {
    if ceiling >= MAX_RESULT_BUDGET_TOKENS {
        // Not a failure — a decision, and the one an operator is most likely to
        // mistake for a wiring gap, because the resulting read (`usize::MAX`)
        // is byte-for-byte what a boot that never got here leaves behind.
        RESULT_BUDGET_CEILING.decline(
            "this model's context window needs no per-result clamp: the ceiling \
             derived from `[context_budget] token_budget` is at or above the \
             largest per-result budget (the read window), and this knob only \
             ever clamps DOWN. A smaller-window model (or a smaller \
             `token_budget`) installs one.",
        );
        return;
    }
    let _ = RESULT_BUDGET_CEILING.install(ceiling);
}

/// Record that boot reached this slot and had nothing to install.
///
/// Distinct from the in-setter decline above: this is for boot's outer
/// `Err(e)` arm, where the `ToolResultStore` failed to open and Layers 2 and 3
/// are disabled together, so no ceiling was even derived. `because` is quoted
/// verbatim to an operator.
pub fn decline_global_result_budget_ceiling(because: &'static str) {
    RESULT_BUDGET_CEILING.decline(because);
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn result_budget_ceiling_slot() -> &'static dyn SlotStatus {
    &RESULT_BUDGET_CEILING
}

/// The ceiling actually installed, `None` when none is (declined for a large
/// window, or never set). What a report of the ceiling in effect reads — the
/// value boot derived is not it when the setter declined.
#[must_use]
pub fn installed_result_budget_ceiling() -> Option<usize> {
    RESULT_BUDGET_CEILING.get().copied()
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
/// read window ([`MAX_RESULT_BUDGET_TOKENS`]), clamped by the boot-installed
/// window ceiling.
///
/// Exposed because `file_read` sizes its own window to stay under this. Reading
/// the constant alone is not enough: on a small-window model the ceiling moves
/// the bound down to as little as 2 000 tokens, and a producer that ignored it
/// would hand the generic truncator a window to cut a hole in — the exact bug the
/// self-sizing exists to prevent.
#[must_use]
pub(crate) fn read_backstop_tokens() -> usize {
    MAX_RESULT_BUDGET_TOKENS.min(result_budget_ceiling())
}

/// Resolve a tool's per-result token budget.
///
/// Lookup order:
/// 1. The read family ([`is_read_family`]) always returns `None` (system
///    invariant — a `file_read` result is the only way the model can pull
///    a persisted marker file back into context, so persisting one would
///    create a loop).
/// 2. `explicit` — the tool's declared
///    [`crate::tools::AlephTool::MAX_RESULT_TOKENS`], carried to the
///    dispatcher by `RegistryToolAdapter` — wins for every other name.
/// 3. Otherwise fall back to [`DEFAULT_RESULT_BUDGET_TOKENS`].
///
/// There is no name table: the declaration is the only per-tool source, so a
/// budget is changed where the tool lives. (A table keyed on tool names used to
/// sit here as a second answer, and was the one actually in effect — the
/// declarations never reached this function.)
///
/// Whatever that yields is then capped by the boot-installed window ceiling
/// (see [`set_global_result_budget_ceiling`]). The cap applies to *every*
/// branch, not just the fallback: a declared 10k budget on a 16k-window model is
/// exactly the value that has to come down, so treating the ceiling as a default
/// rather than a maximum would let the worst offenders through untouched.
///
/// `None` from this function means "do not persist this tool's output;
/// just truncate when it exceeds [`read_backstop_tokens`]".
#[must_use]
pub fn resolve_result_budget(name: &str, explicit: Option<usize>) -> Option<usize> {
    resolve_result_budget_under(name, explicit, result_budget_ceiling())
}

/// Pure core of [`resolve_result_budget`] with the ceiling passed in, so the
/// cap semantics are unit-testable without touching the process-wide slot.
pub(crate) fn resolve_result_budget_under(
    name: &str,
    explicit: Option<usize>,
    ceiling: usize,
) -> Option<usize> {
    if is_read_family(name) {
        return None;
    }
    Some(
        explicit
            .unwrap_or(DEFAULT_RESULT_BUDGET_TOKENS)
            .min(ceiling),
    )
}

/// The read family: the tool whose result is exactly the window the model
/// asked for (`offset` / `limit`). Layer 2 gives it no budget of its own — it
/// is never offloaded there, because the only way back from an offloaded read
/// is another read — and bounds it by [`read_backstop_tokens`] instead. The
/// per-turn spill does NOT exempt it: a turn's reads still have to fit the
/// window, and a spilled read is persisted and indexed, so it is a re-read,
/// not a loss.
///
/// One name: the builtin `file_read`. Nothing registers or aliases a tool as
/// `read_file` / `Read` (MCP tools arrive qualified `server__tool`), so those
/// spellings, which this list used to carry, matched no call.
#[must_use]
pub(crate) fn is_read_family(tool_name: &str) -> bool {
    use crate::tools::AlephTool;
    tool_name == <crate::builtin_tools::FileReadTool as AlephTool>::NAME
}

tokio::task_local! {
    /// The retrieval tools callable in the dispatch this future runs under,
    /// scoped around the tool's own execution by every layer that knows a
    /// gate: the scoped dispatcher, and a narrowing wrapper around it
    /// (`AllowlistToolService`). A tool that offloads its own output
    /// (`web_fetch`'s fetch by intent, the browser offload) names only these
    /// in its footer, as Layer 2 does.
    static DISPATCH_RECOVERY_TOOLS: RecoveryTools;
}

/// Run `fut` with `tools` in scope. A scope already set by an outer layer is
/// narrowed, never widened: the effective set is the intersection, so an
/// inner dispatcher that knows only its own gates cannot undo the narrowing
/// of a wrapper around it.
pub(crate) async fn with_recovery_tools<F: std::future::Future>(
    tools: RecoveryTools,
    fut: F,
) -> F::Output {
    let effective = dispatch_recovery_tools().map_or(tools, |outer| outer.intersect(tools));
    DISPATCH_RECOVERY_TOOLS.scope(effective, fut).await
}

/// The retrieval tools callable in this dispatch, or `None` outside one (a
/// direct call, a test): the caller then cannot see the gates.
#[must_use]
pub(crate) fn dispatch_recovery_tools() -> Option<RecoveryTools> {
    DISPATCH_RECOVERY_TOOLS.try_with(|t| *t).ok()
}

/// Which retrieval tools the model can call, this turn, to get an offloaded
/// original back.
///
/// The recovery footer is an instruction to the model, so it may only name
/// tools that will actually dispatch: "use `ctx_search`" said to an agent whose
/// allow set or `[policies.tool_permissions]` excludes it is a handle that fails
/// on first use. [`apply_result_budget`] takes this from its caller because only
/// the dispatcher can see the turn's gates (`ScopedToolService::recovery_tools`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryTools {
    /// `ctx_search` is dispatchable.
    pub ctx_search: bool,
    /// `file_read` is dispatchable.
    pub file_read: bool,
}

impl RecoveryTools {
    /// Neither retrieval tool is callable: an offloaded original would be a
    /// path the model has no tool to open. The one predicate every offload
    /// writer consults (through [`offload`]) before it writes.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        !self.ctx_search && !self.file_read
    }

    /// Callable in both.
    #[must_use]
    pub const fn intersect(self, other: Self) -> Self {
        Self {
            ctx_search: self.ctx_search && other.ctx_search,
            file_read: self.file_read && other.file_read,
        }
    }

    /// Both callable — what a caller that cannot see the turn's tool gates
    /// assumes: [`dispatch_recovery_tools`]'s fallback outside a dispatch, and
    /// `ToolService::recovery_tools`'s default for a service that has no gates
    /// to consult. The harness Layer-3 spill asks its `ToolService`, which
    /// answers from the turn's gates when it is a `ScopedToolService`.
    pub const ALL: Self = Self {
        ctx_search: true,
        file_read: true,
    };
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
///
/// `recovery` names the retrieval tools the footer may point at; see
/// [`RecoveryTools`].
pub fn apply_result_budget(
    tool_call_id: &str,
    tool_name: &str,
    text: &str,
    store: Option<&ToolResultStore>,
    budget: Option<usize>,
    reduced_from: Option<&str>,
    recovery: RecoveryTools,
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
        // bypassed on this branch, which handed a 2 400-token-window model the
        // full read allowance — the one case the knob exists to prevent.
        let truncated = truncate_with_budget(text, read_backstop_tokens());
        let tokens_after = estimate_tokens_smart(&truncated);
        return ProcessedResult {
            text: truncated,
            tokens_in_context: tokens_after,
            persisted_path: None,
        };
    };

    // A result that already carries its own offload (a tool that persisted and
    // indexed its output under this same call id, e.g. `web_fetch`'s fetch by
    // intent) is never persisted again: the blob is named by the call id, so a
    // second write would replace the original with this result. Recognised by
    // the marker naming THIS call's blob file (see [`own_marker_at`]). Kept as
    // is when it fits; over budget, the footer (marker line and its hint) is
    // kept whole and everything else around it is cut into what is left —
    // text after the footer included, which is where a procedure's later
    // steps land.
    if let Some(at) = own_marker_at(tool_call_id, tool_name, text) {
        let kept = if tokens <= budget {
            text.to_string()
        } else {
            bounded_around_footer(text, own_footer_span(text, at), budget)
        };
        return ProcessedResult {
            tokens_in_context: estimate_tokens_smart(&kept),
            text: kept,
            persisted_path: None,
        };
    }

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
        return match recovery_footer_for(store, tool_call_id, tool_name, full, budget, recovery) {
            Some((footer, path)) => {
                let body = format!("{text}\n{footer}");
                ProcessedResult {
                    tokens_in_context: estimate_tokens_smart(&body),
                    text: body,
                    persisted_path: path,
                }
            }
            // Detail was dropped and nothing the model can call would read
            // an offload back: say the reduction is final.
            None if recovery.is_empty() => {
                let room = budget.saturating_sub(estimate_tokens_smart(NOT_SAVED_NOTE) + 1);
                let kept = if tokens <= room {
                    text.to_string()
                } else {
                    distill_or_truncate(text, room)
                };
                let body = format!("{kept}\n{NOT_SAVED_NOTE}");
                ProcessedResult {
                    tokens_in_context: estimate_tokens_smart(&body),
                    text: body,
                    persisted_path: None,
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
    let rendered = crate::tool_output::render::line_preserving(persist_source);
    if let Some(Offloaded { footer, path, .. }) = offload(
        store,
        tool_call_id,
        tool_name,
        persist_source,
        &rendered,
        budget,
        recovery,
    ) {
        let footer_tokens = estimate_tokens_smart(&footer);
        let body = match original {
            // Content-typed: inline the signal, sized so body + footer still
            // respect the tool's declared budget. `distill_or_truncate` rather
            // than a blind head/tail cut — a reduction that is *still* over budget
            // is usually a wall of diagnostics, and the middle is where the
            // failure is named.
            Some(_) => distill_or_truncate(text, budget.saturating_sub(footer_tokens)),
            // Opaque: a bounded error preview only — visible without a
            // ctx_search round-trip, absent when there is no error signal.
            // Read off the rendering (here `persist_source` IS `text`): a typed
            // result's flat envelope is one line, which a line digest cannot
            // read, so it used to come back empty for every typed result. Not
            // for a fenced result: the digest lines are the untrusted text
            // itself and would sit above the marker, outside the fence.
            None if rendered.fenced => String::new(),
            None => inline_error_digest(&rendered.text, Some(budget)).unwrap_or_default(),
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

    // No store, the persist failed (the store logs internally), or nothing the
    // model can call would read an offload back — truncate. The last is said:
    // the cut is final, and the model should not look for the rest.
    let truncated = if recovery.is_empty() {
        let room = budget.saturating_sub(estimate_tokens_smart(NOT_SAVED_NOTE) + 1);
        format!("{}\n{NOT_SAVED_NOTE}", distill_or_truncate(text, room))
    } else {
        distill_or_truncate(text, budget)
    };
    let tokens_after = estimate_tokens_smart(&truncated);
    ProcessedResult {
        text: truncated,
        tokens_in_context: tokens_after,
        persisted_path: None,
    }
}

/// Said where an over-budget result is cut and nothing was saved, because no
/// retrieval tool is callable to read an offload back.
const NOT_SAVED_NOTE: &str = "[Output cut to fit: no retrieval tool (ctx_search / file_read) is \
                              callable here, so the full output was not saved and the cut \
                              part cannot be read back.]";

/// Whether `text` carries the persist marker of this call's own blob — the
/// file a persist under (`tool_call_id`, `tool_name`) would write, so a second
/// persist would replace it. The one derivation of "already persisted" for
/// both Layer 2 and the Layer-3 turn budget: a marker line that merely appears
/// in the payload (a `file_read` of a file that quotes one) names some other
/// blob and does not count.
#[must_use]
pub(crate) fn carries_own_persisted_marker(
    text: &str,
    tool_call_id: &str,
    tool_name: &str,
) -> bool {
    own_marker_at(tool_call_id, tool_name, text).is_some()
}

/// Byte offset in `text` of the last marker naming this call's own blob file,
/// or `None`. Matched on the file name (`blob_file_name`), not the whole path:
/// the directory differs between a store's scoped and unscoped handles, the
/// name does not. The name is `[A-Za-z0-9_.-]` only, so it reads the same
/// inside the flattened JSON of a typed result; the separator before it may
/// arrive escaped (`\\`), which still ends in a separator.
fn own_marker_at(tool_call_id: &str, tool_name: &str, text: &str) -> Option<usize> {
    use crate::tools::result_store::{blob_file_name, PERSISTED_REF_PREFIX};
    let name = blob_file_name(tool_call_id, tool_name);
    let named = |sep: char| format!("{sep}{name} (");
    let (slash, backslash) = (named('/'), named('\\'));
    text.match_indices(PERSISTED_REF_PREFIX)
        .filter(|(at, _)| {
            let rest = &text[at + PERSISTED_REF_PREFIX.len()..];
            let marker = &rest[..rest.find(")]").unwrap_or(rest.len())];
            marker.contains(&slash) || marker.contains(&backslash)
        })
        .map(|(at, _)| at)
        .last()
}

/// The byte range of the footer starting at `at`: the marker through its
/// closing `)]`, plus the hint line right under it when there is one (the
/// break may be a real newline or, inside flattened JSON, an escaped one).
fn own_footer_span(text: &str, at: usize) -> std::ops::Range<usize> {
    let rest = &text[at..];
    let Some(close) = rest.find(")]") else {
        return at..text.len();
    };
    let marker_end = close + ")]".len();
    let after = &rest[marker_end..];
    let sep = if after.starts_with('\n') {
        1
    } else if after.starts_with("\\n") {
        2
    } else {
        0
    };
    let hint = &after[sep..];
    let is_hint = sep > 0 && (hint.starts_with("[Indexed ") || hint.starts_with(FILE_READ_HINT));
    if !is_hint {
        return at..at + marker_end;
    }
    let hint_end = [hint.find('\n'), hint.find("\\n")]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(hint.len());
    at..at + marker_end + sep + hint_end
}

/// `text` bounded by `budget` with the footer at `span` kept whole: the text
/// around it (before and after) is joined and head/tail cut into the room the
/// footer leaves, halving that room until the total fits.
fn bounded_around_footer(text: &str, span: std::ops::Range<usize>, budget: usize) -> String {
    let footer = &text[span.clone()];
    let rest = format!("{}{}", &text[..span.start], &text[span.end..]);
    let mut room = budget.saturating_sub(estimate_tokens_smart(footer) + 1);
    loop {
        let body = if room == 0 {
            String::new()
        } else {
            truncate_with_budget(&rest, room)
        };
        let kept = if body.trim().is_empty() {
            footer.to_string()
        } else {
            format!("{body}\n{footer}")
        };
        if room == 0 || estimate_tokens_smart(&kept) <= budget {
            return kept;
        }
        room /= 2;
    }
}

/// Offload `full` to the result store and build the recovery footer the model
/// uses to get the dropped detail back: the persist marker plus a hint naming
/// how to read it back — naming only the retrieval tools in `recovery`, the
/// ones the model can call. A writer derives that set from where it runs: the
/// dispatcher from its gates, the harness Layer-3 spill from
/// `ToolService::recovery_tools`, a tool offloading its own output from
/// [`dispatch_recovery_tools`] (falling back to [`RecoveryTools::ALL`] only
/// outside a dispatch, where nothing about the gates is known).
///
/// `None` when there is no store or the persist did not happen (content under
/// `threshold`, or a write failure) — the caller then falls back to truncation.
/// (The spill once called `persist_if_large` directly and so emitted a marker
/// with **no** `ctx_search` hint over a blob that was never indexed — the model
/// was pointed at a file it could only re-read whole, defeating the offload.)
///
/// Every writer of an offloaded original goes through here, which is why the
/// line-preserving rendering happens here and not at ingress: a flattened typed
/// result is ONE line of JSON, and both readers of the blob work in lines —
/// `file_read` pages by line and clamps an overlong one, `ContentIndex` chunks
/// by line count — so stored as-is it is one section and one clipped line:
/// found, never read. See [`crate::tool_output::render`].
///
/// The size gate is measured on `full` as given, not on the rendering: the
/// estimator charges one-line JSON at the code ratio and rendered prose at a
/// cheaper one, so gating on the rendering could call an over-budget result
/// "small", skip the offload, and send the caller to truncation.
pub(crate) fn recovery_footer_for(
    store: Option<&ToolResultStore>,
    tool_call_id: &str,
    tool_name: &str,
    full: &str,
    threshold: usize,
    recovery: RecoveryTools,
) -> Option<(String, Option<PathBuf>)> {
    offload_indexed(store, tool_call_id, tool_name, full, threshold, recovery)
        .map(|o| (o.footer, o.path))
}

/// [`recovery_footer_for`], also saying how many sections the blob indexed
/// into (`None`: the index could not take it) — for a writer that searches
/// its own blob next (`web_fetch`'s fetch by intent) and must not read "not
/// indexed" as "nothing matched".
pub(crate) fn offload_indexed(
    store: Option<&ToolResultStore>,
    tool_call_id: &str,
    tool_name: &str,
    full: &str,
    threshold: usize,
    recovery: RecoveryTools,
) -> Option<Offloaded> {
    let rendered = crate::tool_output::render::line_preserving(full);
    offload(
        store,
        tool_call_id,
        tool_name,
        full,
        &rendered,
        threshold,
        recovery,
    )
}

/// What an offload left: the footer the model reads, the blob path, and the
/// number of sections the blob indexed into (`None` when indexing failed).
pub(crate) struct Offloaded {
    pub(crate) footer: String,
    pub(crate) path: Option<PathBuf>,
    pub(crate) sections: Option<usize>,
}

/// The fewest tokens a result must exceed before an offload may replace it:
/// the marker this store writes for the call, plus the longest hint line the
/// footer can carry under it. One measurement for both the offload and the
/// Layer-3 turn budget's decision to spill, so the budget never credits a
/// spill the offload then refuses. (A preview whose scrub grows it — a
/// chat-template token becomes a longer placeholder — can still exceed the
/// bound on the hint.)
#[must_use]
pub(crate) fn offload_floor_tokens(
    store: &ToolResultStore,
    tool_call_id: &str,
    tool_name: &str,
    tokens: usize,
) -> usize {
    let marker = store.marker_for(tool_call_id, tool_name, tokens);
    estimate_tokens_smart(&marker) + 1 + longest_hint_tokens()
}

/// Tokens of the longest hint [`footer_hint`] can write: the search hint with
/// every preview at its longest, or the `file_read` hint.
fn longest_hint_tokens() -> usize {
    use crate::context::retrieval::{MAX_TITLE_CHARS, PREVIEW_COUNT};
    let longest = IndexOutcome {
        sections: usize::MAX,
        previews: vec!["x".repeat(MAX_TITLE_CHARS + 1); PREVIEW_COUNT],
    };
    estimate_tokens_smart(&search_hint(&longest, true)).max(estimate_tokens_smart(FILE_READ_HINT))
}

/// What a Layer-3 spill leaves in context in place of `text`: the offload
/// footer; `text` itself when it is no larger than a footer would be (a spill
/// would grow it); otherwise — no store, no retrieval tool callable, a failed
/// write — a head/tail cut to the residue the turn budget credits, with a note
/// that the rest was not saved. Every branch leaves at most what the turn
/// budget booked, so the per-turn bound holds whether or not the write did.
#[must_use]
pub(crate) fn spill_replacement(
    store: Option<&ToolResultStore>,
    tool_call_id: &str,
    tool_name: &str,
    text: &str,
    recovery: RecoveryTools,
) -> String {
    let tokens = estimate_tokens_smart(text);
    if let Some(store) = store {
        if tokens <= offload_floor_tokens(store, tool_call_id, tool_name, tokens) {
            return text.to_string();
        }
    }
    if let Some((footer, _)) =
        recovery_footer_for(store, tool_call_id, tool_name, text, 0, recovery)
    {
        return footer;
    }
    let residue = crate::tools::turn_budget::spill_residue_tokens(tokens);
    let room = residue.saturating_sub(estimate_tokens_smart(NOT_SAVED_NOTE) + 1);
    format!("{}\n{NOT_SAVED_NOTE}", truncate_with_budget(text, room))
}

/// The shared body of [`recovery_footer_for`], for a caller that already holds
/// the rendering (the opaque arm of [`apply_result_budget`] reads it too).
/// `gate` is the flat text the size check reads; `rendered` is what is stored.
fn offload(
    store: Option<&ToolResultStore>,
    tool_call_id: &str,
    tool_name: &str,
    gate: &str,
    rendered: &Rendered<'_>,
    threshold: usize,
    recovery: RecoveryTools,
) -> Option<Offloaded> {
    let store = store?;
    // A blob nothing can read back is a fail-dead handle: the caller cuts
    // instead, and says so.
    if recovery.is_empty() {
        return None;
    }
    let gate_tokens = estimate_tokens_smart(gate);
    // Never replace a result with a footer at least as large as the result —
    // measured by the one floor the turn budget also uses.
    if gate_tokens <= threshold
        || gate_tokens <= offload_floor_tokens(store, tool_call_id, tool_name, gate_tokens)
    {
        return None;
    }
    let marker = store.persist(tool_call_id, tool_name, &rendered.text)?;
    let path = extract_persisted_path(&marker).map(PathBuf::from);
    // Index the offloaded blob so the model can BM25-retrieve only the relevant
    // sections via `ctx_search` instead of re-reading the whole file (which would
    // defeat the offload). Indexed even when `ctx_search` is not callable this
    // turn: the blob outlives the turn, and the gates are per turn. Best-effort:
    // on failure the marker's path is still readable.
    let indexed = store.index_output(tool_call_id, tool_name, &rendered.text);
    let sections = indexed.as_ref().map(|o| o.sections);
    // No hint means no callable tool reads this blob (only `ctx_search` is
    // callable and the index did not take it): the same fail-dead handle as an
    // empty recovery set, found after the write. The blob is left to the
    // store's sweep; the caller cuts instead.
    let hint = footer_hint(indexed.as_ref(), recovery, rendered.fenced)?;
    Some(Offloaded {
        footer: format!("{marker}\n{hint}"),
        path,
        sections,
    })
}

/// The line under a persist marker telling the model how to read the blob
/// back — naming only a tool it can call. `ctx_search` when the blob indexed
/// into sections and the tool is callable; otherwise `file_read` when that is
/// callable; otherwise nothing, and the marker's path is the whole handle.
///
/// `fenced` (the producing tool marked the output as external content, see
/// [`Rendered::fenced`]) drops the "First sections:" preview: previews are the
/// sections' first lines, i.e. the untrusted text itself, and this footer sits
/// outside any fence.
fn footer_hint(
    indexed: Option<&IndexOutcome>,
    recovery: RecoveryTools,
    fenced: bool,
) -> Option<String> {
    match indexed.filter(|o| o.sections > 0) {
        Some(outcome) if recovery.ctx_search => Some(search_hint(outcome, !fenced)),
        _ if recovery.file_read => Some(FILE_READ_HINT.to_string()),
        _ => None,
    }
}

/// Footer hint when `ctx_search` cannot be offered but `file_read` can.
const FILE_READ_HINT: &str =
    "[Read it back with file_read on that path — page it with offset/limit]";

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

/// Lift the UI presentation a tool attached under
/// [`aleph_protocol::PRESENTATION_KEY`] out of its JSON and REMOVE the key,
/// so the model-facing text (built from `value` right after) never carries
/// it. Only a top-level object key is honoured — a nested one is a tool bug
/// the census (`presentation_census`) will name.
#[must_use]
pub fn hoist_presentation(value: &mut serde_json::Value) -> Option<aleph_protocol::Presentation> {
    let obj = value.as_object_mut()?;
    let raw = obj.remove(aleph_protocol::PRESENTATION_KEY)?;
    match serde_json::from_value::<aleph_protocol::Presentation>(raw) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(error = %e, "tool attached a `_presentation` that is not a Presentation; dropped");
            None
        }
    }
}

/// Recursion bound for [`hoist_walk`]. Tool results are serialized from Rust
/// structs or MCP payloads — both shallow — so this only guards against
/// pathologically deep JSON (e.g. a page that smuggled a nested document into
/// an `evaluate` result).
const MAX_HOIST_DEPTH: usize = 16;
/// Minimum head length kept by [`truncate_with_budget`] even when the
/// computed char budget rounds to 0. Without this floor, a budget that is
/// fully consumed by the footer (or any degenerate input) collapses the
/// body to a header that lies about how much was actually dropped — the
/// caller sees a `[output truncated, ~N tokens omitted]` marker on text
/// that was preserved verbatim. One MAX_LINE_CHARS-equivalent headroom is
/// enough to keep the model from drawing the wrong conclusion; the body
/// is still bounded by the real budget when the budget is non-degenerate.
const MIN_BODY_HEAD_CHARS: usize = crate::tool_output::distill::MIN_BODY_HEAD_CHARS;

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
/// offloaded blob instead of re-reading the whole file, and — when `previews`
/// — lists the first few section titles as orientation. Kept to a few hundred
/// bytes so the offload's token saving is preserved.
fn search_hint(outcome: &IndexOutcome, previews: bool) -> String {
    // With a single section the "First sections:" preview is the head of the one
    // section — i.e. text the model already has immediately above this hint.
    // Orientation is only worth its bytes when there is something to choose
    // between.
    //
    // A preview is a section's first line, i.e. tool output — possibly a web
    // page's or an MCP server's — and this hint sits OUTSIDE any fence the
    // result carried. It goes through the same scrub as unfenced external text
    // so a line that spells a fence marker or a chat-template token cannot act
    // as one here.
    let preview = if previews && outcome.sections > 1 {
        let previews: Vec<String> = outcome
            .previews
            .iter()
            .map(|p| crate::security::content_sanitizer::sanitize_external_text(p))
            .collect();
        previews.join(" · ")
    } else {
        String::new()
    };
    if preview.is_empty() {
        format!(
            "[Indexed {} sections — use ctx_search(queries=[\"…\"]) to retrieve only \
             the relevant parts instead of re-reading the whole file]",
            outcome.sections
        )
    } else {
        format!(
            "[Indexed {} sections — use ctx_search(queries=[\"…\"]) to retrieve only the \
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
    // Reserve a minimum head so a degenerate budget (footer alone eats the
    // budget, or `chars_for_token_budget` rounds to 0) does not silently
    // collapse the body to a header that lies about what was kept.
    let target_chars = target_chars.max(MIN_BODY_HEAD_CHARS);
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

    /// The preview's line cap is a budget knob, not a constant: the knob
    /// reference budget reproduces the shipped 8 lines exactly, a tighter
    /// budget — the default one included — shrinks it, and the floor keeps it
    /// from shrinking past usefulness.
    #[test]
    fn the_error_preview_scales_with_the_budget() {
        let mut text = String::from("running 2001 tests\n");
        for i in 0..30 {
            text.push_str(&format!("error: failure number {i} at f{i}.rs:1\n"));
        }
        text.push_str(&"padding to exceed the distiller's size floor\n".repeat(40));

        let count_errors =
            |digest: &str| digest.lines().filter(|l| l.starts_with("error:")).count();
        let reference = inline_error_digest(
            &text,
            Some(crate::tool_output::KNOB_REFERENCE_BUDGET_TOKENS),
        )
        .expect("distills");
        assert_eq!(
            count_errors(&reference),
            8,
            "the reference budget reproduces the shipped 8-line cap:\n{reference}"
        );
        let no_budget = inline_error_digest(&text, None).expect("distills");
        assert_eq!(no_budget, reference, "None is the reference behaviour");
        let default_n = count_errors(
            &inline_error_digest(&text, Some(DEFAULT_RESULT_BUDGET_TOKENS)).expect("distills"),
        );
        assert!(
            (2..8).contains(&default_n),
            "the default budget is below the reference, so it tightens: {default_n}"
        );
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

    /// A result carrying its own offload marker with a large tail AFTER the
    /// footer — `browser_exec`'s shape: a cut snapshot at step 1, then later
    /// steps' reads — is bounded by the budget as a whole, the footer kept
    /// whole. Keeping "everything from the marker on" left the tail unbounded.
    ///
    /// Mutation-checked: keeping the text from the marker onward whole again
    /// turns this red.
    #[test]
    fn an_own_marker_result_with_a_large_tail_is_bounded_as_a_whole() {
        let (_scratch, store, _base) = test_store("own_marker_tail");
        let (footer, _) = recovery_footer_for(
            Some(&store),
            "call_exec",
            "browser_exec",
            &"- generic \"row\" [ref=e1]\n".repeat(4_000),
            0,
            RecoveryTools::ALL,
        )
        .expect("the snapshot is offloaded");
        let text = format!(
            "step 1 snapshot (cut)\n{footer}\nstep 2 read:\n{}\nstep 3 read:\n{}",
            "a".repeat(20_000),
            "b".repeat(20_000)
        );
        let out = apply_result_budget(
            "call_exec",
            "browser_exec",
            &text,
            Some(&store),
            Some(4_000),
            None,
            RecoveryTools::ALL,
        );
        assert!(
            out.persisted_path.is_none(),
            "never persisted over its own blob"
        );
        assert!(out.tokens_in_context <= 4_000, "{}", out.tokens_in_context);
        assert!(out.text.contains(&footer), "the footer is kept whole");
    }

    /// With no retrieval tool callable, nothing is offloaded — a blob the model
    /// has no tool to open is a dead handle — and the cut says the rest is gone.
    /// Layer 2 here; Layer 3's spill through `spill_replacement`, with and
    /// without a store, stays within the residue the turn budget credits.
    ///
    /// Mutation-checked: dropping the empty-set check in `offload` turns this
    /// red.
    #[test]
    fn an_empty_recovery_set_cuts_and_says_so_instead_of_offloading() {
        let (_scratch, store, _base) = test_store("empty_recovery");
        let none = RecoveryTools {
            ctx_search: false,
            file_read: false,
        };
        let big = "line of plain build output\n".repeat(4_000);
        let out = apply_result_budget(
            "call_none",
            "bash",
            &big,
            Some(&store),
            Some(500),
            None,
            none,
        );
        assert!(out.persisted_path.is_none(), "{}", out.text);
        assert!(
            !store.blob_path("call_none", "bash").exists(),
            "no blob is written for a handle nothing can open"
        );
        assert!(
            !out.text.contains("[Full output persisted: "),
            "{}",
            out.text
        );
        assert!(out.text.contains(NOT_SAVED_NOTE), "{}", out.text);
        assert!(
            out.tokens_in_context <= 500 + 16,
            "{}",
            out.tokens_in_context
        );

        let tokens = estimate_tokens_smart(&big);
        for store in [Some(&store), None] {
            let spilled = spill_replacement(store, "call_none", "bash", &big, none);
            assert!(spilled.contains(NOT_SAVED_NOTE), "{spilled}");
            assert!(!spilled.contains("[Full output persisted: "), "{spilled}");
            assert!(
                estimate_tokens_smart(&spilled)
                    <= crate::tools::turn_budget::spill_residue_tokens(tokens) + 16,
                "within the credited residue"
            );
        }
    }

    /// Only `ctx_search` callable and the index cannot take the blob: the
    /// footer would name no tool that reads it, so the offload is declined
    /// after the write and the result is cut instead — the same dead handle as
    /// an empty recovery set, found late.
    ///
    /// Mutation-checked: footing with the bare marker when no hint applies
    /// turns this red.
    #[test]
    fn a_blob_no_callable_tool_can_read_is_not_handed_over() {
        let (_scratch, base) = crate::utils::scratch::scratch_root();
        // A directory where the index database should be: it cannot open.
        std::fs::create_dir_all(base.join("index.db")).unwrap();
        let store = ToolResultStore::with_dir_for_tests(base);
        let search_only = RecoveryTools {
            ctx_search: true,
            file_read: false,
        };
        let big = "line of plain build output\n".repeat(4_000);
        let out = apply_result_budget(
            "call_unindexed",
            "bash",
            &big,
            Some(&store),
            Some(500),
            None,
            search_only,
        );
        assert!(out.persisted_path.is_none(), "{}", out.text);
        assert!(
            !out.text.contains("[Full output persisted: "),
            "{}",
            out.text
        );
    }

    /// A result over its budget but no larger than the marker that would
    /// replace it is never offloaded: the swap would grow the context.
    ///
    /// Mutation-checked: dropping the marker floor in `offload` turns this red.
    #[test]
    fn a_result_no_larger_than_its_marker_is_not_offloaded() {
        let (_scratch, store, _base) = test_store("marker_floor");
        let out = apply_result_budget(
            "call_floor",
            "bash",
            "short result text",
            Some(&store),
            Some(1),
            None,
            RecoveryTools::ALL,
        );
        assert!(out.persisted_path.is_none(), "{}", out.text);
        assert!(
            !out.text.contains("[Full output persisted: "),
            "{}",
            out.text
        );
    }

    /// A result carrying the marker of its own call's blob (a tool that
    /// offloaded its output itself, like `web_fetch`'s fetch by intent) is
    /// never persisted again, even over budget: the blob is named by the call
    /// id, and a second write would replace the original with this result.
    /// Recognised as written and as it reads inside flattened JSON.
    ///
    /// Mutation-checked: dropping the own-marker check turns this red.
    #[test]
    fn a_result_carrying_its_own_offload_marker_is_not_persisted_over_it() {
        let (_scratch, store, _base) = test_store("own_marker");
        let original = "original page line\n".repeat(3_000);
        let (footer, _) = recovery_footer_for(
            Some(&store),
            "call_own",
            "web_fetch",
            &original,
            0,
            RecoveryTools::ALL,
        )
        .expect("the store takes the original");
        let sections = format!("{}\n{footer}", "selected section text ".repeat(2_000));
        let flattened = serde_json::json!({ "content": sections }).to_string();
        for text in [&sections, &flattened] {
            let out = apply_result_budget(
                "call_own",
                "web_fetch",
                text,
                Some(&store),
                Some(100),
                None,
                RecoveryTools::ALL,
            );
            assert!(out.persisted_path.is_none(), "persisted again");
            let blob = std::fs::read_to_string(store.blob_path("call_own", "web_fetch"))
                .expect("the blob is still there");
            assert!(blob.contains("original page line"), "blob replaced");
            assert!(!blob.contains("selected section text"), "blob replaced");
            assert!(out.text.len() < text.len(), "over budget, still cut");
            let own = format!(
                "{}{}",
                crate::tools::result_store::PERSISTED_REF_PREFIX,
                store.blob_path("call_own", "web_fetch").display()
            );
            assert!(out.text.contains(&own), "the handle is kept whole");
        }
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
    // hoist_presentation — the UI diff side-channel
    // ---------------------------------------------------------------

    #[test]
    fn hoist_presentation_removes_the_key_and_returns_the_typed_value() {
        let p = aleph_protocol::Presentation::FileChanges { changes: vec![] };
        let mut v = serde_json::json!({"success": true, "_presentation": serde_json::to_value(&p).unwrap()});
        assert_eq!(hoist_presentation(&mut v), Some(p));
        assert!(v.get("_presentation").is_none());
        assert_eq!(v["success"], true);
    }

    #[test]
    fn hoist_presentation_is_none_for_strings_and_for_a_malformed_payload() {
        let mut s = serde_json::json!("plain text");
        assert!(hoist_presentation(&mut s).is_none());
        let mut bad = serde_json::json!({"_presentation": {"kind": "nope"}});
        assert!(hoist_presentation(&mut bad).is_none());
        assert!(
            bad.get("_presentation").is_none(),
            "a malformed payload is still removed from the model text"
        );
    }

    // ---------------------------------------------------------------
    // resolve_result_budget
    // ---------------------------------------------------------------

    #[test]
    fn the_read_family_always_returns_none() {
        assert_eq!(resolve_result_budget("file_read", None), None);
        // Even an explicit setting cannot override the read-recursion guard.
        assert_eq!(resolve_result_budget("file_read", Some(99_999)), None);
        // The spellings the family used to list name no tool anything
        // registers, so they get the default like any other name.
        assert_eq!(
            resolve_result_budget_under("read_file", None, usize::MAX),
            Some(DEFAULT_RESULT_BUDGET_TOKENS)
        );
        assert_eq!(
            resolve_result_budget_under("Read", None, usize::MAX),
            Some(DEFAULT_RESULT_BUDGET_TOKENS)
        );
    }

    #[test]
    fn explicit_wins_over_the_default() {
        assert_eq!(resolve_result_budget("bash", Some(123)), Some(123));
        assert_eq!(resolve_result_budget("custom_thing", Some(50)), Some(50));
    }

    /// The name table is gone: a name carries no budget of its own, so a tool
    /// that declares nothing gets the default whatever it is called — the
    /// declaration is the only per-tool source.
    #[test]
    fn a_tool_name_alone_carries_no_budget() {
        for name in ["web_fetch", "Grep", "search_files", "bash"] {
            assert_eq!(
                resolve_result_budget_under(name, None, usize::MAX),
                Some(DEFAULT_RESULT_BUDGET_TOKENS),
                "{name}"
            );
        }
    }

    #[test]
    fn unknown_tool_falls_back_to_default() {
        assert_eq!(
            resolve_result_budget("some_other_tool", None),
            Some(DEFAULT_RESULT_BUDGET_TOKENS)
        );
    }

    // ---------------------------------------------------------------
    // window ceiling (B14)
    // ---------------------------------------------------------------

    #[test]
    fn window_ceiling_caps_declared_budgets_not_just_the_default() {
        // A 16k-window model yields a 2_400 per-result ceiling. A declared
        // budget above it is exactly the value that must come down — a ceiling
        // applied only to the `None` fallback would leave the biggest offender
        // untouched.
        let ceiling = 2_400;
        assert_eq!(
            resolve_result_budget_under("web_fetch", Some(10_000), ceiling),
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
            resolve_result_budget_under("file_read", None, ceiling),
            None
        );
    }

    #[test]
    fn uncapped_ceiling_is_todays_behavior() {
        // No ceiling installed (large window / no `[context_budget]`) → a
        // declared budget and the default pass through unchanged.
        assert_eq!(
            resolve_result_budget_under("web_fetch", Some(10_000), usize::MAX),
            Some(10_000)
        );
        assert_eq!(
            resolve_result_budget_under("bash", None, usize::MAX),
            Some(DEFAULT_RESULT_BUDGET_TOKENS)
        );
    }

    #[test]
    fn ceiling_at_or_above_the_read_window_is_refused() {
        // A large-window model must not install a ceiling at all: one at or
        // above the largest per-result budget clamps nothing. The installer
        // drops it, so the global stays uncapped. (Never call it here with a
        // value below `MAX_RESULT_BUDGET_TOKENS`: that installs a process-wide
        // ceiling under every other test in this binary.)
        set_global_result_budget_ceiling(MAX_RESULT_BUDGET_TOKENS);
        set_global_result_budget_ceiling(50_000);
        assert_eq!(
            resolve_result_budget("web_fetch", Some(10_000)),
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
        let out = apply_result_budget(
            "c1",
            "bash",
            "hello",
            Some(&store),
            Some(10_000),
            None,
            RecoveryTools::ALL,
        );
        assert_eq!(out.text, "hello");
        assert!(out.persisted_path.is_none());
    }

    #[test]
    fn budget_none_truncates_no_persist() {
        let (_scratch, store, base) = test_store("budget_none");
        let big = "x".repeat(60_000);
        let out = apply_result_budget(
            "c2",
            "file_read",
            &big,
            Some(&store),
            None,
            None,
            RecoveryTools::ALL,
        );
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
        let out = apply_result_budget(
            "c3",
            "bash",
            &big,
            Some(&store),
            Some(100),
            None,
            RecoveryTools::ALL,
        );
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

    /// The offload gate is measured on the flattened text the model would
    /// otherwise receive, not on the rendering that gets stored. The estimator
    /// charges one-line JSON at the code ratio and rendered prose at a cheaper
    /// one, so gating on the rendering would call this result "small", skip
    /// the offload and send the caller to truncation.
    #[test]
    fn the_offload_gate_reads_the_flat_text_not_its_rendering() {
        let (_scratch, store, _base) = test_store("gate_on_flat");
        let prose = (0..200)
            .map(|i| format!("plain sentence number {i} about nothing in particular"))
            .collect::<Vec<_>>()
            .join("\n");
        let flat = serde_json::json!({ "stdout": prose, "exit_code": 0 }).to_string();
        let rendered = crate::tool_output::render::line_preserving(&flat).text;
        let threshold = (estimate_tokens_smart(&flat) + estimate_tokens_smart(&rendered)) / 2;
        assert!(
            estimate_tokens_smart(&rendered) <= threshold
                && threshold < estimate_tokens_smart(&flat),
            "precondition: the two estimates straddle the threshold ({} / {threshold} / {})",
            estimate_tokens_smart(&rendered),
            estimate_tokens_smart(&flat)
        );

        let footer = recovery_footer_for(
            Some(&store),
            "c-gate",
            "bash",
            &flat,
            threshold,
            RecoveryTools::ALL,
        );

        let (_, path) = footer.expect("over the threshold as the model would see it ⇒ offloaded");
        let blob = std::fs::read_to_string(path.expect("a path")).unwrap();
        assert_eq!(blob, rendered, "and what is stored is the rendering");
    }

    /// An opaque typed result — hygiene found nothing to reduce because every
    /// field is one short line — used to reach the model as the bare marker:
    /// the error digest read the flat envelope, which is one line. Read off the
    /// rendering, the error line is found. The source is not fenced, so the
    /// "First sections:" orientation stays.
    #[test]
    fn an_opaque_typed_result_gets_its_error_digest_from_the_rendering() {
        let (_scratch, store, _base) = test_store("opaque_digest");
        let mut steps: Vec<serde_json::Value> = (0..400)
            .map(|i| serde_json::json!({ "name": format!("step {i}"), "status": "ok" }))
            .collect();
        steps.push(serde_json::json!({
            "name": "link",
            "status": "error: linker failed with exit code 1",
        }));
        let mut value = serde_json::json!({ "steps": steps });
        let outcome = crate::tool_output::ingress::clean_for_ingress("bash", &mut value, Some(300));
        assert!(
            outcome.reduced_from.is_none(),
            "precondition: opaque — hygiene had nothing to reduce"
        );

        let out = apply_result_budget(
            "c-opaque-typed",
            "bash",
            &outcome.model_facing,
            Some(&store),
            Some(300),
            None,
            RecoveryTools::ALL,
        );

        assert!(out.persisted_path.is_some());
        assert!(
            out.text.contains("Output digest") && out.text.contains("linker failed"),
            "the error line must be inlined above the marker: {}",
            out.text
        );
        assert!(
            out.text.contains("First sections:"),
            "an unfenced source keeps its orientation preview: {}",
            out.text
        );
    }

    /// For output its tool fenced as external content, the footer carries the
    /// marker and the retrieval hint only: no section-title preview and no
    /// error digest, because both are the untrusted text itself and the footer
    /// sits outside the fence. "Fenced" is read off the fields by the same
    /// test the ingress rewrites use — not from a list of tools.
    #[test]
    fn a_fenced_source_gets_no_preview_and_no_digest_outside_its_fence() {
        use crate::security::content_sanitizer::{wrap_external_content, ContentSource};
        let (_scratch, store, _base) = test_store("fenced_footer");
        let src = ContentSource::McpTool {
            server: "srv".into(),
            tool: "rows".into(),
        };
        let mut blocks: Vec<serde_json::Value> = (0..300)
            .map(|i| {
                serde_json::json!({
                    "type": "text",
                    "text": wrap_external_content(&format!("row {i} is fine"), src.clone()),
                })
            })
            .collect();
        blocks.push(serde_json::json!({
            "type": "text",
            "text": wrap_external_content("error: linker failed with exit code 1", src.clone()),
        }));
        let mut value = serde_json::json!({ "content": blocks });
        let outcome =
            crate::tool_output::ingress::clean_for_ingress("srv__rows", &mut value, Some(300));
        assert!(
            outcome.reduced_from.is_none(),
            "precondition: opaque — no field is big enough to reduce"
        );

        let out = apply_result_budget(
            "c-fenced",
            "srv__rows",
            &outcome.model_facing,
            Some(&store),
            Some(300),
            None,
            RecoveryTools::ALL,
        );

        assert!(
            out.text.contains("[Full output persisted: "),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("ctx_search("),
            "the handle stays: {}",
            out.text
        );
        assert!(!out.text.contains("First sections:"), "{}", out.text);
        assert!(
            !out.text.contains("linker failed"),
            "untrusted lines must not sit outside the fence: {}",
            out.text
        );
    }

    #[test]
    fn no_store_means_truncate_only() {
        let big = "z".repeat(40_000);
        let out = apply_result_budget(
            "c4",
            "bash",
            &big,
            None,
            Some(100),
            None,
            RecoveryTools::ALL,
        );
        assert!(out.persisted_path.is_none());
        assert!(!out.text.starts_with("[Full output persisted:"));
        assert!(
            out.text.contains("[output truncated"),
            "expected truncate marker: {}",
            &out.text[..120.min(out.text.len())]
        );
    }

    /// The parse itself now lives beside the writer
    /// (`result_store::extract_persisted_path`); this keeps the assertion that
    /// THIS module's `recovery_footer_for` still gets a path back out of the marker
    /// it just produced, which is the part `recovery_footer_for`'s callers rely on.
    #[test]
    fn parse_marker_path_roundtrip() {
        let marker = "[Full output persisted: /tmp/aleph/x.txt (1234 tokens, bash)]";
        let path = extract_persisted_path(marker)
            .map(PathBuf::from)
            .expect("parse");
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

        let out = apply_result_budget(
            "c-distill",
            "file_read",
            &big,
            Some(&store),
            None,
            None,
            RecoveryTools::ALL,
        );
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
            RecoveryTools::ALL,
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
            RecoveryTools::ALL,
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
        let out = apply_result_budget(
            "c-opaque",
            "bash",
            &big,
            Some(&store),
            Some(100),
            None,
            RecoveryTools::ALL,
        );
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
        let out = apply_result_budget(
            "c-persist",
            "bash",
            &big,
            Some(&store),
            Some(100),
            None,
            RecoveryTools::ALL,
        );
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
