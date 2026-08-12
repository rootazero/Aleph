//! Outbound harvest: settle a tool's `_media` output into the artifact store.
//!
//! # Why this lives at the dispatch chokepoint
//!
//! The `_media` convention (`{"_media": [MediaItem, …]}` in a tool result) used
//! to be harvested at exactly one site — the slash-command fast path. A
//! model-initiated `media_send` in a normal turn therefore reached nobody: no
//! Panel, no channel, no disk. [`ScopedToolService::apply_layer_two`] is the
//! universal tool chokepoint AND it is the last place that still sees the
//! *structured* value: a few lines later the whole result is flattened to a
//! `String` and truncated to the result-token budget, after which the items are
//! irrecoverable. Harvesting here covers every surface at once.
//!
//! [`ScopedToolService::apply_layer_two`]: super::ScopedToolService
//!
//! # Two lanes, one resolution
//!
//! `_media` has two destinations and they are independent:
//!
//! * the **artifact store**, which backs the workspace pane and outlives the run;
//! * the run's **channel-delivery buffer** ([`PendingMedia`]), which this run's
//!   `ReplyEmitter` drains and posts to Telegram / Slack / … as attachments.
//!
//! Only the first used to hang here. The second had exactly one writer — the
//! slash fast path, which holds the buffer in hand — so `/image …` reached a
//! channel while a model-initiated `media_send` in a normal turn did not: it
//! settled into the pane and stopped there, even though `media_send`'s own doc
//! comment promised channel delivery. Both lanes are now filled from this one
//! function, so the parse, the per-run cap and the delivery decision cannot
//! drift between the two entry points.
//!
//! The two lanes also used to resolve the item **separately**: this one fetched
//! it into a throwaway scratch directory for the store, and the emitter fetched
//! it *again* at `RunComplete` for the channel. Two downloads of the same URL,
//! each up to 50 MB and 60 s. Worse than the waste: the emitter's copy is where
//! a 404 / timeout / oversize was discovered, and the emitter runs **after the
//! loop has ended** — so the failure could not be reported to anyone. The
//! resolution now happens here, once, into the run's own media session
//! directory (the one `deliver_run_media` already cleans up after draining), and
//! what could not be resolved comes back as [`MediaFailure`] for the dispatch
//! chokepoint to put in front of the model while it can still act on it.
//!
//! # Contract
//!
//! * **Read-only on the tool value.** The items are cloned out; `out.value` is
//!   untouched. (The caller may *append* a delivery report to it — see
//!   `ScopedToolService::apply_layer_two` — but nothing here mutates it.)
//! * **Never fails the tool call.** A media item that could not be resolved is
//!   *reported*, not raised: the return type carries failures, it is not a
//!   `Result`, and storage failures are not even reported — the tool call
//!   itself succeeded and losing the durable copy must not undo that.
//!   Returning no error is not enough to keep that promise — see the deadline
//!   below, which is what stops the harvest from *indirectly* failing the call
//!   by outrunning the caller's own clock.
//! * **Delivery is not gated on storage.** A run with no `ArtifactStore`
//!   installed must still be able to send the user a picture.
//! * **The live event is a content-free ping.** `session.artifact` carries only
//!   the session key. The stream that carries it is deliberately lossy, so a
//!   consumer must treat [`ArtifactStore::list`] as the settlement and the ping
//!   as nothing more than "re-read now".
//!
//! # The deadline, and why it is not a number of its own
//!
//! Moving the resolution here bought visibility at the cost of spending the
//! fetch *inside* the tool call rather than after the loop — and the fetch is
//! up to [`MAX_MEDIA_PER_RUN`] items of 60 s each. That is not merely slow: the
//! chokepoint runs inside `ScopedToolService::execute_inner`'s
//! `tokio::time::timeout(budget, …)`, so a harvest that overruns turns the
//! whole call into a `ToolError::Timeout` — discarding every item that *did*
//! resolve (the lanes are filled only after the loop finishes) and, for a
//! generator, discarding a successfully generated and already-paid-for image
//! along with the URL the model could otherwise have handed the user. The
//! contract above says this harvest never fails the tool call; without a
//! deadline it could, just at one remove.
//!
//! So every entry point takes the instant **the caller's own clock fires** and
//! guarantees to return before it, reserving [`HARVEST_RETURN_MARGIN`] for the
//! work that follows it in `apply_layer_two`. Items the budget did not reach
//! are [`MediaFailure`]s on the same channel every other failure uses — and a
//! remote URL still degrades to a bare link, which is exactly the case where a
//! link is worth most (a slow origin is one Telegram may well fetch itself).
//!
//! There is deliberately **no `MEDIA_HARVEST_TIMEOUT` constant**. "How long may
//! this call take" is a question `crate::tools::budget` already owns for every
//! tool, and a second ceiling here would be a second answer free to drift from
//! it. An operator who wants media settled faster tightens `media_send`'s row
//! in `BUILTIN_TOOL_BUDGETS_MS` — same single source, and it bounds the tool's
//! own work at the same time. The margin below is not that second ceiling: it
//! is slack, not policy.
//!
//! What the deadline covers is the **resolution** — the network, which is the
//! unbounded part. The two lanes that follow it are local file I/O over bytes
//! already on disk; bounding those mid-way would strand half-written artifacts
//! to save a bounded cost.

use std::time::{Duration, Instant};

