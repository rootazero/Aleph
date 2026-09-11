//! `trace.tool_output` — the full text of one tool result, paged, masked.
//!
//! # Where the bytes come from, and why there are three answers
//!
//! The wire `tool_end` frame and the `messages` projection both carry the
//! MODEL-facing copy of a tool result: `apply_result_budget` replaces the value
//! with budgeted text and offloads the original to a blob only when it
//! overflowed. So the honest answer to "show me the whole output" has three
//! shapes, and this method says which one it is served rather than pretending
//! they are the same (`aleph_protocol::ToolOutputSource`):
//!
//! * within budget → the event log's own text IS the whole output
//!   ([`Inline`](aleph_protocol::ToolOutputSource::Inline));
//! * over budget → follow the `[Full output persisted: <path> …]` marker to the
//!   blob ([`Persisted`](aleph_protocol::ToolOutputSource::Persisted));
//! * over budget, blob swept (7-day TTL) or unreadable → the budgeted text is
//!   all that survives, and the page says so
//!   ([`Expired`](aleph_protocol::ToolOutputSource::Expired), `truncated:
//!   true`) instead of presenting a truncation as the whole thing.
//!
//! `Expired` is also what a REFUSED blob path produces, and there are three
//! ways to be refused — the file is gone, it is outside the store root, or it
//! was written for a different call. See [`resolve_source`] for why a marker
//! read back out of a log needs all three; none of them is allowed to become
//! "here are some bytes" (判据 §8).
//!
//! # Visibility
//!
//! An addressed surface like every other: the caller names a session, that
//! session is KeyChecked with [`visibility::session_visible`], and a denial
//! reuses [`visibility::not_found_response`] so a foreign key is
//! byte-identical to a missing one. The event log is read **per session key**,
//! so a `tool_call_id` from someone else's session simply is not in the rows
//! this method looked at — there is no cross-session read to gate separately.
//!
//! # Masking
//!
//! Everything this method serves originates in `session_events` or in a blob
//! that `session_events` points at. Both sit **outside** the write-time
//! redaction invariant (the event log is the MODEL's context and is unmasked by
//! design), and there is no masker downstream of this handler, so the masking
//! happens here. See [`handle_tool_output`] for which side of the seam that is
//! and what it costs.

use std::path::Path;

use aleph_protocol::{ToolOutputPage, ToolOutputSource};
use serde::Deserialize;
use serde_json::json;

use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, RESOURCE_NOT_FOUND, SERVICE_UNAVAILABLE,
};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;
use crate::sync_primitives::Arc;

/// Hard ceiling on one page, whatever `limit` asks for. A tool result blob can
/// be tens of megabytes; a client that wants all of it pages.
pub const MAX_TOOL_OUTPUT_PAGE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Default, Deserialize)]
struct Params {
    #[serde(default)]
    session_key: Option<String>,
    #[serde(default)]
    tool_call_id: Option<String>,
    #[serde(default)]
    offset: u64,
    #[serde(default)]
    limit: Option<u64>,
}

/// Cut `[offset, offset + limit)` out of `text`, on char boundaries.
///
/// Returns the slice, the byte offset it actually **starts** at, and whether
/// anything follows it.
///
/// # The two roundings go opposite ways, and both directions are load-bearing
///
/// `start` rounds **down** and `end` rounds **up**, so a page always covers at
/// least the whole character the request landed in.
///
/// Rounding `end` down instead looks symmetric and is a livelock: `limit: 1`
/// against a text whose first character is two bytes gives `end = 1 → 0`, an
/// empty slice with `truncated: true`, and a client looping `while truncated`
/// never advances — each iteration costing a whole blob read and a whole
/// masking pass. Rounding up costs at most `len_utf8() - 1` extra bytes on the
/// last character.
///
/// # The floor lives here, not at the call site
///
/// `limit: 0` reaches the same livelock through a different door, and by the
/// easiest route there is: at any boundary-aligned offset — which is *every*
/// offset a previous page reported as its own `start + len`, and 0 — it gives
/// `start == end` and an empty page that still says `truncated`. So `limit` is
/// floored at 1 **in this function**.
///
/// That placement is the point. This doc is where the progress property is
/// claimed, so this is where it has to be enforced: a floor applied by the one
/// caller would leave the claim above true only under a precondition stated
/// nowhere, and the second caller would break it silently (判据 §12 — the
/// guarantee and its enforcement derive at one place). The *ceiling* stays with
/// the caller for the mirror-image reason: `MAX_TOOL_OUTPUT_PAGE_BYTES` is a
/// resource policy about how much one response may carry, not a fact about
/// whether this loop terminates.
///
/// A floored `limit: 0` serves one character rather than refusing. Refusing
/// would be defensible, but it makes a wire error out of an input with an
/// obvious honest reading, and a client that sends it is not attacking
/// anything — it is looping.
///
/// Returning `start` (rather than echoing the requested offset) is the other
/// half: `ToolOutputPage`'s own contract is `offset + text.len() <
/// total_bytes ⇒ truncated`, which only holds if `offset` names where the text
/// really begins. A mid-character request would otherwise report an offset one
/// or two bytes past its own first byte, and a client accumulating pages would
/// re-request bytes it already has, forever.
///
/// `start <= end` holds for every input: `start_raw <= end_raw`, flooring is
/// monotone, and `end` only moves up.
fn page(text: &str, offset: u64, limit: u64) -> (String, u64, bool) {
    let total = text.len() as u64;
    let mut start = offset.min(total) as usize;
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    // `text.len()` is always a boundary, so this terminates at or before it.
    let mut end = offset.saturating_add(limit.max(1)).min(total) as usize;
    while !text.is_char_boundary(end) {
        end += 1;
    }
    (
        text[start..end].to_string(),
        start as u64,
        (end as u64) < total,
    )
}

