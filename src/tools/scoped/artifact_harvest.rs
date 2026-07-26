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
//! # Contract
//!
//! * **Read-only on the tool value.** The items stay in `out.value`, so the
//!   existing delivery path (`ReplyEmitter::drain_and_send_media`) is unchanged.
//! * **Never fails the tool call.** Every failure — unresolvable URL, full disk,
//!   missing data dir — is logged and the harvest moves on. The signature
//!   returns `()`, so no error can escape into the model's result.
//! * **The live event is a content-free ping.** `session.artifact` carries only
//!   the session key. The stream that carries it is deliberately lossy, so a
//!   consumer must treat [`ArtifactStore::list`] as the settlement and the ping
//!   as nothing more than "re-read now".

use serde_json::Value;
use tracing::{debug, warn};

use crate::artifacts::{ArtifactOrigin, ArtifactStore};
// The ping's frame shape is owned by the gateway (one source, two producers —
// see that module for why the bus resolution is allowed to differ).
use crate::gateway::event_emitter::artifact_ping::publish_artifact_ping;
use crate::gateway::media::{MediaItem, MAX_MEDIA_PER_RUN};
use crate::media::cache::MediaCache;
use crate::tools::turn_context::TurnContext;

/// Tool-output field carrying the media convention.
const MEDIA_FIELD: &str = "_media";

/// Characters of a random id mixed into a derived filename.
const FILENAME_SUFFIX_LEN: usize = 8;

/// Store every `_media` item the tool declared, then ping the session.
///
/// `turn` is the dispatch's routing context; without one there is no session to
/// attribute the bytes to (direct calls, unit tests) and the harvest is skipped.
pub(super) async fn harvest_outbound_media(tool: &str, value: &Value, turn: Option<&TurnContext>) {
    let Some(turn) = turn else {
        // Cheap pre-check so a context-less dispatch does not pay for parsing.
        if !media_items(value).is_empty() {
            debug!(
                tool,
                "tool declared _media but the dispatch carries no turn context; not settled"
            );
        }
        return;
    };
    let run_id = (!turn.run_id.is_empty()).then_some(turn.run_id.as_str());
    harvest_media_for_session(tool, value, &turn.session_key.to_key_string(), run_id).await;
}

/// Harvest for a caller that knows its session directly.
///
/// The slash-command fast path (`execution_engine::slash_command`) invokes the
/// registry itself and never passes through [`super::ScopedToolService`], so the
/// chokepoint above never fires for it. Before this existed, `/image …` was the
/// *only* invocation whose media reached a channel — and the only one whose
/// media never reached the pane. Sharing the body rather than copying it is the
/// point: two harvests would be two chances to drift on filenames, caps and the
/// ping.
pub(crate) async fn harvest_media_for_session(
    tool: &str,
    value: &Value,
    session_key: &str,
    run_id: Option<&str>,
) {
    let items = media_items(value);
    if items.is_empty() {
        return;
    }
    let Some(store) = ArtifactStore::shared() else {
        return;
    };
    if store_media_items(store, session_key, run_id, tool, items).await > 0 {
        publish_artifact_ping(session_key);
    }
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

/// Resolve and store the items; returns how many landed in the store.
///
/// Runs inline rather than detached: the ping must not outlive the run that
/// produced it, and a detached task would have to be reasoned about against run
/// teardown. The cost is bounded by the media cache's own limits (50 MB, 60 s
/// per fetch) and by [`MAX_MEDIA_PER_RUN`].
async fn store_media_items(
    store: &ArtifactStore,
    session_key: &str,
    run_id: Option<&str>,
    tool: &str,
    items: Vec<MediaItem>,
) -> usize {
    if items.len() > MAX_MEDIA_PER_RUN {
        warn!(
            tool,
            declared = items.len(),
            kept = MAX_MEDIA_PER_RUN,
            "tool declared more _media items than the per-run cap; the rest are dropped"
        );
    }

    // One throwaway media-cache session per harvest. A fresh uuid keeps the
    // scratch directory distinct from the delivery path's (so neither can
    // clobber the other's temp files) and, being alphanumeric, it can never
    // reproduce the illegal-character bug a raw session key causes on Windows.
    let scratch_id = format!("artifact-harvest-{}", uuid::Uuid::new_v4().simple());
    let cache = MediaCache::new();
    let mut stored = 0usize;

    for item in items.into_iter().take(MAX_MEDIA_PER_RUN) {
        let Some((bytes, mime_type)) = resolve_bytes(&cache, &item, &scratch_id).await else {
            continue;
        };
        let filename = filename_for(&item, &mime_type);
        match store
            .put(
                session_key,
                run_id,
                ArtifactOrigin::Outbound,
                &filename,
                &mime_type,
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
            Err(e) => warn!(tool, error = %e, "failed to store tool media as an artifact"),
        }
    }

    // Drop the scratch copies. Only files this harvest materialised live under
    // `scratch_id`; an item that resolved to a pre-existing local path was read
    // in place and is untouched by this.
    if let Err(e) = MediaCache::cleanup_session(&scratch_id) {
        debug!(error = %e, "failed to clean the artifact-harvest scratch dir");
    }

    stored
}

/// Read one item's bytes through the media cache — the existing pipeline that
/// decodes `data:` URLs, refuses model-supplied local paths outside the temp
/// root, and fetches remote URLs through `security::ssrf::safe_fetch`. Reusing
/// it (rather than a bare HTTP client here) is what keeps the harvest from
/// becoming a second, unguarded fetch path.
///
/// `None` when the item could not be resolved: the cache then degrades to a
/// URL-only attachment, which carries no readable local file.
async fn resolve_bytes(
    cache: &MediaCache,
    item: &MediaItem,
    scratch_id: &str,
) -> Option<(Vec<u8>, String)> {
    let attachment = cache.download_media_item(item, scratch_id).await;
    let Some(path) = attachment.path else {
        warn!(
            media_type = %item.media_type,
            "media item did not resolve to readable bytes; not stored as an artifact"
        );
        return None;
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => Some((bytes, attachment.mime_type)),
        Err(e) => {
            warn!(error = %e, "resolved media file could not be read; not stored as an artifact");
            None
        }
    }
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

    #[tokio::test]
    async fn tool_media_is_settled_as_an_outbound_artifact() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ArtifactStore::new(dir.path().to_path_buf());
        let value = media_value(json!([item(HELLO_PNG, Some("cat.png"))]));

        let items = media_items(&value);
        let stored = store_media_items(&store, SESSION, Some("run-1"), "media_send", items).await;
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
        let stored = store_media_items(&store, SESSION, None, "bash", media_items(&value)).await;
        assert_eq!(stored, 0);
        assert!(store.list(SESSION).await.expect("list").is_empty());
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
        let stored =
            store_media_items(&store, SESSION, None, "media_send", media_items(&value)).await;
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

        let stored =
            store_media_items(&store, SESSION, None, "media_send", media_items(&value)).await;
        assert_eq!(stored, 1, "the resolvable item is still settled");
        let listed = store.list(SESSION).await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].filename, "cat.png");
    }

    #[tokio::test]
    async fn the_per_run_media_cap_is_honoured() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ArtifactStore::new(dir.path().to_path_buf());
        let items: Vec<Value> = (0..MAX_MEDIA_PER_RUN + 3)
            .map(|_| item(HELLO_PNG, Some("cat.png")))
            .collect();
        let value = media_value(Value::Array(items));

        let stored =
            store_media_items(&store, SESSION, None, "media_send", media_items(&value)).await;
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