use serde_json::Value;
use tracing::{debug, warn};

use crate::artifacts::{ArtifactOrigin, ArtifactStore};
use crate::gateway::channel::Attachment;
// The ping's frame shape is owned by the gateway (one source, two producers —
// see that module for why the bus resolution is allowed to differ).
use crate::gateway::event_emitter::artifact_ping::publish_artifact_ping;
use crate::gateway::media::{is_remote_fetch_url, MediaItem, PendingMedia, MAX_MEDIA_PER_RUN};
use crate::media::cache::MediaCache;
use crate::tools::turn_context::TurnContext;

/// Tool-output field carrying the media convention.
const MEDIA_FIELD: &str = "_media";

/// Characters of a random id mixed into a derived filename.
const FILENAME_SUFFIX_LEN: usize = 8;

/// Slack reserved between the last fetch this harvest starts and the instant
/// the caller's own wall clock fires.
///
/// Not a media policy — see the module doc for why there is no such number
/// here. It covers what `apply_layer_two` still has to do after the harvest
/// returns (annotate, compress, hygiene, persist the result), all of which is
/// CPU-bound string work over an already-resolved value.
///
/// A caller whose budget is smaller than this margin gets a stop instant in the
/// past, and every item is reported as un-fetched without a socket being
/// opened. That is the honest answer: when the caller is about to be cut off,
/// starting a 60 s download is the one move guaranteed to lose everything.
const HARVEST_RETURN_MARGIN: Duration = Duration::from_secs(5);

/// One `_media` item that could not be turned into deliverable bytes.
///
/// Exists so the failure can reach the **model**, which is the only party able
/// to do anything about it — pick a different URL, re-encode the payload, point
/// at a file the capture tools actually wrote. It used to be a `warn!` and
/// nothing else, while the model read `"Sending 1 media file..."`.
#[derive(Debug, Clone)]
pub(crate) struct MediaFailure {
    /// The item's original `url` field, verbatim.
    pub url: String,
    /// Why it could not be resolved, as the resolver phrased it.
    pub reason: String,
    /// Whether a bare link was still handed to the channel. `false` means the
    /// user got nothing at all for this item.
    pub url_still_offered: bool,
}

/// Resolve and settle every `_media` item the tool declared, then ping the
/// session. Returns the items that could not be resolved.
///
/// `turn` is the dispatch's routing context; without one there is no session to
/// attribute the bytes to (direct calls, unit tests) and the harvest is skipped.
///
/// `caller_deadline` is the instant the caller's own wall clock fires — for the
/// chokepoint, the `tokio::time::timeout` this call is already running inside.
/// It is passed rather than derived here on purpose: derived at this depth it
/// would read `Instant::now() + budget` and so believe it had the *whole*
/// budget, when a slow generator may have spent nearly all of it upstream —
/// leaving the overrun this argument exists to prevent exactly where it hurts
/// most.
pub(super) async fn harvest_outbound_media(
    tool: &str,
    value: &Value,
    turn: Option<&TurnContext>,
    caller_deadline: Instant,
) -> Vec<MediaFailure> {
    let Some(turn) = turn else {
        // Cheap pre-check so a context-less dispatch does not pay for parsing.
        if !media_items(value).is_empty() {
            debug!(
                tool,
                "tool declared _media but the dispatch carries no turn context; not settled"
            );
        }
        return Vec::new();
    };
    // The delivery buffer is published run-tree-wide at the run-loop boundary
    // (`crate::gateway::media::with_pending_media`), so the chokepoint can reach
    // it without it being threaded through `build_request_tool_service`.
    let delivery = crate::gateway::media::current_pending_media();
    let run_id = (!turn.run_id.is_empty()).then_some(turn.run_id.as_str());
    harvest_media_for_session(
        tool,
        value,
        &turn.session_key.to_key_string(),
        run_id,
        delivery.as_ref(),
        caller_deadline,
    )
    .await
}

/// Harvest for a caller that knows its session directly.
///
/// The slash-command fast path (`execution_engine::slash_command`) invokes the
/// registry itself and never passes through [`super::ScopedToolService`], so the
/// chokepoint above never fires for it — it passes its own `RunRequest`'s buffer
/// as `delivery` rather than reading the task-local, which its dispatch never
/// enters. Sharing the body rather than copying it is the point: two harvests
/// would be two chances to drift on filenames, caps, the ping and — the bug this
/// argument exists to prevent recurring — on which lanes get filled at all.
pub(crate) async fn harvest_media_for_session(
    tool: &str,
    value: &Value,
    session_key: &str,
    run_id: Option<&str>,
    delivery: Option<&PendingMedia>,
    caller_deadline: Instant,
) -> Vec<MediaFailure> {
    let items = media_items(value);
    if items.is_empty() {
        return Vec::new();
    }

    let store = ArtifactStore::shared();
    if delivery.is_none() && store.is_none() {
        // Neither lane exists. Resolving here would be a download nobody reads
        // — and, for a remote URL, a real fetch on the tool-dispatch path.
        return Vec::new();
    }

    if items.len() > MAX_MEDIA_PER_RUN {
        warn!(
            tool,
            declared = items.len(),
            kept = MAX_MEDIA_PER_RUN,
            "tool declared more _media items than the per-run cap; the rest are dropped"
        );
    }

    let (media_session, ours_to_clean) = media_session_for(run_id, delivery.is_some());

    let resolved = resolve_media_items(tool, items, &media_session, stop_by(caller_deadline)).await;

    // Channel lane first: it must not depend on an `ArtifactStore` being
    // installed, nor on the store call succeeding.
    if let Some(buffer) = delivery {
        queue_for_delivery(tool, buffer, resolved.deliver).await;
    }
    if let Some(store) = store {
        if store_resolved_media(store, session_key, run_id, tool, &resolved.store).await > 0 {
            publish_artifact_ping(session_key);
        }
    }

    if ours_to_clean {
        if let Err(e) = MediaCache::cleanup_session(&media_session) {
            debug!(error = %e, "failed to clean the media-harvest scratch dir");
        }
    }

    resolved.failures
}