/// Read-only: one tool result's untruncated text, paged.
///
/// Params: `{ session_key, tool_call_id, offset?, limit? }`. Answers
/// `RESOURCE_NOT_FOUND` when this session's event log holds no result under
/// that call id — an `Err`, never an empty page, because "there is no such
/// call" and "the call produced nothing" are different facts and a reader that
/// cannot tell them apart will render the wrong one.
///
/// # Which side of the seam the masking is on
///
/// Its sibling [`handle_by_runs`](super::trace_replay::handle_by_runs) masks
/// almost nothing, and that is correct: the `task_traces` rows it replays are
/// masked **at write** by
/// `execution_engine::unattended_redacting_sink::mask_trace_event`, so masking
/// them again at serve would be redundant work that hides which producer owns
/// the guarantee. The one thing it *does* mask is the presentation it pulls out
/// of `session_events` — and that is the same reason everything here is masked:
///
/// | source | masked where | why |
/// |---|---|---|
/// | `task_traces` rows | at write, by the unattended trace sink | those rows also reach channels and the WS `agent_trace` mirror; one mask at the producer covers every consumer |
/// | `session_events`, and the blobs its markers point at (**everything this method serves**) | at serve, here | the event log is the MODEL's context and is unmasked by design — masking it at write would corrupt what the model reads back |
///
/// So this handler masking everything it emits is not an inconsistency with its
/// sibling masking almost nothing; both follow the same rule about who owns the
/// guarantee. Do not "fix" the asymmetry in either direction.
///
/// Why it is non-optional: the model-facing text of a `file_read` of a `.env`,
/// or of a `bash` run that echoed a token, is that credential in plaintext, and
/// the live `tool_end` frame strips exactly that off `result.output`
/// (`gateway::event_emitter::RedactingEmitter`). Serving it here unmasked would
/// re-open through a different door a hole the same plan already closed once.
///
/// The presentation side-channel has **three** doors that reach a human, not
/// the two this seam account used to name: the live frame, the replay leg
/// (`trace_replay::presentations_for_session`), and the raw value
/// `tools.invoke` returns (`tools_invoke::mask_presentation_in_place`). All
/// three call `exec::masker::mask_presentation`; that call-site list is the
/// census, not this sentence.
///
/// # The asymmetry this leaves on attended runs
///
/// Both write-time redaction legs are installed under `if unattended`
/// (`run_loop/inner.rs:647` and `:1006`), while replay records no
/// attended/unattended signal anywhere — not on the session, not on the event.
/// "Mask exactly when the write path would have" is therefore not
/// implementable here, so this masks unconditionally. On an attended run that
/// means the same bytes the user watched stream past in the clear come back
/// from this method with `***REDACTED***` where a credential was. That is
/// deliberate: the masker only matches credential-shaped strings, so the cost
/// is a redacted credential in the operator's own output, and the alternative
/// is a live credential on a client-facing RPC with no masker downstream. A
/// leak is unrecoverable; a redaction is not.
///
/// # Masked before paging, not after
///
/// The whole text is masked once and the page is cut out of the *masked*
/// string, so `offset` / `total_bytes` describe what is actually served. Cutting
/// first and masking the page would let a secret that straddles a page boundary
/// leave as two unmatched halves — the ends of a paged read are exactly where a
/// per-page masker is blind.
pub async fn handle_tool_output(
    request: JsonRpcRequest,
    sessions: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let params: Params = match request.params.as_ref() {
        Some(v) => match serde_json::from_value(v.clone()) {
            Ok(p) => p,
            Err(_) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid params"),
        },
        None => Params::default(),
    };

    // A malformed/absent key is a validation error, not an existence question —
    // the same split `trace.by_runs` makes.
    let Some(key_str) = params.session_key.as_deref().filter(|s| !s.is_empty()) else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key");
    };
    let Some(session_key) = SessionKey::from_key_string(key_str) else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid session_key");
    };
    let Some(call_id) = params.tool_call_id.as_deref().filter(|s| !s.is_empty()) else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing tool_call_id");
    };

    match sessions.get_metadata(&session_key).await {
        Ok(Some(meta)) if visibility::session_visible(&meta) => {}
        // Foreign owner, missing row, and store error all produce the same
        // response (GC 3 fail-closed, GC 4 no oracle).
        _ => return visibility::not_found_response(request.id),
    }

    let Some(store) = crate::session::store::global_session_event_store() else {
        return JsonRpcResponse::error(
            request.id,
            SERVICE_UNAVAILABLE,
            "session event log not available",
        );
    };
    let events = match store.load_all_events(&session_key).await {
        Ok(events) => events,
        Err(e) => {
            // "We could not read the log" is not "that call does not exist":
            // answering NOT_FOUND here would tell the client to stop asking.
            tracing::warn!(session_key = %key_str, error = %e, "trace.tool_output: event log read failed");
            return JsonRpcResponse::error(
                request.id,
                SERVICE_UNAVAILABLE,
                "session event log unreadable",
            );
        }
    };

    // The join key is the same one `trace_replay::presentations_for_session`
    // uses, and it is genuinely shared rather than assumed: a tool call's
    // `SessionEvent::ToolResult.call_id` and the id every client-facing surface
    // labels it with are both `call.id.clone()` in `harness/agent/act.rs`.
    // Retired events (a rewind, a compaction) are already excluded by
    // `load_events_range`'s `retired_at IS NULL`, so output the user deleted
    // does not come back through this door.
    let Some(text) = events.into_iter().find_map(|rec| match rec.event {
        crate::session::events::SessionEvent::ToolResult {
            call_id: id,
            output,
            ..
        } if id == call_id => Some(match output.value {
            serde_json::Value::String(s) => s,
            other => other.to_string(),
        }),
        _ => None,
    }) else {
        return JsonRpcResponse::error(
            request.id,
            RESOURCE_NOT_FOUND,
            "no tool result with that id in this session",
        );
    };

    // `to_key_string()` rather than the raw `key_str` the caller sent: the
    // writer scopes its store with `request.session_key.to_key_string()`
    // (`run_loop/mod.rs`), so the reader has to reach the directory name
    // through the same derivation or the gate refuses real blobs (判据 §12).
    let (full, source) = resolve_source(text, call_id, &session_key.to_key_string());
    let masked = crate::exec::masker::SecretMasker::new().mask(&full);
    // Ceiling only. The FLOOR (`limit: 0`, which a client can send and nothing
    // upstream rejects) belongs to [`page`], which is what claims to make
    // progress — see its doc for why the two bounds live in different places.
    let limit = params
        .limit
        .unwrap_or(MAX_TOOL_OUTPUT_PAGE_BYTES)
        .min(MAX_TOOL_OUTPUT_PAGE_BYTES);
    let (slice, start, more) = page(&masked, params.offset, limit);
    let out = ToolOutputPage {
        tool_call_id: call_id.to_string(),
        // `truncated` is one bit over two facts on purpose (its own doc in
        // `shared/protocol/src/context_breakdown.rs`): more pages follow, OR
        // the rest is gone for good. `source` is what tells them apart.
        truncated: more || matches!(source, ToolOutputSource::Expired),
        // Where the text REALLY starts, not what was asked for — a request
        // landing inside a multi-byte character is served from that
        // character's first byte. See [`page`].
        offset: start,
        // Bytes of the MASKED text, and therefore stable only within one
        // masker configuration. `exec::masker::operator_patterns()` is
        // runtime-configurable (`[[security.mask_patterns]]`), so an operator
        // adding a pattern between a client's page 1 and page 2 renumbers this
        // underneath it; a client accumulating pages should restart on a
        // `total_bytes` that moves rather than splicing across the change.
        //
        // These coordinates are also NOT `ctx_search`'s: that indexes the
        // UNMASKED blob (`ToolResultStore::index_output` is handed `full`), so
        // its offsets and these are different systems over the same content.
        total_bytes: masked.len() as u64,
        source,
        text: slice,
    };
    JsonRpcResponse::success(request.id, serde_json::to_value(out).unwrap_or(json!(null)))
}

