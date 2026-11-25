//! `SessionDecompressTool` — verbatim restoration of a folded span (Context
//! Fabric, spec 2026-10-01 §3.1b).
//!
//! A manual `/compact` replaces a span of this session's events with a summary
//! and soft-retires the span (`Retire::Through`): the rows stay on disk, the
//! FTS mirror survives, and the fold is made addressable by
//! [`crate::context::compact::folds`]. This tool is the way back: by `fold_id`
//! (or the most recent fold when omitted) it re-reads the retired rows and
//! renders them as a verbatim transcript, one line per content-bearing event.
//!
//! # Channel discipline
//!
//! The restored text rides the ordinary tool-result channel — no transient
//! injection path, no turn-prefix edit (spec §3.1b architecture decision). It
//! therefore MUST be in the read family
//! (`crate::tools::result_processing::is_read_family`): offloading a
//! restoration result would be circular ("the only way back from an offloaded
//! read is another read"), so Layer 2 gives it no budget and
//! `read_backstop_tokens` is the ceiling.
//!
//! # Honesty rules (hard constraints)
//!
//! - Only `Retire::Through` folds restore. A span whose FTS mirror rows are
//!   gone was hit by `Retire::From` (`chat.clear` / `chat.rewind` — the mirror
//!   delete is the one on-disk witness that distinguishes erasure from
//!   compaction); the call then fails with an error that NAMES the erasure
//!   rather than rendering half a transcript. Better a false refusal than
//!   fabricated "original" text.
//! - Fold resolution does NOT sniff `[Context Summary]` bodies: it reads the
//!   `FoldRecorded` / `CompactionPerformed` coordinates from the event log
//!   directly (a user interjection wrapped in the same fence would be
//!   misread).
//! - No fold at all is NOT an error: the result is honestly empty with a
//!   `note` ("nothing was compacted" is a different statement from "the tool
//!   did not run").

use std::fmt::Write as _;
use std::ops::ControlFlow;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{notify_tool_result, notify_tool_start};
use crate::context::budget::pressure::{chars_for_result_token_budget, estimate_tokens_smart};
use crate::error::{AlephError, Result};
use crate::session::events::EventSeq;
use crate::session::service::SessionId;
use crate::session::store::{
    event_content_text, EventContent, FoldRecoverySnapshot, FoldRestoreSpan, FoldSpanStatus,
    SessionEventStore,
};
use crate::tools::AlephTool;

/// Arguments for the `session_decompress` tool.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct SessionDecompressArgs {
    /// Injected by registry — serialized session key (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub __session_key: String,

    /// Which fold to restore (from `session_compact` output or a prior
    /// decompress). Omit to restore the MOST RECENT fold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fold_id: Option<String>,

    /// Page start within the fold (inclusive seq). Must be inside the fold's
    /// span; out-of-span values are an error, not a silent clamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_seq: Option<u64>,

    /// Page end within the fold (inclusive seq). Same bounds rule as
    /// `from_seq`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_seq: Option<u64>,

    /// UTF-8 byte offset in the body at `from_seq`, excluding the display label.
    /// Pass the prior page's `next_from_byte` with the same `fold_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_byte: Option<usize>,

    /// Token cap on the serialized result. Cannot exceed the read backstop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
}

/// Output from the `session_decompress` tool.
#[derive(Debug, Clone, Serialize)]
pub struct SessionDecompressResult {
    /// The fold this page was restored from (empty when there is no fold).
    pub fold_id: String,
    /// First seq addressed by this request.
    pub from_seq: u64,
    /// Last seq requested (inclusive).
    pub to_seq: u64,
    /// Verbatim transcript page. A fragmented event repeats its role/seq
    /// label; concatenate its body slices without adding a delimiter.
    pub rendered: String,
    /// True when `max_tokens` cut the page before the span's end.
    pub truncated: bool,
    /// Continuation coordinate: pass as `from_seq` (with the same `fold_id`)
    /// to read the next page. `None` when the page reached the span's end.
    pub next_from_seq: Option<u64>,
    /// Optional byte continuation within the same event body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_from_byte: Option<usize>,
    /// Human/model-facing note for the honest-empty cases (no folds recorded,
    /// fold spans no renderable events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Tool that verbatim-restores a folded span of the current session.
#[derive(Clone, Default)]
pub struct SessionDecompressTool;

impl SessionDecompressTool {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AlephTool for SessionDecompressTool {
    const NAME: &'static str = "session_decompress";
    const DESCRIPTION: &'static str =
        "Verbatim-restore a folded (compacted) span of THIS session. After session_compact, the \
         folded turns live on as a summary; this tool brings back the exact original events of \
         one fold — pass fold_id, or omit it to restore the most recent fold. Long folds page: \
         max_tokens bounds one serialized page. To continue, keep fold_id and to_seq, pass \
         next_from_seq as from_seq, and pass next_from_byte as from_byte only when present. \
         Division of labor: recall_events SEARCHES this session's past by keyword, \
         ctx_search retrieves offloaded tool OUTPUT, session_search covers OTHER sessions — use \
         this tool when you need the folded span itself, word for word. It errors honestly when \
         the fold_id is unknown, when the page bounds leave the fold's span, or when the span \
         was erased by clear/rewind (hard retirement is not restorable).";

    type Args = SessionDecompressArgs;
    type Output = SessionDecompressResult;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(Self::NAME, args.fold_id.as_deref().unwrap_or("(latest)"));

        if args.__session_key.is_empty() {
            return Err(AlephError::tool(
                "session_decompress: no session context available (session key not injected)",
            ));
        }
        let session_id = SessionId::from_key_string(&args.__session_key).ok_or_else(|| {
            AlephError::tool(format!(
                "session_decompress: failed to parse session key '{}'",
                args.__session_key
            ))
        })?;
        let store = crate::session::store::global_session_event_store().ok_or_else(|| {
            AlephError::tool(
                "session_decompress: session event log not available in this deployment",
            )
        })?;