/// Which media-cache session the resolved bytes go into, and whether this call
/// is the one that deletes it.
///
/// The files have to outlive this call for exactly one reason: the delivery
/// buffer holds paths into them and the emitter reads those at run end. So
/// *that* — not the presence of a run id — is what decides ownership.
///
/// * **Handing off** (a delivery buffer exists, and there is a run to hang the
///   directory off): the run's own media session directory, which is the one
///   `deliver_run_media` cleans up, and it does so *after* draining. Not ours.
/// * **Otherwise** (Panel / cron / A2A — an artifact-store-only run): a
///   throwaway directory deleted before this function returns, exactly as the
///   store lane always did. Writing into the run directory here would strand
///   the files until `cleanup_stale` (1 h), because no emitter exists to clean
///   it, and would risk deleting a directory something else is sharing.
///
/// The alphanumeric uuid also avoids the illegal-character problem a raw
/// session key causes on Windows.
fn media_session_for(run_id: Option<&str>, hands_off_to_emitter: bool) -> (String, bool) {
    match (hands_off_to_emitter, run_id) {
        (true, Some(run)) => (run.to_string(), false),
        // A delivery buffer with no run id: nothing here can clean up after
        // the emitter, so the sweep is left to `cleanup_stale`.
        (true, None) => (scratch_media_session(), false),
        (false, _) => (scratch_media_session(), true),
    }
}

fn scratch_media_session() -> String {
    format!("media-harvest-{}", uuid::Uuid::new_v4().simple())
}

/// What one call's worth of `_media` resolved to.
struct ResolvedMedia {
    /// What the channel should receive, in declaration order.
    deliver: Vec<Attachment>,
    /// Resolved items, each paired with the declaration the store reads for
    /// naming (`Attachment` carries no `media_type`).
    store: Vec<(MediaItem, Attachment)>,
    /// What could not be resolved at all.
    failures: Vec<MediaFailure>,
}

/// The instant the harvest must have stopped fetching by, given when the
/// caller's own clock fires.
///
/// `checked_sub` rather than `-`: a caller whose budget is shorter than the
/// margin yields a stop instant in the past, which the loop reads as "no time
/// at all" — the intended answer, and one an `Instant` subtraction would panic
/// on instead.
fn stop_by(caller_deadline: Instant) -> Instant {
    caller_deadline
        .checked_sub(HARVEST_RETURN_MARGIN)
        .unwrap_or(caller_deadline)
}

/// What the model is told about an item the budget never reached.
///
/// Phrased as a move it can make. The two things that actually help are fewer
/// items per call (the budget is per call, not per item) and a smaller source;
/// "try again" is not on the list, because a retry costs the same budget.
fn deadline_reason() -> String {
    "not fetched: this tool call's wall-clock budget ran out before this item's turn. \
     The budget covers the whole call, so declare fewer media items per call, or point \
     at a smaller/faster source."
        .to_string()
}

/// The single resolution both lanes are built from — one fetch per item, into
/// `media_session`, and never past `stop_by`.
///
/// Sequential rather than `join_all`: this now runs on the tool-dispatch path,
/// where up to [`MAX_MEDIA_PER_RUN`] concurrent 50 MB downloads would be a
/// worse neighbour than a slightly slower tool call. Sequential is also what
/// makes the budget spendable in declaration order — the early items get their
/// full 60 s and land, rather than ten parallel fetches all being cut off
/// together with nothing to show.
async fn resolve_media_items(
    tool: &str,
    items: Vec<MediaItem>,
    media_session: &str,
    stop_by: Instant,
) -> ResolvedMedia {
    let cache = MediaCache::new();
    let mut out = ResolvedMedia {
        deliver: Vec::new(),
        store: Vec::new(),
        failures: Vec::new(),
    };

    for item in items.into_iter().take(MAX_MEDIA_PER_RUN) {
        // Re-read the clock per item: the budget is shared across the call, so
        // what the previous fetch spent is what this one no longer has.
        let remaining = stop_by.saturating_duration_since(Instant::now());
        let outcome = if remaining.is_zero() {
            Err(deadline_reason())
        } else {
            match tokio::time::timeout(remaining, cache.download_media_item(&item, media_session))
                .await
            {
                Ok(Ok(attachment)) => Ok(attachment),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_elapsed) => Err(deadline_reason()),
            }
        };

        match outcome {
            Ok(attachment) => {
                out.deliver.push(attachment.clone());
                out.store.push((item, attachment));
            }
            Err(reason) => {
                // A remote URL we could not fetch may still be one the channel
                // can: Telegram uploads from a URL itself, and a file over our
                // own 50 MB cap is a perfectly good link. The other two
                // branches have nothing to offer — a `data:` URL that did not
                // decode has no link, and a refused local path would go out as
                // an attachment whose `url` is a filesystem path, which no
                // channel can send.
                //
                // This is also where running out of budget pays for itself: the
                // items the clock cut off are overwhelmingly the remote ones,
                // and a link is worth most precisely when the origin was too
                // slow for us.
                let offer_url = is_remote_fetch_url(&item.url);
                if offer_url {
                    out.deliver.push(MediaCache::url_only_attachment(&item));
                }
                warn!(
                    tool,
                    url = %item.url,
                    error = %reason,
                    url_still_offered = offer_url,
                    "media item could not be resolved"
                );
                out.failures.push(MediaFailure {
                    url: item.url,
                    reason,
                    url_still_offered: offer_url,
                });
            }
        }
    }

    out
}