/// Turn the event log's stored text into the text to serve, plus the label that
/// says what it actually is.
///
/// # The marker is data, and both of its gates live in the store
///
/// The marker line is written by the server, but it is read back out of a log
/// whose neighbouring bytes are whatever a tool printed — a `file_read` of an
/// attacker-authored file, a `bash` that echoed one, a fetched page. So the
/// path is checked for containment (inside the store root), for **session**
/// (inside this session's own blob directory, or an earlier epoch of it) and
/// for ownership (written for THIS call). None alone is enough: containment
/// does not exclude another session's blob, ownership is a name prefix that a
/// structurally shorter call id satisfies, and ownership on its own does not
/// exclude `/etc/hosts`.
///
/// The session gate is why this passes `session_key` down rather than reading
/// through the process-wide handle. The store's scope is what arms it: an
/// unscoped handle may read the whole root, so calling it with the global store
/// would leave the gate green in every case (判据 §2) while the module doc
/// above claimed there was "no cross-session read to gate separately" — a claim
/// that was false for exactly this path until 2026-09-11.
///
/// All three gates are inside
/// [`read_call_blob`](crate::tools::result_store::ToolResultStore::read_call_blob)
/// rather than one here and one there, because they have to bind the **same
/// file** and only the store can resolve the path once (判据 §12). Running the
/// name check here on the raw path while the store checks containment on the
/// resolved one is a hole: a symlink under the root whose name matches this
/// call, pointing at another session's blob, satisfies both.
///
/// Every failure — no store installed, path outside the root, wrong call, blob
/// swept, unreadable file — lands on the same honest answer: the budgeted text,
/// labelled `Expired`. "We could not read it" never becomes "here are some
/// bytes" (判据 §8).
fn resolve_source(text: String, call_id: &str, session_key: &str) -> (String, ToolOutputSource) {
    let Some(path) = crate::tools::result_store::extract_persisted_path(&text) else {
        return (text, ToolOutputSource::Inline);
    };
    let blob = crate::tools::result_store::global_tool_result_store()
        .map(|store| crate::tools::result_store::ToolResultStore::for_session(&store, session_key))
        .and_then(|store| store.read_call_blob(Path::new(path), call_id).ok());
    match blob {
        Some(blob) => (blob, ToolOutputSource::Persisted),
        None => (text, ToolOutputSource::Expired),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::session::store::SessionEventStore;
    use serde_json::Value;
    use tempfile::TempDir;

    fn req(params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "trace.tool_output".into(),
            params: Some(params),
            id: Some(json!(1)),
        }
    }

    fn session_store(temp: &TempDir) -> Arc<dyn SessionStore> {
        Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: temp.path().to_path_buf(),
                ..Default::default()
            })
            .unwrap(),
        )
    }

    /// Create `key` owned by `owner`. Mirrors `trace_replay`'s helper minus the
    /// run attribution, which this method does not use — it addresses a call
    /// id, not a run.
    async fn seed_session(sessions: &Arc<dyn SessionStore>, key: &SessionKey, owner: &str) {
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal(owner)),
            sessions.get_or_create(key),
        )
        .await
        .unwrap();
    }

    /// Append one `ToolResult` under `call_id` carrying `value` as its output.
    ///
    /// Uses the crate's shared test event store: the process-global slot is
    /// install-once, so every test in this binary observes the SAME instance
    /// and each keeps to its own session key.
    async fn seed_result(key: &SessionKey, seq: u64, call_id: &str, value: Value) {
        crate::session::store::install_test_event_store()
            .append(
                key,
                seq,
                &crate::session::events::SessionEvent::ToolResult {
                    turn_id: uuid::Uuid::new_v4(),
                    call_id: call_id.to_string(),
                    output: crate::session::events::ToolOutput {
                        value,
                        metadata: crate::session::events::ToolOutputMetadata::default(),
                    },
                    at: seq as i64,
                },
                seq as i64,
            )
            .await
            .unwrap();
    }

    async fn call(
        key: &SessionKey,
        sessions: Arc<dyn SessionStore>,
        params: Value,
    ) -> JsonRpcResponse {
        let mut params = params;
        params["session_key"] = json!(key.to_key_string());
        CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_tool_output(req(params), sessions),
            )
            .await
    }

    fn parse(resp: JsonRpcResponse) -> ToolOutputPage {
        serde_json::from_value(resp.result.expect("success")).expect("ToolOutputPage")
    }

    /// An unknown call id is an error, never an empty page: "no such call" and
    /// "the call produced nothing" render differently and must not collapse.
    #[tokio::test]
    async fn an_unknown_call_id_is_not_found_not_an_empty_page() {
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("conv-tool-output-missing");
        seed_session(&sessions, &key, "u-alice").await;
        seed_result(&key, 1, "call-present", json!("hello")).await;

        let resp = call(&key, sessions, json!({ "tool_call_id": "call-absent" })).await;
        assert!(resp.result.is_none());
        assert_eq!(resp.error.expect("error").code, RESOURCE_NOT_FOUND);
    }

    #[tokio::test]
    async fn an_in_budget_result_is_served_inline_and_paged_on_bytes() {
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("conv-tool-output-inline");
        seed_session(&sessions, &key, "u-alice").await;
        seed_result(&key, 1, "c-inline", json!("hello world")).await;

        let served = parse(
            call(
                &key,
                sessions,
                json!({ "tool_call_id": "c-inline", "offset": 6, "limit": 5 }),
            )
            .await,
        );
        assert_eq!(served.text, "world");
        assert_eq!(served.offset, 6);
        assert_eq!(served.total_bytes, 11);
        assert_eq!(served.source, ToolOutputSource::Inline);
        assert!(
            !served.truncated,
            "the page ends exactly at total_bytes, so nothing follows it"
        );
    }

    /// The over-budget path end to end: a real blob written by the real store,
    /// found through the real marker, read back through the containment check.
    #[tokio::test]
    async fn an_offloaded_result_is_read_back_out_of_its_blob() {
        let store = crate::tools::result_store::install_test_tool_result_store();
        let content = "x".repeat(9000);
        // threshold = 1 token forces the offload.
        let marker = store
            .persist_if_large("c-persisted", "grep", &content, 1)
            .expect("content over the threshold is persisted");

        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("conv-tool-output-persisted");
        seed_session(&sessions, &key, "u-alice").await;
        seed_result(&key, 1, "c-persisted", json!(marker)).await;

        let served = parse(call(&key, sessions, json!({ "tool_call_id": "c-persisted" })).await);
        assert_eq!(served.source, ToolOutputSource::Persisted);
        assert_eq!(served.total_bytes, 9000);
        assert_eq!(served.text, content);
        assert!(!served.truncated);
    }

    /// A marker line naming `path`, the shape `persist_if_large` writes.
    fn marker_for(path: &std::path::Path) -> String {
        format!(
            "[Full output persisted: {} (9000 tokens, grep)]",
            path.display()
        )
    }

    /// A path the store really did write to for `call_id` and no longer has —
    /// the shape the 7-day TTL sweeper leaves behind.
    ///
    /// Derived from a real persist, so the root AND the call-bound file name
    /// are the store's own rather than a guess. The file name matters: a probe
    /// blob named for some other call would be refused one check earlier and
    /// this test would go green without ever reaching the missing-file branch.
    fn swept_blob_path(
        store: &crate::tools::result_store::ToolResultStore,
        call_id: &str,
    ) -> std::path::PathBuf {
        let marker = store
            .persist_if_large(call_id, "grep", &"y".repeat(2000), 1)
            .expect("probe blob is over the threshold");
        let written = std::path::PathBuf::from(
            crate::tools::result_store::extract_persisted_path(&marker).expect("marker path"),
        );
        std::fs::remove_file(&written).expect("sweep the probe blob");
        written
    }

    /// The blob is gone (swept by the TTL, or the process that wrote it used a
    /// different root). The budgeted text is all that survives and the page says
    /// so — it does not present a truncation as the whole output.
    #[tokio::test]
    async fn a_swept_blob_degrades_to_expired_rather_than_lying() {
        let store = crate::tools::result_store::install_test_tool_result_store();
        let marker = marker_for(&swept_blob_path(&store, "c-expired"));

        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("conv-tool-output-expired");
        seed_session(&sessions, &key, "u-alice").await;
        seed_result(&key, 1, "c-expired", json!(marker.clone())).await;

        let served = parse(call(&key, sessions, json!({ "tool_call_id": "c-expired" })).await);
        assert_eq!(served.source, ToolOutputSource::Expired);
        assert!(
            served.truncated,
            "Expired means the rest is gone for good, which the reader must be told"
        );
        assert_eq!(served.text, marker, "the budgeted text is what survives");
    }

    /// C5: a marker pointing at a real, readable file OUTSIDE the store root.
    /// This is a different branch from the swept one above — that exercises
    /// `canonicalize()` failing, this exercises the prefix check — and it is
    /// the branch that decides whether a log line can be turned into an
    /// arbitrary file read.
    ///
    /// The decoy is named `c-outside_grep.txt` ON PURPOSE: that is the file
    /// name this call's own blob would have, so the call binding admits it and
    /// the ROOT check is what refuses. A differently-named decoy would be
    /// rejected one step earlier and this test would pass without ever
    /// exercising the containment it is named for.
    #[tokio::test]
    async fn a_marker_pointing_outside_the_root_serves_the_marker_not_the_file() {
        let _store = crate::tools::result_store::install_test_tool_result_store();
        let (_outside_guard, outside_root) = crate::utils::scratch::scratch_root();
        std::fs::create_dir_all(&outside_root).unwrap();
        let outside = outside_root.join("c-outside_grep.txt");
        std::fs::write(&outside, "OUTSIDE-THE-ROOT-CONTENTS").unwrap();
        assert!(
            crate::tools::result_store::blob_belongs_to_call(&outside, "c-outside"),
            "the decoy must clear the call binding, or the root check is never reached"
        );

        let marker = marker_for(&outside);
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("conv-tool-output-outside");
        seed_session(&sessions, &key, "u-alice").await;
        seed_result(&key, 1, "c-outside", json!(marker.clone())).await;

        let served = parse(call(&key, sessions, json!({ "tool_call_id": "c-outside" })).await);
        assert_eq!(served.source, ToolOutputSource::Expired);
        assert_eq!(served.text, marker);
        assert!(
            !served.text.contains("OUTSIDE-THE-ROOT-CONTENTS"),
            "a refused read must never become the file's contents: {}",
            served.text
        );
    }

    /// The third refusal branch, and the one root containment cannot reach: a
    /// marker INSIDE the store root naming another call's blob.
    ///
    /// A tool's output is whatever the tool printed, and
    /// `extract_persisted_path` scans every line, so a `file_read` of an
    /// attacker-authored file is enough to put a chosen path in the log. Blobs
    /// of every session live under one root, so without the call binding this
    /// would serve a stranger's offloaded output under this call's id.
    #[tokio::test]
    async fn a_marker_naming_another_calls_blob_is_refused() {
        let store = crate::tools::result_store::install_test_tool_result_store();
        let victim = store
            .persist_if_large("c-victim", "grep", &"v".repeat(3000), 1)
            .expect("victim blob is over the threshold");
        let victim_path = std::path::PathBuf::from(
            crate::tools::result_store::extract_persisted_path(&victim).expect("marker path"),
        );
        assert!(
            store.read_call_blob(&victim_path, "c-victim").is_ok(),
            "the victim blob must be readable BY ITS OWN CALL, or this proves nothing"
        );

        // The crafted line, as it would arrive inside some other tool's output.
        let marker = marker_for(&victim_path);
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("conv-tool-output-crossed");
        seed_session(&sessions, &key, "u-alice").await;
        seed_result(&key, 1, "c-thief", json!(marker.clone())).await;

        let served = parse(call(&key, sessions, json!({ "tool_call_id": "c-thief" })).await);
        assert_eq!(served.source, ToolOutputSource::Expired);
        assert_eq!(served.text, marker);
        assert!(
            !served.text.contains(&"v".repeat(3000)),
            "another call's blob must never be served under this call's id"
        );
    }

    /// The hole the call binding cannot cover, closed by the session gate: a
    /// `tool_call_id` that is a **structural prefix** of the victim's.
    ///
    /// `blob_belongs_to_call` tests `name.starts_with("{call}_")` and the
    /// prefix does not have to end at a name boundary, so the id `toolu`
    /// satisfies it for `toolu_01ABC_bash.txt` — every Anthropic-shaped call in
    /// reach. An upstream that supplies tool-call ids verbatim (the
    /// OpenAI-compatible and Gemini paths do) can emit exactly that id, so this
    /// is not hypothetical; what makes it harmless is that "in reach" is now
    /// one session's own directory.
    ///
    /// # What makes this red
    ///
    /// Both halves. Drop the gate in `read_call_blob` and the victim's bytes
    /// are served; drop the `for_session` narrowing in [`resolve_source`] and
    /// the unscoped handle may read the whole root, so the gate is green for
    /// every input and this test is the only thing that says so (判据 §2).
    ///
    /// The victim is written through a handle scoped to ITS OWN session, which
    /// is what production does (`tool_service_builder` narrows the request's
    /// handle) — writing it flat would put it in the unowned root, which is
    /// admissible by design, and this test would pass while proving nothing.
    #[tokio::test]
    async fn a_prefix_shaped_call_id_cannot_reach_another_sessions_blob() {
        let store = crate::tools::result_store::install_test_tool_result_store();
        let victim_session = "conv-tool-output-prefix-victim";
        let victim_store =
            crate::tools::result_store::ToolResultStore::for_session(&store, victim_session);
        let victim = victim_store
            .persist_if_large("toolu_01VICTIM", "grep", &"v".repeat(3000), 1)
            .expect("victim blob is over the threshold");
        let victim_path = std::path::PathBuf::from(
            crate::tools::result_store::extract_persisted_path(&victim).expect("marker path"),
        );
        assert!(
            victim_store
                .read_call_blob(&victim_path, "toolu_01VICTIM")
                .is_ok(),
            "the victim blob must be readable BY ITS OWN SESSION AND CALL, or this proves nothing"
        );
        assert!(
            crate::tools::result_store::blob_belongs_to_call(&victim_path, "toolu"),
            "the short id must clear the CALL binding, or the session gate is never what refuses"
        );

        let marker = marker_for(&victim_path);
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("conv-tool-output-prefix-thief");
        seed_session(&sessions, &key, "u-alice").await;
        seed_result(&key, 1, "toolu", json!(marker.clone())).await;

        let served = parse(call(&key, sessions, json!({ "tool_call_id": "toolu" })).await);
        assert_eq!(served.source, ToolOutputSource::Expired);
        assert_eq!(served.text, marker);
        assert!(
            !served.text.contains(&"v".repeat(3000)),
            "another session's blob must never be served under a prefix-shaped id"
        );
    }

    /// `session_events` is unmasked by design — it is the model's own context —
    /// and nothing downstream of this handler masks. A `file_read` of a `.env`
    /// or a `bash` that echoed a token puts the credential in `output.value`
    /// verbatim, and the live `tool_end` frame strips exactly that.
    ///
    /// Asserts the REDACTION MARKER is present in the served response, not
    /// merely that the secret is absent — "does not contain" passes on any
    /// string that never had it. The secret is the codebase's own example key,
    /// so `SecretMasker`'s pattern decides rather than this fixture.
    #[tokio::test]
    async fn the_served_text_is_masked() {
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("conv-tool-output-secret");
        seed_session(&sessions, &key, "u-alice").await;
        seed_result(
            &key,
            1,
            "c-secret",
            json!("AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE\n"),
        )
        .await;

        let served = parse(call(&key, sessions, json!({ "tool_call_id": "c-secret" })).await);
        assert!(
            served.text.contains("REDACTED"),
            "the served text must carry the redaction marker: {}",
            served.text
        );
        assert!(
            !served.text.contains("AKIAIOSFODNN7EXAMPLE"),
            "and the credential itself must be gone: {}",
            served.text
        );
        assert_eq!(
            served.total_bytes,
            served.text.len() as u64,
            "total_bytes describes the MASKED text — the page is cut after \
             masking so a secret cannot straddle a page boundary"
        );
    }

    /// Same denial as every other addressed surface: a session someone else
    /// owns is byte-identical to a session that does not exist.
    #[tokio::test]
    async fn a_foreign_session_is_not_found() {
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("conv-tool-output-foreign");
        seed_session(&sessions, &key, "u-bob").await;
        seed_result(&key, 1, "c-bob", json!("bob's output")).await;

        let resp = call(&key, sessions, json!({ "tool_call_id": "c-bob" })).await;
        assert!(resp.result.is_none());
        let err = resp.error.expect("error");
        assert_eq!(err.code, RESOURCE_NOT_FOUND);
        assert_eq!(err.message, "session not found");
    }

    #[test]
    fn paging_never_splits_a_char_and_reports_what_follows() {
        // 'h' is byte 0; 'é' spans bytes 1..3, so byte 2 is not a boundary.
        let text = "héllo";
        let (slice, start, more) = page(text, 0, 2);
        assert_eq!(
            slice, "hé",
            "a limit ending mid-character extends to cover it, never truncates it away"
        );
        assert_eq!(start, 0);
        assert!(more);

        let (slice, start, more) = page(text, 0, 100);
        assert_eq!(slice, text);
        assert_eq!(start, 0);
        assert!(!more);

        let (slice, start, more) = page(text, 900, 10);
        assert_eq!(slice, "", "an offset past the end is empty, not a panic");
        assert_eq!(start, text.len() as u64);
        assert!(!more);
    }

    /// The livelock a symmetric "round both ends down" produces, and the
    /// broken invariant it hides. Both halves are about a client that pages by
    /// `offset += text.len()` while `truncated`.
    #[test]
    fn a_page_always_advances_and_reports_where_it_really_starts() {
        // A one-byte limit against a two-byte first character. Rounding `end`
        // DOWN gives ("", true) here — an infinite loop over a client that
        // pays a full blob read and a full masking pass per iteration.
        let two_byte_first = "élan";
        let (slice, start, more) = page(two_byte_first, 0, 1);
        assert_eq!(slice, "é", "a page must cover at least one whole character");
        assert_eq!(start, 0);
        assert!(more);

        // A request landing INSIDE that character is served from its first
        // byte, and says so: `ToolOutputPage`'s contract is
        // `offset + text.len() < total_bytes ⇒ truncated`, which only holds if
        // `offset` names where the text really begins.
        let (slice, start, more) = page(two_byte_first, 1, 1);
        assert_eq!(slice, "é");
        assert_eq!(start, 0, "the REPORTED offset is the rounded one");
        assert!(more);
        assert_eq!(
            start + slice.len() as u64,
            2,
            "so the client's next offset lands on the boundary after it"
        );

        // Whole-text progress: pages tile the string exactly once.
        let mut at = 0u64;
        let mut seen = String::new();
        for _ in 0..16 {
            let (slice, start, more) = page(two_byte_first, at, 1);
            assert_eq!(start, at, "each step starts where the last one ended");
            seen.push_str(&slice);
            at = start + slice.len() as u64;
            if !more {
                break;
            }
        }
        assert_eq!(
            seen, two_byte_first,
            "paging by 1 byte must terminate whole"
        );
    }

    /// The livelock's other door, and the easier one to walk through: `limit: 0`
    /// at a **boundary-aligned** offset — which is every offset a previous page
    /// reported as its own `start + len`, and 0.
    ///
    /// Both of the tests above use `limit >= 1`, so neither can see this: it
    /// needs no multi-byte character at all, just a zero.
    #[test]
    fn a_zero_limit_still_advances() {
        let (slice, start, more) = page("hello", 0, 0);
        assert_eq!(slice, "h", "limit 0 is floored to one character, not empty");
        assert_eq!(start, 0);
        assert!(more, "and it still says the rest follows");

        // Boundary-aligned mid-string, the shape a paging client reaches.
        let (slice, start, more) = page("hello", 3, 0);
        assert_eq!(slice, "l");
        assert_eq!(start, 3);
        assert!(more);

        // A whole character, not a byte, when the floor lands on a wide one.
        let (slice, _, _) = page("élan", 0, 0);
        assert_eq!(slice, "é");

        // Terminates: the loop below spins forever without the floor.
        let text = "hello";
        let mut at = 0u64;
        let mut seen = String::new();
        for _ in 0..32 {
            let (slice, start, more) = page(text, at, 0);
            assert!(
                !slice.is_empty(),
                "an empty page with more to come is the livelock itself"
            );
            seen.push_str(&slice);
            at = start + slice.len() as u64;
            if !more {
                break;
            }
        }
        assert_eq!(seen, text, "limit 0 must tile the whole string");
    }

    /// The floor end to end, because `Params.limit` is an `Option<u64>` with no
    /// validation upstream: `{"limit": 0}` off the wire reaches `page` as a
    /// zero, and the handler applies only a ceiling.
    #[tokio::test]
    async fn a_zero_limit_off_the_wire_still_returns_bytes() {
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("conv-tool-output-zero-limit");
        seed_session(&sessions, &key, "u-alice").await;
        seed_result(&key, 1, "c-zero", json!("hello world")).await;

        let served = parse(
            call(
                &key,
                sessions,
                json!({ "tool_call_id": "c-zero", "limit": 0 }),
            )
            .await,
        );
        assert_eq!(served.text, "h", "a zero limit serves one character");
        assert_eq!(served.offset, 0);
        assert_eq!(served.total_bytes, 11);
        assert!(
            served.truncated,
            "the client is told to keep going — and now it can"
        );
    }
}