        // Fold ids are derived from the CANONICAL key string (the manual
        // compaction path derives them from `to_key_string()`), so normalize
        // before resolving — a legacy-shaped incoming key would otherwise
        // derive ids that match nothing.
        let canonical_key = session_id.to_key_string();
        let out = decompress_fold_span(store.as_ref(), &session_id, &canonical_key, &args).await;
        match &out {
            Ok(r) => notify_tool_result(
                Self::NAME,
                &format!("fold {} (truncated={})", r.fold_id, r.truncated),
                true,
            ),
            Err(e) => notify_tool_result(Self::NAME, &e.to_string(), false),
        }
        out
    }
}

/// Keep fold discovery, erase authorization, and page construction inside
/// the same read snapshot; notify lifecycle consumers only after releasing it.
pub(crate) async fn decompress_fold_span(
    store: &dyn SessionEventStore,
    session_id: &SessionId,
    session_key: &str,
    args: &SessionDecompressArgs,
) -> Result<SessionDecompressResult> {
    let mut built_page = None;
    store
        .with_fold_recovery(session_id, &mut |snapshot| {
            built_page = Some(build_page(snapshot, session_key, args));
            Ok(())
        })
        .await
        .map_err(restore_storage_error)?;
    let page = built_page.ok_or_else(|| {
        AlephError::tool("session_decompress: store did not provide a snapshot")
    })??;
    if !page.fold_id.is_empty() {
        crate::context::fabric::tool_admission::notify_current_lifecycle(
            crate::context::fabric::ContextLifecycleEvent::RestoreRequested,
        )
        .await;
    }
    Ok(page)
}

fn restore_storage_error(error: crate::session::service::SessionError) -> AlephError {
    AlephError::tool(format!(
        "session_decompress: event log read failed: {error}"
    ))
}

fn build_page(
    snapshot: &dyn FoldRecoverySnapshot,
    session_key: &str,
    args: &SessionDecompressArgs,
) -> Result<SessionDecompressResult> {
    if args.from_byte.is_some() && (args.fold_id.is_none() || args.from_seq.is_none()) {
        return Err(AlephError::tool(
            "session_decompress: from_byte requires fold_id and from_seq",
        ));
    }
    let backstop = crate::tools::result_processing::read_backstop_tokens();
    let cap = args.max_tokens.unwrap_or(backstop).min(backstop);
    let events = snapshot.fold_events().map_err(restore_storage_error)?;
    let folds = crate::context::compact::folds::list_folds(session_key, &events);
    let mut page = SessionDecompressResult {
        fold_id: String::new(),
        from_seq: 0,
        to_seq: 0,
        rendered: String::new(),
        truncated: false,
        next_from_seq: None,
        next_from_byte: None,
        note: None,
    };
    let fold = match &args.fold_id {
        Some(id) => folds.iter().find(|f| &f.fold_id == id).ok_or_else(|| {
            let known = folds
                .iter()
                .take(8)
                .map(|fold| display_id(&fold.fold_id))
                .collect::<Vec<_>>()
                .join(", ");
            AlephError::tool(format!(
                "session_decompress: fold '{}' not found in this session; known folds (up to 8): [{known}]",
                display_id(id)
            ))
        })?,
        None => match folds.last() {
            Some(fold) => fold,
            None => {
                page.note = Some(
                    "no folds recorded in this session — nothing has been compacted, so the \
                     conversation you already see is the whole of it"
                        .into(),
                );
                return checked_page(page, cap);
            }
        },
    };

    let start = args.from_seq.unwrap_or(fold.from_seq);
    let end = args.to_seq.unwrap_or(fold.to_seq);
    let span = FoldRestoreSpan::new(start, end)
        .filter(|_| start >= fold.from_seq && end <= fold.to_seq)
        .ok_or_else(|| {
            AlephError::tool(format!(
                "session_decompress: page [{start}..={end}] is invalid or leaves the fold's span \
                 [{}..={}]",
                fold.from_seq, fold.to_seq
            ))
        })?;
    page.fold_id.clone_from(&fold.fold_id);
    page.from_seq = start;
    page.to_seq = end;
    if let crate::context::compact::folds::FoldRecoveryRoute::ForeignOwner { owner } =
        fold.recovery_route()
    {
        page.note = Some(format!(
            "degraded recovery: fold owner '{}' is not mounted; Core restored the readable span with bounded builtin recovery",
            display_id(&owner)
        ));
    }

    let mut offset = args.from_byte;
    // Compact JSON charges at most 2.5 chars per estimated token; a code
    // point can occupy four bytes. This is a conservative copy ceiling.
    let budget = PageBudget {
        tokens: cap,
        max_bytes: chars_for_result_token_budget(cap).saturating_mul(4),
    };
    let mut render_error = None;
    let mut pending = None;
    let status = snapshot
        .visit_verified_span(span, &mut |record| {
            let body = event_content_text(&record.event);
            let byte = match offset.take() {
                Some(byte) if record.seq == start && body.is_some() => byte,
                Some(_) => {
                    render_error = Some(invalid_byte_offset());
                    return ControlFlow::Break(());
                }
                None => 0,
            };
            let Some((label, body)) = body else {
                return ControlFlow::Continue(());
            };
            let added = (|| {
                if let Some(line) = pending.take() {
                    if finish_line(&mut page, line, Some(record.seq), cap)?.is_break() {
                        return Ok(ControlFlow::Break(()));
                    }
                }
                let mut window = BodyWindow::new(byte, budget.window_bytes());
                let text = window.body(&body)?;
                append_event(
                    &mut page,
                    record.seq,
                    label,
                    text,
                    byte,
                    budget,
                    &mut pending,
                )
            })();
            match added {
                Ok(flow) => flow,
                Err(error) => {
                    render_error = Some(error);
                    ControlFlow::Break(())
                }
            }
        })
        .map_err(restore_storage_error)?;
    match status {
        FoldSpanStatus::Denied => {
            return Err(AlephError::tool(format!(
                "session_decompress: fold '{}' span {start}..={end} was hard-retired — erased by \
                 clear/rewind, not compacted. The original text is no longer recallable; only its \
                 summary remains.",
                display_id(&fold.fold_id)
            )));
        }
        FoldSpanStatus::Empty if offset.is_none() => {
            return Err(AlephError::tool(format!(
                "session_decompress: fold '{}' span {start}..={end} has no rows in the event log",
                display_id(&fold.fold_id)
            )));
        }
        _ => {}
    }
    if let Some(error) = render_error {
        return Err(error);
    }
    if offset.is_some() {
        return Err(invalid_byte_offset());
    }
    if let Some(line) = pending {
        let _ = finish_line(&mut page, line, None, cap)?;
    }
    if page.rendered.is_empty() && page.note.is_none() {
        page.note = Some(
            "the fold's span holds no content-bearing events (markers/checkpoints only)".into(),
        );
    }
    checked_page(page, cap)
}

fn display_id(id: &str) -> &str {
    &id[..id.floor_char_boundary(id.len().min(128))]
}

fn invalid_byte_offset() -> AlephError {
    AlephError::tool(
        "session_decompress: from_byte must address a UTF-8 boundary inside the first event body",
    )
}

fn page_too_small() -> AlephError {
    AlephError::tool(
        "session_decompress: max_tokens cannot fit the response envelope and required content",
    )
}

impl SessionDecompressResult {
    fn set_next(&mut self, seq: Option<EventSeq>, byte: Option<usize>) {
        self.truncated = seq.is_some();
        self.next_from_seq = seq;
        self.next_from_byte = byte;
    }