/// Append the resolved attachments to the run's channel-delivery buffer,
/// honouring the per-run cap across every tool call in the run (the buffer is
/// shared, so the cap is on what is still unsent, not on what one call
/// declared).
async fn queue_for_delivery(tool: &str, buffer: &PendingMedia, attachments: Vec<Attachment>) {
    if attachments.is_empty() {
        return;
    }
    let declared = attachments.len();
    let mut pending = buffer.lock().await;
    let remaining = MAX_MEDIA_PER_RUN.saturating_sub(pending.len());
    if remaining == 0 {
        warn!(
            tool,
            declared,
            "the run's media delivery queue is full; these items are not sent to the channel"
        );
        return;
    }
    let taken = declared.min(remaining);
    pending.extend(attachments.into_iter().take(taken));
    debug!(
        tool,
        queued = taken,
        dropped = declared - taken,
        "queued tool media for channel delivery"
    );
}

/// Tool-output field carrying the delivery report back to the model.
const DELIVERY_FIELD: &str = "_media_delivery";

/// Write `failures` into the tool's result value so the model reads them on its
/// next turn.
///
/// No-op when there is nothing to report — the overwhelmingly common case — so
/// a successful `media_send` produces the exact bytes it always did. Also a
/// no-op when the result is not a JSON object: `_media` can only have come off
/// an object in the first place, so there is nowhere to put this and nothing
/// that could have failed.
pub(super) fn annotate_media_failures(value: &mut Value, failures: &[MediaFailure]) {
    if failures.is_empty() {
        return;
    }
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    let reported: Vec<Value> = failures
        .iter()
        .map(|f| {
            serde_json::json!({
                "url": f.url,
                "error": f.reason,
                // Spelled out rather than left for the model to infer: "we sent
                // a bare link instead" and "the user got nothing" call for
                // different next moves.
                "delivered_as_link_instead": f.url_still_offered,
            })
        })
        .collect();
    obj.insert(
        DELIVERY_FIELD.to_string(),
        serde_json::json!({
            "failed": reported,
            "note": "These media items were NOT delivered as files. Delivery happens after \
                     this turn ends, so this is the only point at which you can correct it.",
        }),
    );
}

/// Parse the `_media` field. Absent or malformed yields no items — a tool that
/// mis-shapes its own convention must not break its own call.
fn media_items(value: &Value) -> Vec<MediaItem> {
    let Some(raw) = value.get(MEDIA_FIELD) else {
        return Vec::new();
    };
    match serde_json::from_value::<Vec<MediaItem>>(raw.clone()) {
        Ok(items) => items,
        Err(e) => {
            debug!(error = %e, "tool output carried a malformed `_media` field; nothing harvested");
            Vec::new()
        }
    }
}

/// Store the already-resolved items; returns how many landed in the store.
///
/// Runs inline rather than detached: the ping must not outlive the run that
/// produced it, and a detached task would have to be reasoned about against run
/// teardown.
///
/// Reads the bytes back off the file the single resolution produced. That file
/// came through the media cache — the existing pipeline that decodes `data:`
/// URLs, refuses model-supplied local paths outside the temp root, and fetches
/// remote URLs through `security::ssrf::safe_fetch` — which is what keeps this
/// from becoming a second, unguarded fetch path.
async fn store_resolved_media(
    store: &ArtifactStore,
    session_key: &str,
    run_id: Option<&str>,
    tool: &str,
    resolved: &[(MediaItem, Attachment)],
) -> usize {
    let mut stored = 0usize;

    for (item, attachment) in resolved {
        let Some(path) = attachment.path.as_deref() else {
            continue;
        };
        let bytes = match tokio::fs::read(path).await {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(error = %e, "resolved media file could not be read; not stored as an artifact");
                continue;
            }
        };
        let filename = filename_for(item, &attachment.mime_type);
        match store
            .put(
                session_key,
                run_id,
                ArtifactOrigin::Outbound,
                &filename,
                &attachment.mime_type,
                &bytes,
            )
            .await
        {
            Ok(record) => {
                debug!(
                    tool,
                    artifact_id = %record.id,
                    filename = %record.filename,
                    size = record.size,
                    "settled tool media as an outbound artifact"
                );
                stored += 1;
            }
            // Best-effort by contract: the tool already succeeded, and losing
            // the durable copy must not turn that into a model-visible error.
            // Note this is *not* reported as a `MediaFailure` — the user still
            // gets the media, only the workspace-pane copy is missing.
            Err(e) => warn!(tool, error = %e, "failed to store tool media as an artifact"),
        }
    }

    stored
}

