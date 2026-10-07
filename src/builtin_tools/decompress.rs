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
//! ([`crate::tools::result_processing::is_read_family`]): offloading a
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

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{notify_tool_result, notify_tool_start};
use crate::error::{AlephError, Result};
use crate::session::events::{EventSeq, SessionEvent};
use crate::session::service::SessionId;
use crate::session::store::SessionEventStore;
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

    /// Token cap for this page. Defaults to the read-family backstop
    /// (`read_backstop_tokens`). A single event that alone exceeds the cap is
    /// still emitted — a page must always make progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
}

/// Output from the `session_decompress` tool.
#[derive(Debug, Clone, Serialize)]
pub struct SessionDecompressResult {
    /// The fold this page was restored from (empty when there is no fold).
    pub fold_id: String,
    /// First seq of the fold span this page belongs to.
    pub from_seq: u64,
    /// Last seq of the fold span (inclusive).
    pub to_seq: u64,
    /// The verbatim transcript page — one `[role #seq]: body` line per
    /// content-bearing event, in seq order. Concatenating every page's
    /// `rendered` reproduces the whole fold's transcript exactly.
    pub rendered: String,
    /// True when `max_tokens` cut the page before the span's end.
    pub truncated: bool,
    /// Continuation coordinate: pass as `from_seq` (with the same `fold_id`)
    /// to read the next page. `None` when the page reached the span's end.
    pub next_from_seq: Option<u64>,
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
         max_tokens bounds one page, and a truncated result carries next_from_seq to continue \
         with. Division of labor: recall_events SEARCHES this session's past by keyword, \
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

/// The restore itself, split from the process-global resolution above so the
/// guard tests can drive it against a real in-memory store.
///
/// Order of operations: load the WHOLE log including retired rows (a fold
/// whose own record was hard-retired must still resolve, so that its error
/// can say "erased" instead of "not found") → derive the fold list → resolve
/// the requested fold → read its span → refuse hard-retired spans → render
/// and paginate.
pub(crate) async fn decompress_fold_span(
    store: &dyn SessionEventStore,
    session_id: &SessionId,
    session_key: &str,
    args: &SessionDecompressArgs,
) -> Result<SessionDecompressResult> {
    // The WHOLE log, retired rows included: a fold whose own record was
    // hard-retired must still resolve, so that its failure can say "erased"
    // instead of "not found".
    let rows = store
        .load_events_with_retirement(session_id, None, None)
        .await
        .map_err(|e| AlephError::tool(format!("session_decompress: event log read failed: {e}")))?;
    let events: Vec<SessionEvent> = rows.iter().map(|r| r.record.event.clone()).collect();
    let folds = crate::context::compact::folds::list_folds(session_key, &events);

    if folds.is_empty() {
        return Ok(SessionDecompressResult {
            fold_id: String::new(),
            from_seq: 0,
            to_seq: 0,
            rendered: String::new(),
            truncated: false,
            next_from_seq: None,
            note: Some(
                "no folds recorded in this session — nothing has been compacted, so the \
                 conversation you already see is the whole of it"
                    .to_string(),
            ),
        });
    }

    // Default = the most recent fold (list_folds is seq-monotonic, so the
    // last entry is the latest span).
    let fold = match &args.fold_id {
        None => folds.last().expect("folds is non-empty").clone(),
        Some(id) => folds
            .iter()
            .find(|f| &f.fold_id == id)
            .cloned()
            .ok_or_else(|| {
                let known = folds
                    .iter()
                    .map(|f| f.fold_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                AlephError::tool(format!(
                    "session_decompress: fold '{id}' not found in this session; \
                     known folds: [{known}]"
                ))
            })?,
    };

    // Page bounds must stay inside the fold's span — a page that reaches
    // past it would render events the fold never folded.
    let start = args.from_seq.unwrap_or(fold.from_seq);
    let end = args.to_seq.unwrap_or(fold.to_seq);
    if start < fold.from_seq || end > fold.to_seq || start > end {
        return Err(AlephError::tool(format!(
            "session_decompress: page [{start}..={end}] leaves the fold's span \
             [{}..={}]",
            fold.from_seq, fold.to_seq
        )));
    }

    let span: Vec<_> = rows
        .iter()
        .filter(|r| r.record.seq >= start && r.record.seq <= end)
        .collect();
    if span.is_empty() {
        return Err(AlephError::tool(format!(
            "session_decompress: fold '{}' span {start}..={end} has no rows in the event \
             log — the fold record survives but its events are gone",
            fold.fold_id
        )));
    }

    // The hard-retirement witness: a content-bearing row whose BM25 mirror
    // is gone. `Retire::Through` keeps the mirror; `Retire::From` deletes it
    // in the same transaction — so a missing mirror on a retired content row
    // means clear/rewind erased this span. Restoring anyway would hand over
    // payloads the user asked to erase AND (partially erased span) a
    // half-transcript dressed as the original; refuse honestly instead.
    let hard_retired = span
        .iter()
        .any(|r| r.retired && !r.fts_mirror_present && content_body(&r.record.event).is_some());
    if hard_retired {
        return Err(AlephError::tool(format!(
            "session_decompress: fold '{}' span {start}..={end} was hard-retired — erased by \
             clear/rewind, not compacted. The original text is no longer recallable; only its \
             summary remains.",
            fold.fold_id
        )));
    }

    let lines: Vec<(EventSeq, String)> = span
        .iter()
        .filter_map(|r| render_line(r.record.seq, &r.record.event).map(|line| (r.record.seq, line)))
        .collect();
    if lines.is_empty() {
        return Ok(SessionDecompressResult {
            fold_id: fold.fold_id,
            from_seq: start,
            to_seq: end,
            rendered: String::new(),
            truncated: false,
            next_from_seq: None,
            note: Some(
                "the fold's span holds no content-bearing events (markers/checkpoints only)"
                    .to_string(),
            ),
        });
    }

    // Paginate at event granularity against the token cap. A first line that
    // alone exceeds the cap is still emitted: a page must always make
    // progress, or the continuation coordinate would loop.
    let cap = args
        .max_tokens
        .unwrap_or_else(crate::tools::result_processing::read_backstop_tokens);
    let mut used = 0usize;
    let mut page: Vec<&str> = Vec::new();
    let mut next_from_seq = None;
    for (seq, line) in &lines {
        let cost = crate::context::budget::pressure::estimate_tokens_smart(line) + 1;
        if !page.is_empty() && used + cost > cap {
            next_from_seq = Some(*seq);
            break;
        }
        used += cost;
        page.push(line);
    }

    Ok(SessionDecompressResult {
        fold_id: fold.fold_id,
        from_seq: start,
        to_seq: end,
        rendered: page.join("\n"),
        truncated: next_from_seq.is_some(),
        next_from_seq,
        note: None,
    })
}

/// An event's `(role label, verbatim body)` when it bears content, `None`
/// otherwise. The trim filter is the same one `render_event_text` applies
/// before writing the FTS mirror, so "content-bearing" means exactly "had a
/// mirror row" — the identity the hard-retirement witness above relies on.
fn content_body(event: &SessionEvent) -> Option<(&'static str, std::borrow::Cow<'_, str>)> {
    let (label, body) = crate::session::store::event_content_text(event)?;
    if body.trim().is_empty() {
        None
    } else {
        Some((label, body))
    }
}

/// Render one event as a transcript line, `None` for events with no content
/// body (turn markers, checkpoints, the fold records themselves).
///
/// The label and the body come from one place —
/// [`crate::session::store::event_content_text`] — so the set of
/// content-bearing variants cannot drift between indexing and restoration.
fn render_line(seq: EventSeq, event: &SessionEvent) -> Option<String> {
    let (label, body) = content_body(event)?;
    Some(format!("[{label} #{seq}]: {body}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{now_ms, Durability, MessageContent, Retire, ToolOutput};
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

        // Force tiny pages: ~20 tokens ≈ one rendered line.
        let page = |from: Option<u64>| SessionDecompressArgs {
            fold_id: Some(fold_id.clone()),
            from_seq: from,
            max_tokens: Some(20),
            ..Default::default()
        };
        let mut rendered = String::new();
        let mut from = None;
        let mut pages = 0usize;
        loop {
            let out = decompress(&store, key, &page(from)).await.unwrap();
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