    fn fits(&self, cap: usize) -> bool {
        // Ingress flattens the compact Value, not just the transcript body.
        serde_json::to_value(self)
            .is_ok_and(|value| estimate_tokens_smart(&value.to_string()) <= cap)
    }
}

fn checked_page(page: SessionDecompressResult, cap: usize) -> Result<SessionDecompressResult> {
    if page.fits(cap) {
        Ok(page)
    } else {
        Err(page_too_small())
    }
}

// Serialize structured bodies up to this page's window. Skipping a byte
// prefix costs a scan, not an allocation proportional to the prefix or body.
struct BodyWindow {
    skip: usize,
    limit: usize,
    bytes: Vec<u8>,
    more: bool,
}

impl BodyWindow {
    fn new(skip: usize, limit: usize) -> Self {
        Self {
            skip,
            limit,
            bytes: Vec::new(),
            more: false,
        }
    }

    fn body<'a>(&'a mut self, content: &EventContent<'a>) -> Result<&'a str> {
        if let EventContent::Text(text) = content {
            return text
                .get(self.skip..)
                .filter(|body| !body.is_empty())
                .ok_or_else(invalid_byte_offset);
        }
        if let Err(error) = content.write_to(self) {
            if !self.more {
                return Err(AlephError::tool(format!(
                    "session_decompress: cannot render event body: {error}"
                )));
            }
        }
        if self.bytes.is_empty() {
            return Err(invalid_byte_offset());
        }
        // The last chunk may end in a code point. The four extra bytes let
        // append_event distinguish a clipped window from a complete body.
        match std::str::from_utf8(&self.bytes) {
            Ok(body) => Ok(body),
            Err(error) if error.error_len().is_none() && self.more => {
                Ok(std::str::from_utf8(&self.bytes[..error.valid_up_to()])
                    .expect("validated prefix"))
            }
            Err(_) => Err(invalid_byte_offset()),
        }
    }
}

impl std::io::Write for BodyWindow {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let skipped = self.skip.min(bytes.len());
        self.skip -= skipped;
        let rest = &bytes[skipped..];
        let copied = rest.len().min(self.limit - self.bytes.len());
        self.bytes.extend_from_slice(&rest[..copied]);
        if copied != rest.len() {
            self.more = true;
            return Err(std::io::Error::other("restore body window filled"));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct PageBudget {
    tokens: usize,
    max_bytes: usize,
}

impl PageBudget {
    fn window_bytes(self) -> usize {
        self.max_bytes.saturating_add(4)
    }
}

struct PendingLine {
    seq: EventSeq,
    from_byte: usize,
    prior_len: usize,
    label_end: usize,
}

fn append_event(
    page: &mut SessionDecompressResult,
    seq: EventSeq,
    label: &str,
    body: &str,
    from_byte: usize,
    budget: PageBudget,
    pending: &mut Option<PendingLine>,
) -> Result<ControlFlow<()>> {
    let cap = budget.tokens;
    let prior_len = page.rendered.len();
    if prior_len != 0 {
        page.rendered.push('\n');
    }
    write!(page.rendered, "[{label} #{seq}]: ").expect("writing to a String cannot fail");
    let line = PendingLine {
        seq,
        from_byte,
        prior_len,
        label_end: page.rendered.len(),
    };
    if body.len() <= budget.max_bytes {
        page.rendered.push_str(body);
        let terminal = page.fits(cap);
        if seq < page.to_seq {
            // A dismissed candidate must not consume a lookahead. Only wait
            // for the actual next seq when the envelope can change the answer.
            let shortest = fits_continuation(page, seq + 1, cap);
            let longest = fits_continuation(page, page.to_seq, cap);
            if terminal && longest {
                return Ok(ControlFlow::Continue(()));
            }
            if terminal || shortest {
                *pending = Some(line);
                return Ok(ControlFlow::Continue(()));
            }
        } else if terminal {
            return Ok(ControlFlow::Continue(()));
        }
    }
    let end = body.floor_char_boundary(body.len().min(budget.window_bytes()));
    fragment_or_defer_line(page, line, &body[..end], cap)
}

fn fits_continuation(page: &mut SessionDecompressResult, seq: EventSeq, cap: usize) -> bool {
    page.set_next(Some(seq), None);
    let fits = page.fits(cap);
    page.set_next(None, None);
    fits
}

fn finish_line(
    page: &mut SessionDecompressResult,
    line: PendingLine,
    next_seq: Option<EventSeq>,
    cap: usize,
) -> Result<ControlFlow<()>> {
    let fits = match next_seq {
        Some(seq) => fits_continuation(page, seq, cap),
        None => page.fits(cap),
    };
    if fits {
        return Ok(ControlFlow::Continue(()));
    }
    let body = page.rendered.split_off(line.label_end);
    fragment_or_defer_line(page, line, &body, cap)
}

fn fragment_or_defer_line(
    page: &mut SessionDecompressResult,
    line: PendingLine,
    body: &str,
    cap: usize,
) -> Result<ControlFlow<()>> {
    let PendingLine {
        seq,
        from_byte,
        prior_len,
        label_end,
    } = line;
    if prior_len != 0 {
        page.rendered.truncate(prior_len);
        page.set_next(Some(seq), None);
        return Ok(ControlFlow::Break(()));
    }

    // One event exceeds an empty page. Search only a budget-bounded prefix,
    // measuring escaped JSON with the real continuation on each probe.
    // Compact JSON has a monotonic estimate as characters and cursor digits
    // are added. Snap probes to code point boundaries without copying offsets.
    let mut best = 0;
    let mut high = body.len().saturating_sub(1);
    while best < high {
        let probe = best + (high - best).div_ceil(2);
        let above = body.ceil_char_boundary(probe);
        let end = if above <= high {
            above
        } else {
            body.floor_char_boundary(probe)
        };
        if end == best {
            break;
        }
        page.rendered.truncate(label_end);
        page.rendered.push_str(&body[..end]);
        page.set_next(Some(seq), Some(from_byte + end));
        if page.fits(cap) {
            best = end;
        } else {
            high = end - 1;
        }
    }
    if best == 0 {
        return Err(page_too_small());
    }
    page.rendered.truncate(label_end);
    page.rendered.push_str(&body[..best]);
    page.set_next(Some(seq), Some(from_byte + best));
    Ok(ControlFlow::Break(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{
        now_ms, Durability, MessageContent, Retire, SessionEvent, ToolOutput,
    };
    use crate::session::store::{migrate_add_session_events, SqliteEventStore};
    use crate::sync_primitives::Arc;

    fn test_store() -> Arc<SqliteEventStore> {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory sqlite");
        migrate_add_session_events(&conn).expect("migrate session_events");
        Arc::new(SqliteEventStore::new(conn))
    }

    fn session(key: &str) -> SessionId {
        SessionId::from_key_string(key).expect("test session key parses")
    }

    /// The canonical key string the fold ids are derived from (the tool
    /// normalizes through `to_key_string`; fixtures must match).
    fn canonical(key: &str) -> String {
        session(key).to_key_string()
    }

    fn user_ev(text: &str) -> SessionEvent {
        SessionEvent::UserMessage {
            turn_id: uuid::Uuid::new_v4(),
            content: MessageContent {
                text: text.to_string(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            synthetic: false,
            author_user_id: None,
            at: now_ms(),
        }
    }

    fn assistant_ev(text: &str) -> SessionEvent {
        SessionEvent::AssistantMessage {
            turn_id: uuid::Uuid::new_v4(),
            content: MessageContent {
                text: text.to_string(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            usage: None,
            at: now_ms(),
            model: None,
            model_provider: None,
            model_generation: None,
        }
    }

    fn tool_result_ev(call_id: &str, value: serde_json::Value) -> SessionEvent {
        SessionEvent::ToolResult {
            turn_id: uuid::Uuid::new_v4(),
            call_id: call_id.to_string(),
            output: ToolOutput {
                value,
                metadata: Default::default(),
            },
            at: now_ms(),
        }
    }

    /// The fold record emitted for span [from_seq, to_seq], mirroring the
    /// manual path's batch shape.
    fn fold_recorded(key: &str, from_seq: u64, to_seq: u64) -> SessionEvent {
        SessionEvent::FoldRecorded {
            fold_id: crate::context::compact::folds::derive_fold_id(&canonical(key), from_seq),
            from_seq,
            to_seq,
            summary_ref: uuid::Uuid::new_v4().to_string(),
            strategy: "manual".to_string(),
            trigger: "manual-command".to_string(),
            folded_tokens: 500,
            summary_tokens: 50,
            owner: None,
            at: now_ms(),
        }
    }

    fn args_all() -> SessionDecompressArgs {
        SessionDecompressArgs::default()
    }

    /// Append `events` at seq 1.., then fold `[1, fold_through]` in a second
    /// batch (summary + checkpoint + fold record + `Retire::Through`), the
    /// same shape `manual::compact_session` commits. Returns the fold_id.
    async fn compacted_session(
        store: &Arc<SqliteEventStore>,
        key: &str,
        events: Vec<SessionEvent>,
        fold_through: u64,
        with_fold_record: bool,
    ) -> String {
        let id = session(key);
        let n = events.len() as u64;
        let rows: Vec<(SessionEvent, i64)> = events.into_iter().map(|e| (e, now_ms())).collect();
        store
            .append_batch(&id, 1, &rows, None, Durability::Normal)
            .await
            .expect("append conversation");

        let live = fold_through as usize;
        let mut batch = vec![
            (
                SessionEvent::SystemMessage {
                    turn_id: uuid::Uuid::new_v4(),
                    content: "[Context Summary]\nsummary body".to_string(),
                    at: now_ms(),
                },
                now_ms(),
            ),
            (
                SessionEvent::CompactionPerformed {
                    from_seq: 1,
                    to_seq: fold_through,
                    summary_ref: uuid::Uuid::new_v4().to_string(),
                    at: now_ms(),
                },
                now_ms(),
            ),
        ];
        let fold_id = crate::context::compact::folds::derive_fold_id(&canonical(key), 1);
        if with_fold_record {
            batch.push((fold_recorded(key, 1, fold_through), now_ms()));
        }
        store
            .append_batch(
                &id,
                n + 1,
                &batch,
                Some(Retire::Through {
                    through: fold_through,
                    live,
                }),
                Durability::Normal,
            )
            .await
            .expect("compaction batch");
        fold_id
    }

    async fn decompress(
        store: &Arc<SqliteEventStore>,
        key: &str,
        args: &SessionDecompressArgs,
    ) -> Result<SessionDecompressResult> {
        decompress_fold_span(store.as_ref(), &session(key), &canonical(key), args).await
    }

    #[tokio::test]
    async fn restore_capability_preserves_cursor_and_uses_only_the_injected_session() {
        use crate::executor::ToolRegistry as _;
        use crate::tools::{ScopedToolService, ToolHandlerRegistry, ToolService as _};

        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let builtin = Arc::new(crate::executor::BuiltinToolRegistry::new().await.unwrap());
        let store = crate::session::store::install_test_event_store();
        let key = "agent:restore-capability:main";
        let other_key = "agent:restore-capability-other:main";
        let backstop = crate::tools::result_processing::read_backstop_tokens();
        let body = serde_json::json!({"result": "你好\n\"😀".repeat(backstop)});
        let fold_id = compacted_session(
            &store,
            key,
            vec![tool_result_ev("restore-capability-call", body)],
            1,
            true,
        )
        .await;
        let other_fold =
            compacted_session(&store, other_key, vec![user_ev("other session")], 1, true).await;
        let args = serde_json::json!({"__session_key": other_key, "max_tokens": backstop});
        let denied = builtin
            .execute_tool("session_decompress", args.clone())
            .await
            .unwrap_err();
        assert!(denied
            .to_string()
            .contains("no authoritative session context"));

        let handlers = Arc::new(ToolHandlerRegistry::new());
        let _registration = crate::tools::handlers::builtin::register_builtin_routers(
            &handlers,
            builtin,
            ["session_decompress".to_owned()],
        )
        .await
        .unwrap();
        let mut registry = crate::tools::runtime::LoopToolRegistry::new();
        registry.bind_canonical_registry(handlers.clone(), Arc::new(|_| true));
        let facade = crate::capability::zahir_facade::ZahirFacade::new(
            handlers.as_ref().clone(),
            Arc::new(crate::capability::ownership::OwnershipTree::new()),
        );
        assert!(registry.bind_canonical_facade(facade));
        let entry = handlers.resolve_entry("session_decompress").unwrap();
        registry.register(Box::new(
            crate::tools::adapters::McpRegistryTool::from_registry_entry(
                entry.handler.clone(),
                &entry.descriptor,
            ),
        ));
        let svc = ScopedToolService::new(Arc::new(registry), Default::default()).with_turn_context(
            crate::tools::turn_context::TurnContext {
                session_key: crate::routing::session_key::SessionKey::main("restore-capability"),
                run_id: String::new(),
                channel_id: "test".into(),
                conversation_id: "test".into(),
                caller_role: None,
                channel_tool_permissions: None,
                unattended: false,
                plan_gate: None,
                side_question: false,
                telecom_authority: None,
            },
        );
        let id = session(key);
        let before = store.load_head_seq(&id).await.unwrap();
        let out = crate::approval::with_call_identity(
            Some(crate::approval::CallIdentity {
                turn_id: uuid::Uuid::new_v4(),
                call_id: "restore-capability-page".into(),
            }),
            svc.execute("session_decompress", args),
        )
        .await
        .unwrap();
        let text = out
            .value
            .as_str()
            .expect("model receives compact JSON text");
        let page: serde_json::Value = serde_json::from_str(text).expect("page survived Layer 2");
        assert_eq!(page["fold_id"], fold_id);
        assert_ne!(page["fold_id"], other_fold);
        assert_eq!(page["next_from_seq"], 1);
        assert!(page["next_from_byte"].as_u64().unwrap() > 0);
        assert!(estimate_tokens_smart(text) <= backstop);
        assert_eq!(store.load_head_seq(&id).await.unwrap(), before);
    }

    #[tokio::test]
    async fn terminal_cursor_envelope_cannot_turn_a_valid_fragment_into_an_error() {
        let store = test_store();
        let key = "agent:main:peer:dec-envelope";
        compacted_session(
            &store,
            key,
            vec![user_ev(&"a".repeat(83)), user_ev("  ")],
            2,
            true,
        )
        .await;
        let page = decompress(
            &store,
            key,
            &SessionDecompressArgs {
                max_tokens: Some(79),
                ..args_all()
            },
        )
        .await
        .unwrap();
        assert_eq!(page.next_from_seq, Some(1));
        assert!(page.next_from_byte.unwrap() > 0);
        assert!(page.fits(79));
    }

    #[tokio::test]
    async fn tight_budget_can_fit_a_continuation_but_not_a_terminal_envelope() {
        let store = test_store();
        let key = "agent:main:peer:dec-continuation-envelope";
        let fold_id =
            compacted_session(&store, key, vec![user_ev("a"), user_ev("b")], 2, true).await;
        let args = SessionDecompressArgs {
            fold_id: Some(fold_id),
            max_tokens: Some(46),
            ..args_all()
        };
        let mut page = decompress(&store, key, &args).await.unwrap();
        assert_eq!(page.rendered, "[user #1]: a");
        assert_eq!(page.next_from_seq, Some(2));
        assert!(page.fits(46));
        page.set_next(None, None);
        assert!(!page.fits(46));
        let tail = decompress(
            &store,
            key,
            &SessionDecompressArgs {
                from_seq: Some(2),
                max_tokens: Some(48),
                ..args
            },
        )
        .await
        .unwrap();
        assert_eq!(tail.rendered, "[user #2]: b");
        assert!(!tail.truncated);
    }

    #[tokio::test]
    #[ignore = "manual SQLite recovery measurement; no wall-clock threshold"]
    async fn measure_fold_recovery_with_unrelated_tail() {
        for tail in [1_000, 4_000] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("events.sqlite");
            let conn = rusqlite::Connection::open(&path).unwrap();
            migrate_add_session_events(&conn).unwrap();
            let store = Arc::new(SqliteEventStore::new(conn));
            let key = "agent:main:peer:dec-timing";
            let fold_id = compacted_session(
                &store,
                key,
                vec![
                    user_ev("restore this exact line"),
                    assistant_ev("later original"),
                ],
                2,
                true,
            )
            .await;
            let rows: Vec<_> = (0..tail)
                .map(|i| (user_ev(&format!("unrelated live tail {i}")), now_ms()))
                .collect();
            store
                .append_batch(&session(key), 6, &rows, None, Durability::Normal)
                .await
                .unwrap();
            let args = SessionDecompressArgs {
                fold_id: Some(fold_id),
                from_seq: Some(1),
                to_seq: Some(1),
                max_tokens: Some(256),
                ..Default::default()
            };
            let baseline = rusqlite::Connection::open(&path).unwrap();
            for run in 1..=2 {
                // Baseline is the removed SQL + eager row decode, against
                // the exact same file seeded via SessionEventStore. It omits
                // the old caller's subsequent clone, rendering, and paging.
                let started = std::time::Instant::now();
                let decoded = legacy_recovery_read(&baseline, key);
                let old_read = started.elapsed();
                assert_eq!(decoded.len(), tail + 5);
                let started = std::time::Instant::now();
                let page = decompress(&store, key, &args).await.unwrap();
                eprintln!("tail={tail} run={run} legacy_read={old_read:?} candidate_page={:?} legacy_decoded={}", started.elapsed(), decoded.len());
                assert_eq!(page.rendered, "[user #1]: restore this exact line");
                assert!(!page.truncated);
            }
            store.retire_from(&session(key), 2).await.unwrap();
            let late_range = SessionDecompressArgs {
                to_seq: Some(2),
                ..args
            };
            let error = decompress(&store, key, &late_range).await.unwrap_err();
            assert!(error.to_string().contains("hard-retired"));
        }
    }

    fn legacy_recovery_read(
        conn: &rusqlite::Connection,
        key: &str,
    ) -> Vec<(crate::session::events::SessionEventRecord, bool, bool)> {
        let session_json = serde_json::to_string(&session(key)).unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT e.seq, e.payload_json, e.created_at, e.retired_at,
                    (SELECT COUNT(*) FROM session_events_fts f
                     WHERE f.session_id = e.session_id AND f.seq = e.seq)
             FROM session_events e
             WHERE e.session_id = ?1 AND e.seq >= ?2 AND e.seq < ?3
             ORDER BY e.seq ASC",
            )
            .unwrap();
        let mut rows = stmt
            .query(rusqlite::params![session_json, 0, i64::MAX])
            .unwrap();
        let mut decoded = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            let seq: i64 = row.get(0).unwrap();
            let payload: String = row.get(1).unwrap();
            let created_at: i64 = row.get(2).unwrap();
            let retired: Option<i64> = row.get(3).unwrap();
            let witnesses: i64 = row.get(4).unwrap();
            let crate::session::store::DecodedRow::Event(record) =
                crate::session::store::decode_row(
                    u64::try_from(seq).unwrap(),
                    created_at,
                    &payload,
                )
            else {
                panic!("measurement data must decode")
            };
            decoded.push((record, retired.is_some(), witnesses > 0));
        }
        decoded
    }

    #[tokio::test]
    async fn late_erasure_is_not_authorized_by_another_sessions_witness() {
        let temp = tempfile::tempdir().unwrap();
        let conn = rusqlite::Connection::open(temp.path().join("events.sqlite")).unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store = Arc::new(SqliteEventStore::new(conn));
        let key = "agent:main:peer:dec-late-erase";
        let first = "first page ".repeat(32);
        let later = format!("erased after the first page {}", "secret ".repeat(100));
        let events = vec![user_ev(&first), assistant_ev(&later)];
        compacted_session(&store, key, events.clone(), 2, true).await;
        compacted_session(&store, "agent:main:peer:dec-late-other", events, 2, true).await;
        let args = SessionDecompressArgs {
            max_tokens: Some(240),
            ..args_all()
        };
        let page = decompress(&store, key, &args).await.unwrap();
        assert!(page.truncated);
        assert!(!page.rendered.contains("erased after"));
        store.retire_from(&session(key), 2).await.unwrap();
        let err = decompress(&store, key, &args).await.unwrap_err();
        assert!(err.to_string().contains("hard-retired"), "{err}");
    }

    #[tokio::test]
    async fn witnessed_corrupt_suffix_fails_only_when_needed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.sqlite");
        let conn = rusqlite::Connection::open(&path).unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store = Arc::new(SqliteEventStore::new(conn));
        let key = "agent:main:peer:dec-corrupt-suffix";
        let first = "first page ".repeat(32);
        let fold_id = compacted_session(
            &store,
            key,
            vec![
                user_ev(&first),
                assistant_ev(&first),
                assistant_ev("corrupt later"),
            ],
            3,
            true,
        )
        .await;
        let affected = rusqlite::Connection::open(path).unwrap().execute(
            "UPDATE session_events SET payload_json = 'not-json' WHERE session_id = ?1 AND seq = 3",
            [serde_json::to_string(&session(key)).unwrap()],
        ).unwrap();
        assert_eq!(
            affected, 1,
            "the witnessed suffix must actually be corrupted"
        );
        let page = decompress(
            &store,
            key,
            &SessionDecompressArgs {
                max_tokens: Some(240),
                ..args_all()
            },
        )
        .await
        .unwrap();
        assert_eq!(page.next_from_seq, Some(2));
        let err = decompress(
            &store,
            key,
            &SessionDecompressArgs {
                fold_id: Some(fold_id),
                from_seq: Some(3),
                ..args_all()
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("event log read failed"), "{err}");
    }

    #[tokio::test]
    async fn missing_fts_and_unsupported_store_are_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.sqlite");
        let conn = rusqlite::Connection::open(&path).unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store = Arc::new(SqliteEventStore::new(conn));
        let key = "agent:main:peer:dec-missing-fts";
        compacted_session(&store, key, vec![user_ev("retired text")], 1, true).await;
        rusqlite::Connection::open(path)
            .unwrap()
            .execute_batch("DROP TABLE session_events_fts")
            .unwrap();
        let err = decompress(&store, key, &args_all()).await.unwrap_err();
        assert!(err.to_string().contains("event log read failed"), "{err}");

        let unsupported = crate::session::store::test_support::CountingStore::in_memory();
        let err = decompress_fold_span(
            unsupported.as_ref(),
            &session(key),
            &canonical(key),
            &args_all(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("unavailable"), "{err}");
    }

    #[tokio::test]
    async fn structured_tool_body_streams_bounded_verbatim_fragments() {
        let store = test_store();
        let key = "agent:main:peer:dec-structured";
        let value = serde_json::json!({"data": "你🙂\n\"\\".repeat(350), "tail": [true, 7]});
        let expected = value.to_string();
        let mut probe = BodyWindow::new(0, 32);
        let prefix = probe.body(&EventContent::Json(&value)).unwrap();
        assert!(expected.starts_with(prefix));
        assert!(probe.more);
        assert_eq!(probe.bytes.len(), 32);

        let fold_id = compacted_session(
            &store,
            key,
            vec![tool_result_ev("large-result", value)],
            1,
            true,
        )
        .await;
        let mut args = SessionDecompressArgs {
            fold_id: Some(fold_id),
            from_seq: Some(1),
            max_tokens: Some(140),
            ..args_all()
        };
        let mut recovered = String::new();
        loop {
            let page = decompress(&store, key, &args).await.unwrap();
            assert!(page.fits(140));
            recovered.push_str(page.rendered.strip_prefix("[tool_result #1]: ").unwrap());
            let Some(next) = page.next_from_byte else {
                break;
            };
            assert_eq!(page.next_from_seq, Some(1));
            assert_eq!(next, recovered.len());
            assert!(next > args.from_byte.unwrap_or(0));
            args.from_byte = Some(next);
        }
        assert_eq!(recovered, expected);
    }

    #[tokio::test]
    async fn decompress_oversized_unicode_page_reassembles() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.sqlite");
        let reopen = || {
            Arc::new(SqliteEventStore::new(
                rusqlite::Connection::open(&path).unwrap(),
            ))
        };
        let conn = rusqlite::Connection::open(&path).unwrap();
        migrate_add_session_events(&conn).unwrap();
        let mut store = Arc::new(SqliteEventStore::new(conn));
        let key = "agent:main:peer:dec-bytes";
        let body = format!("{}  ", "你🙂 \"quoted\" \\ line\n\t".repeat(400));
        let fold_id = compacted_session(&store, key, vec![user_ev(&body)], 1, true).await;
        let page_args = |seq, byte, cap| {
            serde_json::from_value(serde_json::json!({
                "fold_id": fold_id,
                "from_seq": seq,
                "to_seq": 1,
                "from_byte": byte,
                "max_tokens": cap,
            }))
            .unwrap()
        };
        let cap = 256;
        let mut from = 0;
        let mut recovered = String::new();
        loop {
            let page = decompress(&store, key, &page_args(1, from, cap))
                .await
                .unwrap();
            let value = serde_json::to_value(&page).unwrap();
            assert!(
                crate::context::budget::pressure::estimate_tokens_smart(&value.to_string()) <= cap
            );
            assert_eq!(page.truncated, page.next_from_seq.is_some());
            recovered.push_str(page.rendered.strip_prefix("[user #1]: ").unwrap());
            if !page.truncated {
                break;
            }
            assert_eq!(page.next_from_seq, Some(1));
            let next = value["next_from_byte"].as_u64().unwrap() as usize;
            assert!(next > from && body.is_char_boundary(next));
            if from == 0 {
                drop(store);
                store = reopen();
            }
            from = next;
        }
        assert_eq!(recovered, body);
        for args in [
            page_args(1, 1, cap),
            page_args(1, body.len(), cap),
            page_args(1, usize::MAX, cap),
            page_args(1, 0, 1),
            page_args(u64::MAX, 0, cap),
        ] {
            assert!(decompress(&store, key, &args).await.is_err());
        }
        for value in [
            serde_json::json!({"from_byte": 0, "from_seq": 1}),
            serde_json::json!({"fold_id": fold_id, "from_byte": 0}),
        ] {
            let args = serde_json::from_value(value).unwrap();
            assert!(decompress(&store, key, &args).await.is_err());
        }
        let args = SessionDecompressArgs {
            fold_id: Some(fold_id.clone()),
            ..args_all()
        };
        let err = decompress(&store, "agent:main:peer:dec-wrong-session", &args)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
    }

    /// The headline guarantee (plan Step 1): the restored transcript is the
    /// folded span's original text, byte for byte — an effect assertion on a
    /// really-compacted session, not a mock's self-report.
    #[tokio::test]
    async fn decompress_roundtrip_byte_exact() {
        let store = test_store();
        let key = "agent:main:peer:dec-roundtrip";
        let fold_id = compacted_session(
            &store,
            key,
            vec![
                user_ev("how do I resize the window?"),
                assistant_ev("call SetWindowPos with the new bounds"),
                tool_result_ev("c1", serde_json::json!("exit code 0")),
            ],
            3,
            true,
        )
        .await;

        let out = decompress(&store, key, &args_all())
            .await
            .expect("soft-retired fold restores");
        assert_eq!(out.fold_id, fold_id);
        assert_eq!((out.from_seq, out.to_seq), (1, 3));
        assert!(!out.truncated);
        assert_eq!(out.next_from_seq, None);
        let expected = "[user #1]: how do I resize the window?\n\
                        [assistant #2]: call SetWindowPos with the new bounds\n\
                        [tool_result #3]: exit code 0";
        assert_eq!(
            out.rendered, expected,
            "restored text must equal the folded events' original text byte for byte"
        );

        // Sanity: the span really was retired — the live log no longer
        // contains it, so this test is not reading back live rows.
        let live = store.load_all_events(&session(key)).await.unwrap();
        assert!(
            live.iter().all(|r| r.seq > 3),
            "folded seqs must be retired from the live view"
        );
    }

    /// Omitting fold_id restores the MOST RECENT fold (latest span), not the
    /// first one found.
    #[tokio::test]
    async fn decompress_latest_fold_default() {
        let store = test_store();
        let key = "agent:main:peer:dec-latest";
        let id = session(key);

        // First fold covers [1, 2]; the conversation continues at seq 6..
        // (1 summary + 1 checkpoint + 1 fold record = 3 rows).
        let first = compacted_session(
            &store,
            key,
            vec![user_ev("old question"), assistant_ev("old answer")],
            2,
            true,
        )
        .await;
        // `Retire::Through` retires every live row at or below `through`, so
        // the second fold's span starts at the first LIVE row (seq 3 — the
        // first compaction's summary), not at the new conversation rows.
        let rows: Vec<(SessionEvent, i64)> = vec![
            (user_ev("new question"), now_ms()),
            (assistant_ev("new answer"), now_ms()),
        ];
        store
            .append_batch(&id, 6, &rows, None, Durability::Normal)
            .await
            .unwrap();
        let second_id = crate::context::compact::folds::derive_fold_id(&canonical(key), 3);
        let batch = vec![
            (
                SessionEvent::SystemMessage {
                    turn_id: uuid::Uuid::new_v4(),
                    content: "[Context Summary]\nsecond".to_string(),
                    at: now_ms(),
                },
                now_ms(),
            ),
            (
                SessionEvent::CompactionPerformed {
                    from_seq: 3,
                    to_seq: 7,
                    summary_ref: uuid::Uuid::new_v4().to_string(),
                    at: now_ms(),
                },
                now_ms(),
            ),
            (fold_recorded(key, 3, 7), now_ms()),
        ];
        store
            .append_batch(
                &id,
                8,
                &batch,
                Some(Retire::Through {
                    through: 7,
                    live: 5,
                }),
                Durability::Normal,
            )
            .await
            .unwrap();

        let out = decompress(&store, key, &args_all())
            .await
            .expect("default resolves the latest fold");
        assert_eq!(out.fold_id, second_id, "not the first fold: {first}");
        assert_eq!((out.from_seq, out.to_seq), (3, 7));
        assert!(out.rendered.contains("new question"));

        // …and an explicit fold_id still reaches the older one.
        let out = decompress(
            &store,
            key,
            &SessionDecompressArgs {
                fold_id: Some(first.clone()),
                ..Default::default()
            },
        )
        .await
        .expect("explicit fold_id resolves");
        assert_eq!(out.fold_id, first);
        assert!(out.rendered.contains("old question"));
    }

    /// Two pages concatenated reproduce the whole transcript exactly
    /// (plan: 跨页拼接 == 整段), and `next_from_seq` is the continuation
    /// coordinate that makes it happen.
    #[tokio::test]
    async fn decompress_pagination_reassembles() {
        let store = test_store();
        let key = "agent:main:peer:dec-pages";
        let events: Vec<SessionEvent> = (1..=6)
            .map(|i| assistant_ev(&format!("page line {i} {}", "y".repeat(40))))
            .collect();
        let fold_id = compacted_session(&store, key, events, 6, true).await;

        let full = decompress(&store, key, &args_all()).await.unwrap();
        assert!(!full.truncated, "uncapped restore fits in one page");

        // Fit the serialized envelope and complete lines, but not the entire span.
        let page = |from: Option<u64>| SessionDecompressArgs {
            fold_id: Some(fold_id.clone()),
            from_seq: from,
            max_tokens: Some(140),
            ..Default::default()
        };
        let mut rendered = String::new();
        let mut from = None;
        let mut pages = 0usize;
        loop {
            let out = decompress(&store, key, &page(from)).await.unwrap();
            assert_eq!(out.next_from_byte, None, "each line fits by itself");
            if !rendered.is_empty() && !out.rendered.is_empty() {
                rendered.push('\n');
            }
            rendered.push_str(&out.rendered);
            pages += 1;
            match out.next_from_seq {
                Some(next) if out.truncated => from = Some(next),
                _ => break,
            }
            assert!(pages < 10, "pagination must terminate");
        }
        assert!(pages >= 2, "the cap must actually split the span");
        assert_eq!(
            rendered, full.rendered,
            "pages must concatenate to the whole"
        );
    }

    /// Review Focus #2: a fold whose span was later hard-retired
    /// (`Retire::From` — clear/rewind deletes the FTS mirror) must fail with
    /// an error that says ERASED, not render surviving payload rows and call
    /// it a restoration; and the error must read differently from
    /// "no such fold".
    #[tokio::test]
    async fn decompress_hard_retired_range_errors_honestly() {
        let store = test_store();
        let key = "agent:main:peer:dec-hard";
        let fold_id = compacted_session(
            &store,
            key,
            vec![user_ev("secret plan"), assistant_ev("secret steps")],
            2,
            true,
        )
        .await;

        // chat.clear / rewind shape: retire everything from seq 2 up — this
        // hits span row 2 AND the fold records, and deletes their FTS rows.
        store
            .retire_from(&session(key), 2)
            .await
            .expect("hard retire");

        let err = decompress(
            &store,
            key,
            &SessionDecompressArgs {
                fold_id: Some(fold_id),
                ..Default::default()
            },
        )
        .await
        .expect_err("a hard-retired span must not restore");
        let msg = err.to_string();
        assert!(
            msg.contains("hard-retired") || msg.contains("erased"),
            "the error names the erasure: {msg}"
        );

        let err = decompress(
            &store,
            key,
            &SessionDecompressArgs {
                fold_id: Some("fold_deadbeef_99".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect_err("unknown fold ids fail too");
        let msg = err.to_string();
        assert!(
            msg.contains("no such fold") || msg.contains("not found"),
            "unknown fold reads differently from erased fold: {msg}"
        );
    }

    /// Review Focus #1: a pre-registry session carries only
    /// `CompactionPerformed`; the derived legacy fold must restore the same
    /// way (fold_id = `derive_fold_id`).
    #[tokio::test]
    async fn decompress_legacy_fold_via_compaction_performed() {
        let store = test_store();
        let key = "agent:main:peer:dec-legacy";
        let fold_id = compacted_session(
            &store,
            key,
            vec![user_ev("legacy question"), assistant_ev("legacy answer")],
            2,
            false, // no FoldRecorded — the pre-registry shape
        )
        .await;

        let out = decompress(&store, key, &args_all())
            .await
            .expect("legacy fold restores via the derived id");
        assert_eq!(out.fold_id, fold_id);
        assert_eq!(
            out.rendered,
            "[user #1]: legacy question\n[assistant #2]: legacy answer"
        );
    }

    /// Review Focus #3: folded history can itself contain marker-shaped text
    /// (an assistant once wrote `[Context folded]`, a user pasted a
    /// `<system-reminder>` fence). The restored transcript rides the
    /// tool-result channel and must not read as harness scaffolding: it never
    /// STARTS with the reminder fence, so `is_synthetic_reminder` is false.
    #[tokio::test]
    async fn decompressed_marker_text_not_synthetic() {
        let store = test_store();
        let key = "agent:main:peer:dec-marker";
        compacted_session(
            &store,
            key,
            vec![
                user_ev("<system-reminder>\nnote: [Context folded] earlier\n</system-reminder>"),
                assistant_ev("I wrote [Context Summary] markers in my notes"),
            ],
            2,
            true,
        )
        .await;

        let out = decompress(&store, key, &args_all()).await.unwrap();
        // The marker text survives verbatim inside the transcript…
        assert!(out.rendered.contains("[Context folded]"));
        assert!(out.rendered.contains("<system-reminder>"));
        // …but the transcript as a whole is role-labelled, so the synthetic
        // classifier does not mistake it for scaffolding.
        assert!(
            !crate::thinker::nudges::is_synthetic_reminder(&out.rendered),
            "restored transcript must not read as a synthetic reminder"
        );
    }

    /// Bounds outside the fold's span are named errors, not silent clamps.
    #[tokio::test]
    async fn decompress_out_of_span_page_bounds_error() {
        let store = test_store();
        let key = "agent:main:peer:dec-bounds";
        let fold_id = compacted_session(
            &store,
            key,
            vec![user_ev("a"), assistant_ev("b"), assistant_ev("c")],
            3,
            true,
        )
        .await;

        for (from, to) in [(Some(0), None), (None, Some(4)), (Some(3), Some(2))] {
            let err = decompress(
                &store,
                key,
                &SessionDecompressArgs {
                    fold_id: Some(fold_id.clone()),
                    from_seq: from,
                    to_seq: to,
                    ..Default::default()
                },
            )
            .await
            .expect_err("out-of-span bounds must error");
            assert!(
                err.to_string().contains("span"),
                "the error names the span: {err}"
            );
        }

        // In-span sub-pages are legal.
        let out = decompress(
            &store,
            key,
            &SessionDecompressArgs {
                fold_id: Some(fold_id),
                from_seq: Some(2),
                to_seq: Some(2),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(out.rendered, "[assistant #2]: b");
    }

    /// "没找到 vs 没跑": a session with no folds answers honestly empty with
    /// an advisory note, not an error and not invented content.
    #[tokio::test]
    async fn decompress_no_folds_is_honest_empty() {
        let store = test_store();
        let key = "agent:main:peer:dec-empty";
        let rows: Vec<(SessionEvent, i64)> = vec![(user_ev("nothing folded here"), now_ms())];
        store
            .append_batch(&session(key), 1, &rows, None, Durability::Normal)
            .await
            .unwrap();

        let out = decompress(&store, key, &args_all())
            .await
            .expect("no folds is not an error");
        assert!(out.rendered.is_empty());
        assert!(
            out.note
                .as_deref()
                .is_some_and(|n| n.contains("no folds") || n.contains("nothing")),
            "advisory says why it is empty: {:?}",
            out.note
        );
    }

    #[test]
    fn tool_name_and_description() {
        assert_eq!(SessionDecompressTool::NAME, "session_decompress");
        // The DESCRIPTION carries the division-of-labour contract (plan:
        // 与 ctx_search 的分工必须写明).
        assert!(SessionDecompressTool::DESCRIPTION.contains("ctx_search"));
        assert!(SessionDecompressTool::DESCRIPTION.contains("fold_id"));
    }
}