/// Display name for the artifact: the item's own name when it has one, else the
/// URL's last segment when that looks like a filename, else a synthetic name
/// carrying the media type and a short random suffix (so a session's generated
/// images do not all list as the same name). The store sanitizes whatever this
/// returns.
fn filename_for(item: &MediaItem, mime_type: &str) -> String {
    if let Some(name) = item.filename.as_deref().map(str::trim) {
        if !name.is_empty() {
            return name.to_string();
        }
    }
    if !item.url.to_ascii_lowercase().starts_with("data:") {
        let path = item.url.split(['?', '#']).next().unwrap_or(&item.url);
        if let Some(segment) = path.rsplit('/').next() {
            let segment = segment.trim();
            if segment.contains('.') && !segment.starts_with('.') {
                return segment.to_string();
            }
        }
    }
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let suffix = suffix.get(..FILENAME_SUFFIX_LEN).unwrap_or(suffix.as_str());
    let stem = if item.media_type.is_empty() {
        "media"
    } else {
        item.media_type.as_str()
    };
    format!("{stem}-{suffix}.{}", extension_for(mime_type))
}

/// Extension for a synthetic filename. Only the shapes the `_media` convention
/// actually carries; anything else is `bin`.
fn extension_for(mime_type: &str) -> &'static str {
    match mime_type.split(';').next().unwrap_or("").trim() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "audio/mpeg" => "mp3",
        "audio/wav" => "wav",
        "audio/ogg" => "ogg",
        "text/plain" => "txt",
        "application/pdf" => "pdf",
        _ => "bin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `data:` URL decoding to the five bytes `Hello`.
    const HELLO_PNG: &str = "data:image/png;base64,SGVsbG8=";

    const SESSION: &str = "agent:main:main";

    fn media_value(items: Value) -> Value {
        json!({ "_display": "Sending media...", MEDIA_FIELD: items })
    }

    fn item(url: &str, filename: Option<&str>) -> Value {
        match filename {
            Some(name) => json!({ "url": url, "media_type": "image", "filename": name }),
            None => json!({ "url": url, "media_type": "image" }),
        }
    }

    fn buffer() -> PendingMedia {
        PendingMedia::default()
    }

    /// A caller deadline far enough out that it never fires — the shape of a
    /// tool with its full budget left. Tests that are *about* the deadline
    /// build their own.
    fn ample() -> Instant {
        Instant::now() + Duration::from_secs(300)
    }

    /// Run the real resolution, then the store lane — the two halves
    /// `harvest_media_for_session` runs back to back. Tests own a local
    /// `ArtifactStore` while production reads a global one, so they cannot go
    /// through the front door; they go through the *same resolver* instead of
    /// a copy of its loop, which is the part that would drift.
    async fn resolve_then_store(
        store: &ArtifactStore,
        value: &Value,
        run_id: Option<&str>,
        tool: &str,
    ) -> usize {
        let session = format!("harvest-test-{}", uuid::Uuid::new_v4().simple());
        let resolved = resolve_media_items(tool, media_items(value), &session, ample()).await;
        let stored = store_resolved_media(store, SESSION, run_id, tool, &resolved.store).await;
        let _ = MediaCache::cleanup_session(&session);
        stored
    }

    fn turn() -> TurnContext {
        TurnContext {
            session_key: crate::routing::session_key::SessionKey::main("main"),
            run_id: "run-1".to_string(),
            channel_id: "telegram".to_string(),
            conversation_id: "chat-1".to_string(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
        }
    }

    // ── Channel-delivery lane ───────────────────────────────────────────
    //
    // The regression these guard: the harvest used to fill only the artifact
    // store, so a model-initiated `media_send` reached the workspace pane and
    // never the channel. Only `/image …` — which pushed the buffer itself —
    // got through.

    #[tokio::test]
    async fn the_dispatch_chokepoint_queues_media_for_the_runs_channel() {
        let queue = buffer();
        let value = media_value(json!([item(HELLO_PNG, Some("cat.png"))]));

        // Exactly how a normal turn runs: the run loop publishes the buffer,
        // `apply_layer_two` calls the harvest many frames below.
        crate::gateway::media::with_pending_media(Some(queue.clone()), async {
            harvest_outbound_media("media_send", &value, Some(&turn()), ample()).await;
        })
        .await;

        let queued = queue.lock().await;
        assert_eq!(queued.len(), 1, "the model's media must reach the channel");
        assert_eq!(queued[0].filename.as_deref(), Some("cat.png"));
    }

    #[tokio::test]
    async fn a_run_with_no_channel_leg_still_harvests() {
        // Panel / cron runs publish no buffer. The harvest must not panic on the
        // absent task-local, and must still take the artifact-store lane.
        let value = media_value(json!([item(HELLO_PNG, None)]));
        assert!(crate::gateway::media::current_pending_media().is_none());
        harvest_outbound_media("media_send", &value, Some(&turn()), ample()).await;
    }

    #[tokio::test]
    async fn delivery_is_not_gated_on_the_artifact_store() {
        // `ArtifactStore::shared()` is absent in unit tests — the exact shape of
        // a deployment with no data dir. The user must still get the picture.
        let queue = buffer();
        let value = media_value(json!([item(HELLO_PNG, Some("cat.png"))]));

        harvest_media_for_session("media_send", &value, SESSION, None, Some(&queue), ample()).await;

        assert_eq!(queue.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn the_delivery_queue_cap_counts_the_whole_run_not_one_call() {
        let queue = buffer();
        // Two calls in one run, each declaring the full cap.
        let value = media_value(Value::Array(
            (0..MAX_MEDIA_PER_RUN)
                .map(|_| item(HELLO_PNG, None))
                .collect(),
        ));

        harvest_media_for_session(
            "image_generate",
            &value,
            SESSION,
            None,
            Some(&queue),
            ample(),
        )
        .await;
        harvest_media_for_session(
            "image_generate",
            &value,
            SESSION,
            None,
            Some(&queue),
            ample(),
        )
        .await;

        assert_eq!(
            queue.lock().await.len(),
            MAX_MEDIA_PER_RUN,
            "the cap is on the shared buffer, not on one tool call"
        );
    }

    #[tokio::test]
    async fn a_tool_without_media_leaves_the_queue_alone() {
        let queue = buffer();
        let value = json!({ "_display": "ok", "stdout": "hello" });
        harvest_media_for_session("bash", &value, SESSION, None, Some(&queue), ample()).await;
        assert!(queue.lock().await.is_empty());
    }

    #[tokio::test]
    async fn tool_media_is_settled_as_an_outbound_artifact() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ArtifactStore::new(dir.path().to_path_buf());
        let value = media_value(json!([item(HELLO_PNG, Some("cat.png"))]));

        let stored = resolve_then_store(&store, &value, Some("run-1"), "media_send").await;
        assert_eq!(stored, 1);

        let listed = store.list(SESSION).await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].filename, "cat.png");
        assert_eq!(listed[0].mime_type, "image/png");
        assert_eq!(listed[0].origin, ArtifactOrigin::Outbound);
        assert_eq!(listed[0].run_id.as_deref(), Some("run-1"));

        let (_record, bytes) = store.read(SESSION, &listed[0].id).await.expect("read");
        assert_eq!(bytes, b"Hello", "the artifact holds the decoded bytes");
    }

    #[tokio::test]
    async fn a_tool_output_without_media_is_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ArtifactStore::new(dir.path().to_path_buf());
        let value = json!({ "_display": "ok", "stdout": "hello", "exit_code": 0 });

        assert!(media_items(&value).is_empty());
        let stored = resolve_then_store(&store, &value, None, "bash").await;
        assert_eq!(stored, 0);
        assert!(store.list(SESSION).await.expect("list").is_empty());
    }

    // ── Failure reporting ───────────────────────────────────────────────
    //
    // The regression these guard: an item that could not be fetched used to
    // degrade to a bare link with the reason left in a `warn!`, while the model
    // read "Sending 1 media file...". Delivery happens at `RunComplete`, after
    // the loop has ended, so the dispatch chokepoint is the last place the model
    // can be told anything at all.

    #[tokio::test]
    async fn a_refused_local_path_is_reported_and_not_offered_as_a_link() {
        let queue = buffer();
        // Outside the media root. A url-only attachment here would carry a
        // filesystem path as its `url` — nothing a channel can send.
        let value = media_value(json!([item("/etc/hosts", Some("hosts"))]));

        let failures =
            harvest_media_for_session("media_send", &value, SESSION, None, Some(&queue), ample())
                .await;

        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].url, "/etc/hosts");
        assert!(
            !failures[0].url_still_offered,
            "a filesystem path must not go out as a link"
        );
        assert!(
            queue.lock().await.is_empty(),
            "nothing deliverable, so nothing queued"
        );
    }

    #[tokio::test]
    async fn an_undecodable_data_url_is_reported_and_not_offered_as_a_link() {
        let queue = buffer();
        let value = media_value(json!([item("data:image/png;base64,!!!bad!!!", None)]));

        let failures =
            harvest_media_for_session("media_send", &value, SESSION, None, Some(&queue), ample())
                .await;

        assert_eq!(failures.len(), 1);
        assert!(
            !failures[0].url_still_offered,
            "a data: URL that did not decode has no link to hand on"
        );
        assert!(queue.lock().await.is_empty());
    }

    #[tokio::test]
    async fn a_resolvable_item_reports_nothing() {
        let queue = buffer();
        let value = media_value(json!([item(HELLO_PNG, Some("cat.png"))]));

        let failures =
            harvest_media_for_session("media_send", &value, SESSION, None, Some(&queue), ample())
                .await;

        assert!(failures.is_empty(), "the success path reports nothing");
        assert_eq!(queue.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn the_queued_attachment_is_already_resolved() {
        // THE wire: the emitter no longer fetches, so whatever lands in the
        // buffer must already point at a readable local file. If this ever
        // regresses to queueing the raw item, the user gets nothing and no test
        // downstream would notice — the emitter just forwards what it is given.
        let queue = buffer();
        let value = media_value(json!([item(HELLO_PNG, Some("cat.png"))]));

        harvest_media_for_session(
            "media_send",
            &value,
            SESSION,
            Some("run-resolved"),
            Some(&queue),
            ample(),
        )
        .await;

        let queued = queue.lock().await;
        let path = queued[0]
            .path
            .as_deref()
            .expect("a queued attachment must carry a local path");
        assert_eq!(
            tokio::fs::read(path).await.expect("the file must exist"),
            b"Hello",
            "the bytes the channel will upload"
        );
        drop(queued);
        let _ = MediaCache::cleanup_session("run-resolved");
    }

    #[test]
    fn only_a_hand_off_to_the_emitter_borrows_the_runs_media_dir() {
        // Writing into the run's own directory is what lets the emitter send
        // the files without re-fetching — but only the emitter cleans that
        // directory. A store-only run (Panel / cron) has no emitter, so
        // borrowing it there would strand the files for an hour; it gets a
        // throwaway directory it deletes itself, exactly as the store lane
        // always did.
        let (dir, ours) = media_session_for(Some("run-1"), true);
        assert_eq!(dir, "run-1", "the emitter's directory, so it can drain it");
        assert!(!ours, "deleting it would delete what was just queued");

        let (dir, ours) = media_session_for(Some("run-1"), false);
        assert_ne!(dir, "run-1", "no emitter will ever clean the run's dir");
        assert!(ours, "a store-only run must clean up after itself");

        let (_, ours) = media_session_for(None, false);
        assert!(ours);

        // A delivery buffer with no run to hang the directory off: still not
        // ours — the buffer holds paths into it — so `cleanup_stale` sweeps it.
        let (_, ours) = media_session_for(None, true);
        assert!(!ours);
    }

    // ── The deadline ────────────────────────────────────────────────────
    //
    // The regression these guard: the resolution moved onto the tool-dispatch
    // path, where it runs inside the call's own `tokio::time::timeout`. Ten
    // items at 60 s outruns any budget the repo ships, and the overrun does not
    // merely delay — it converts the whole call into `ToolError::Timeout`,
    // taking the items that DID resolve and the generator's own successful
    // result down with it.

    #[test]
    fn the_harvest_always_stops_before_the_caller_does() {
        // THE invariant. If this ever inverts, the harvest becomes the thing
        // that trips the clock it was handed to respect.
        let caller = Instant::now() + Duration::from_secs(300);
        assert!(
            stop_by(caller) < caller,
            "the harvest must give the caller room to finish"
        );

        // A caller with less budget than the margin: the stop instant lands in
        // the past rather than panicking on the `Instant` subtraction, and the
        // loop reads that as no time at all.
        let tight = Instant::now() + Duration::from_millis(1);
        assert!(stop_by(tight) <= Instant::now());
    }

    #[tokio::test]
    async fn an_expired_budget_reports_every_item_instead_of_dropping_it() {
        let queue = buffer();
        // A remote URL, so the degrade-to-link branch is in play too.
        let value = media_value(json!([item("https://example.com/big.mp4", None)]));

        let failures = harvest_media_for_session(
            "media_send",
            &value,
            SESSION,
            None,
            Some(&queue),
            // Already fired: the caller is out of time before we start.
            Instant::now(),
        )
        .await;

        assert_eq!(failures.len(), 1, "silence is the one unacceptable answer");
        assert!(
            failures[0].reason.contains("budget"),
            "the model has to be able to tell this from a 404; got: {}",
            failures[0].reason
        );
        assert!(
            failures[0].url_still_offered,
            "an origin too slow for us is exactly when a bare link is worth most"
        );
        assert_eq!(
            queue.lock().await.len(),
            1,
            "and that link is actually queued"
        );
    }

    #[tokio::test]
    async fn an_expired_budget_opens_no_socket() {
        // Not just "reports the failure" — it must report it *without* spending
        // the time. An unroutable host would sit in connect() for seconds.
        let value = media_value(json!([
            item("https://10.255.255.1/a.png", None),
            item("https://10.255.255.1/b.png", None),
        ]));

        // A delivery buffer, not `None`: with neither lane present the harvest
        // returns before it ever reaches the resolver, and whether the *store*
        // lane exists depends on `ArtifactStore::shared()` — a `OnceLock` over
        // the real data dir that a unit test cannot inject. Passing the buffer
        // is what keeps this measuring the deadline rather than the machine.
        let queue = buffer();
        let started = Instant::now();
        let failures = harvest_media_for_session(
            "media_send",
            &value,
            SESSION,
            None,
            Some(&queue),
            Instant::now(),
        )
        .await;

        assert_eq!(failures.len(), 2);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "an expired budget must short-circuit, not fetch; took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn the_budget_is_spent_in_declaration_order_not_forfeited_wholesale() {
        // The point of resolving sequentially against a shared deadline: what
        // fits still lands. Under the old unbounded loop the whole call was
        // killed by the outer timeout and even the resolved items were lost.
        let queue = buffer();
        let value = media_value(json!([
            item(HELLO_PNG, Some("first.png")),
            item(HELLO_PNG, Some("second.png")),
        ]));

        let failures =
            harvest_media_for_session("media_send", &value, SESSION, None, Some(&queue), ample())
                .await;

        assert!(failures.is_empty());
        let queued = queue.lock().await;
        assert_eq!(queued.len(), 2);
        assert_eq!(queued[0].filename.as_deref(), Some("first.png"));
    }

    #[test]
    fn a_clean_run_leaves_the_tool_value_byte_identical() {
        let mut value = json!({ "_display": "Sending 1 media file...", MEDIA_FIELD: [] });
        // rust-doctor-disable-next-line excessive-clone
        let before = value.clone();
        annotate_media_failures(&mut value, &[]);
        assert_eq!(value, before, "no failures must write nothing at all");
    }

    #[test]
    fn a_failure_is_written_where_the_model_will_read_it() {
        let mut value = json!({ "_display": "Sending 1 media file..." });
        annotate_media_failures(
            &mut value,
            &[MediaFailure {
                url: "https://example.com/gone.png".to_string(),
                reason: "HTTP 404 Not Found".to_string(),
                url_still_offered: true,
            }],
        );
        let report = value
            .get(DELIVERY_FIELD)
            .expect("the report rides on the tool result the model reads");
        let failed = report["failed"].as_array().expect("failed[]");
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0]["url"], "https://example.com/gone.png");
        assert_eq!(
            failed[0]["error"], "HTTP 404 Not Found",
            "the reason has to survive — it is what the model corrects against"
        );
        assert_eq!(failed[0]["delivered_as_link_instead"], true);
    }

    #[test]
    fn a_non_object_result_is_left_alone() {
        // `_media` can only have come off an object, so there is nothing to
        // report on and nowhere to put it.
        let mut value = json!("plain string result");
        annotate_media_failures(
            &mut value,
            &[MediaFailure {
                url: "x".to_string(),
                reason: "y".to_string(),
                url_still_offered: false,
            }],
        );
        assert_eq!(value, json!("plain string result"));
    }

    #[test]
    fn a_malformed_media_field_yields_no_items() {
        // A tool that mis-shapes its own convention must not break its own call.
        assert!(media_items(&json!({ MEDIA_FIELD: "not-an-array" })).is_empty());
        assert!(media_items(&json!({ MEDIA_FIELD: [{ "no_url": true }] })).is_empty());
    }

    #[tokio::test]
    async fn a_store_failure_never_fails_the_tool_call() {
        // Root is a FILE, so every `create_dir_all` under it fails.
        let dir = tempfile::tempdir().expect("tempdir");
        let blocked = dir.path().join("root-is-a-file");
        tokio::fs::write(&blocked, b"x").await.expect("write");
        let store = ArtifactStore::new(blocked);
        let value = media_value(json!([item(HELLO_PNG, Some("cat.png"))]));

        // Returns normally with nothing stored — the error is swallowed, and
        // the caller (`apply_layer_two`) has no error to propagate by construction.
        let stored = resolve_then_store(&store, &value, None, "media_send").await;
        assert_eq!(stored, 0);
    }

    #[tokio::test]
    async fn an_unresolvable_item_does_not_stop_the_rest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ArtifactStore::new(dir.path().to_path_buf());
        // `/etc/hosts` is outside the media cache's allowed local root, so it
        // degrades to a URL-only attachment with no readable bytes.
        let value = media_value(json!([
            item("/etc/hosts", Some("hosts")),
            item(HELLO_PNG, Some("cat.png")),
        ]));

        let stored = resolve_then_store(&store, &value, None, "media_send").await;
        assert_eq!(stored, 1, "the resolvable item is still settled");
        let listed = store.list(SESSION).await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].filename, "cat.png");

        // …and the one that did not resolve is reported rather than dropped.
        let session = "harvest-partial-failure";
        let resolved =
            resolve_media_items("media_send", media_items(&value), session, ample()).await;
        assert_eq!(resolved.failures.len(), 1);
        assert_eq!(resolved.failures[0].url, "/etc/hosts");
        let _ = MediaCache::cleanup_session(session);
    }

    #[tokio::test]
    async fn the_per_run_media_cap_is_honoured() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ArtifactStore::new(dir.path().to_path_buf());
        let items: Vec<Value> = (0..MAX_MEDIA_PER_RUN + 3)
            .map(|_| item(HELLO_PNG, Some("cat.png")))
            .collect();
        let value = media_value(Value::Array(items));

        let stored = resolve_then_store(&store, &value, None, "media_send").await;
        assert_eq!(stored, MAX_MEDIA_PER_RUN);
        let listed = store.list(SESSION).await.expect("list");
        assert_eq!(listed.len(), MAX_MEDIA_PER_RUN);
    }

    #[test]
    fn filename_prefers_the_declared_name_then_the_url_segment() {
        let declared = MediaItem {
            url: "https://example.com/pic/photo.png".into(),
            media_type: "image".into(),
            mime_type: None,
            filename: Some("chart.png".into()),
        };
        assert_eq!(filename_for(&declared, "image/png"), "chart.png");

        let from_url = MediaItem {
            filename: None,
            ..declared.clone()
        };
        assert_eq!(filename_for(&from_url, "image/png"), "photo.png");

        let query_stripped = MediaItem {
            url: "https://example.com/pic/photo.png?token=abc".into(),
            ..from_url.clone()
        };
        assert_eq!(filename_for(&query_stripped, "image/png"), "photo.png");
    }

    #[test]
    fn a_data_url_without_a_name_gets_a_synthetic_one() {
        let item = MediaItem {
            url: HELLO_PNG.into(),
            media_type: "image".into(),
            mime_type: None,
            filename: None,
        };
        let name = filename_for(&item, "image/png");
        assert!(name.starts_with("image-"), "got {name}");
        assert!(name.ends_with(".png"), "got {name}");
        assert_ne!(
            name,
            filename_for(&item, "image/png"),
            "synthetic names must not collide across items"
        );
    }
}
