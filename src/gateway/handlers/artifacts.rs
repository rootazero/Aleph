//! Artifact metadata RPCs.
//!
//! Methods:
//! - `artifacts.list       { session_key }` → `{ session_key, items, count }`
//! - `session.export_html  { session_key }` → `{ url, filename, size }`
//!
//! These are pure I/O (R4): they parse params, ask core, and serialize. Where
//! the bytes live is [`crate::artifacts::ArtifactStore`]'s business; how the
//! exported document is built is [`crate::export`]'s. Nothing here decides
//! either — the only thing this file owns is the shape of the JSON.
//!
//! The `url` on every item is the [`crate::gateway::security::ArtifactCapabilities`]
//! capability route: `/artifact/<cap>/<id>/<filename>`. Minting it here is the
//! point — this RPC arrives over an already-authenticated WS connection, and
//! the capability is how that authentication is carried to the byte route,
//! which has no session of its own.

use serde::Deserialize;
use serde_json::json;

use crate::artifacts::{ArtifactOrigin, ArtifactRecord, ArtifactStore};
use crate::gateway::security::ArtifactCapabilities;
use crate::gateway::session_store::SessionStore;
use crate::sync_primitives::Arc;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::super::router::SessionKey;
use super::parse_params;

/// Percent-encode set for the filename path segment of an artifact URL.
///
/// Filenames are already sanitized by the store (no directory components), but
/// they can still hold spaces, `#`, `?` and non-ASCII — all of which change how
/// a URL parses. `-`, `_` and `.` stay literal so an extension is still legible
/// in the address bar.
const URL_SEGMENT_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.');

/// Params for every RPC in this file: one session, addressed by key.
#[derive(Debug, Deserialize)]
pub struct SessionScopeParams {
    pub session_key: String,
}

/// Parse and canonicalise the session key, or return the error response.
///
/// Round-tripping through [`SessionKey`] matters: producers stamp artifacts
/// with `SessionKey::to_key_string()`, so a caller passing a legacy spelling
/// must be normalised to the same string or it would read an empty session.
// Same trade-off as `parse_params`: `JsonRpcResponse` is a large `Err` variant,
// but boxing it would make every handler call site unwrap a `Box` to return it.
#[allow(clippy::result_large_err)]
fn resolve_session(request: &JsonRpcRequest, raw: &str) -> Result<SessionKey, JsonRpcResponse> {
    SessionKey::from_key_string(raw).ok_or_else(|| {
        JsonRpcResponse::error(
            request.id.clone(),
            INVALID_PARAMS,
            "Invalid session_key format",
        )
    })
}

/// Build the capability URL for one record.
fn artifact_url(cap: &str, record: &ArtifactRecord) -> String {
    let filename = utf8_percent_encode(&record.filename, URL_SEGMENT_ENCODE_SET);
    format!("/artifact/{cap}/{}/{filename}", record.id)
}

/// Serialize one record for the wire, including its capability URL.
fn record_json(cap: &str, record: &ArtifactRecord) -> serde_json::Value {
    json!({
        "id": record.id,
        "filename": record.filename,
        "mime_type": record.mime_type,
        "size": record.size,
        "origin": record.origin,
        "run_id": record.run_id,
        "created_at": record.created_at,
        "url": artifact_url(cap, record),
    })
}

// ─── artifacts.list ──────────────────────────────────────────────────────────

/// `artifacts.list { session_key }` → every artifact of a session, newest first.
///
/// This is the authoritative read. The `session.artifact` event on the bus is a
/// content-free invalidation ping — the `agent_trace` stream is deliberately
/// lossy, so a client that treated live events as the record would show ghosts
/// the moment a frame is dropped. Clients re-read this list instead.
pub async fn handle_list(request: JsonRpcRequest, store: Arc<ArtifactStore>) -> JsonRpcResponse {
    let params: SessionScopeParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let session_key = match resolve_session(&request, &params.session_key) {
        Ok(k) => k.to_key_string(),
        Err(e) => return e,
    };

    let records = match store.list(&session_key).await {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to list artifacts: {e}"),
            );
        }
    };

    let cap = ArtifactCapabilities::mint(&session_key);
    let items: Vec<serde_json::Value> = records.iter().map(|r| record_json(&cap, r)).collect();
    let count = items.len();

    JsonRpcResponse::success(
        request.id,
        json!({
            "session_key": session_key,
            "items": items,
            "count": count,
        }),
    )
}

// ─── session.export_html ─────────────────────────────────────────────────────

/// `session.export_html { session_key }` → a self-contained HTML transcript.
///
/// The document is built by [`crate::export::render_session_html`] and settled
/// through the same [`ArtifactStore`] as everything else, so it is listed,
/// capability-addressed and evicted exactly like any other artifact — there is
/// no second delivery mechanism to secure.
pub async fn handle_export_html(
    request: JsonRpcRequest,
    store: Arc<ArtifactStore>,
    sessions: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let params: SessionScopeParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let key = match resolve_session(&request, &params.session_key) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let session_key = key.to_key_string();

    let messages = match sessions.get_history(&key, None).await {
        Ok(m) => m,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to read session history: {e}"),
            );
        }
    };
    let export_messages: Vec<crate::export::ExportMessage> = messages
        .into_iter()
        .map(|m| crate::export::ExportMessage {
            // Timestamp first: `role`/`content` move out of `m`, and the
            // accessor needs `m` whole.
            timestamp: m.rfc3339(),
            role: m.role,
            text: m.content,
        })
        .collect();

    let records = match store.list(&session_key).await {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to list artifacts: {e}"),
            );
        }
    };
    let embeddable = embeddable_records(&records);
    let export_artifacts =
        crate::export::collect_artifacts(&store, &session_key, &embeddable).await;

    let title = match sessions.get_metadata(&key).await {
        Ok(Some(meta)) => meta
            .label
            .or(meta.derived_title)
            .or(meta.topic)
            .unwrap_or_else(|| session_key.clone()),
        _ => session_key.clone(),
    };

    let html = crate::export::render_session_html(&title, &export_messages, &export_artifacts);
    let filename = format!(
        "aleph-session-{}.html",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    );

    let record = match store
        .put(
            &session_key,
            None,
            ArtifactOrigin::Export,
            &filename,
            "text/html",
            html.as_bytes(),
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to store the exported document: {e}"),
            );
        }
    };

    let cap = ArtifactCapabilities::mint(&session_key);
    JsonRpcResponse::success(
        request.id,
        json!({
            "url": artifact_url(&cap, &record),
            "filename": record.filename,
            "size": record.size,
        }),
    )
}

/// Which of a session's artifacts a transcript export may embed.
///
/// Everything except earlier exports. An export is itself stored as an artifact
/// of the session, so embedding one would make each export carry all of its
/// predecessors — quadratic growth from a self-referential document. Published
/// deliverables are *not* excluded: the transcript is the complete record of
/// the session, and the work product is part of that record.
fn embeddable_records(records: &[ArtifactRecord]) -> Vec<ArtifactRecord> {
    records
        .iter()
        .filter(|r| r.origin != ArtifactOrigin::Export)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn req(method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: Some(params),
            id: Some(json!(1)),
        }
    }

    async fn store_with_one_artifact(session_key: &str) -> (TempDir, Arc<ArtifactStore>) {
        let tmp = TempDir::new().expect("tempdir");
        let store = Arc::new(ArtifactStore::new(tmp.path().to_path_buf()));
        store
            .put(
                session_key,
                Some("run-1"),
                ArtifactOrigin::Outbound,
                "chart one.png",
                "image/png",
                b"not really a png",
            )
            .await
            .expect("put");
        (tmp, store)
    }

    #[tokio::test]
    async fn list_returns_items_with_capability_urls() {
        let session_key = "agent:main:main";
        let (_tmp, store) = store_with_one_artifact(session_key).await;

        let resp = handle_list(
            req("artifacts.list", json!({ "session_key": session_key })),
            store,
        )
        .await;

        let result = resp.result.expect("expected success");
        assert_eq!(result["count"], 1);
        let item = &result["items"][0];
        assert_eq!(item["mime_type"], "image/png");
        assert_eq!(item["origin"], "outbound");
        assert_eq!(item["run_id"], "run-1");
        assert_eq!(item["filename"], "chart one.png");

        let url = item["url"].as_str().expect("url");
        let id = item["id"].as_str().expect("id");
        // Space is encoded, the extension is not, and the id is a path segment.
        assert!(url.ends_with(&format!("/{id}/chart%20one.png")), "{url}");
        assert!(url.starts_with("/artifact/"), "{url}");
    }

    #[tokio::test]
    async fn list_url_carries_a_capability_scoped_to_that_session() {
        let session_key = "agent:main:main";
        let (_tmp, store) = store_with_one_artifact(session_key).await;

        let resp = handle_list(
            req("artifacts.list", json!({ "session_key": session_key })),
            store,
        )
        .await;
        let url = resp.result.expect("success")["items"][0]["url"]
            .as_str()
            .expect("url")
            .to_string();
        let cap = url.split('/').nth(2).expect("cap segment");

        assert!(ArtifactCapabilities::validate(cap, session_key));
        assert!(!ArtifactCapabilities::validate(cap, "agent:other:main"));
    }

    #[tokio::test]
    async fn list_of_a_session_without_artifacts_is_empty_not_an_error() {
        let tmp = TempDir::new().expect("tempdir");
        let store = Arc::new(ArtifactStore::new(tmp.path().to_path_buf()));

        let resp = handle_list(
            req(
                "artifacts.list",
                json!({ "session_key": "agent:main:main" }),
            ),
            store,
        )
        .await;

        let result = resp.result.expect("expected success");
        assert_eq!(result["count"], 0);
        assert!(result["items"].as_array().expect("items").is_empty());
    }

    #[tokio::test]
    async fn list_rejects_a_malformed_session_key() {
        let tmp = TempDir::new().expect("tempdir");
        let store = Arc::new(ArtifactStore::new(tmp.path().to_path_buf()));

        let resp = handle_list(
            req("artifacts.list", json!({ "session_key": "not-a-session" })),
            store,
        )
        .await;

        assert_eq!(resp.error.expect("expected error").code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn list_requires_params() {
        let tmp = TempDir::new().expect("tempdir");
        let store = Arc::new(ArtifactStore::new(tmp.path().to_path_buf()));

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "artifacts.list".to_string(),
            params: None,
            id: Some(json!(1)),
        };
        let resp = handle_list(request, store).await;
        assert_eq!(resp.error.expect("expected error").code, INVALID_PARAMS);
    }

    /// Byte-budget behaviour is `export::collect_artifacts`' own business and
    /// is tested there. What belongs here is the transcript's *selection* rule.
    #[tokio::test]
    async fn an_export_never_embeds_an_earlier_export_but_keeps_the_deliverable() {
        let session_key = "agent:main:main";
        let tmp = TempDir::new().expect("tempdir");
        let store = ArtifactStore::new(tmp.path().to_path_buf());

        let earlier = store
            .put(
                session_key,
                None,
                ArtifactOrigin::Export,
                "aleph-session-20260726-090000.html",
                "text/html",
                b"<p>an earlier export</p>",
            )
            .await
            .expect("put export");
        let deliverable = store
            .put(
                session_key,
                None,
                ArtifactOrigin::Deliverable,
                "q3-review.html",
                "text/html",
                b"<p>the work product</p>",
            )
            .await
            .expect("put deliverable");
        let attachment = store
            .put(
                session_key,
                None,
                ArtifactOrigin::Inbound,
                "notes.txt",
                "text/plain",
                b"kept",
            )
            .await
            .expect("put inbound");

        let kept = embeddable_records(&[earlier, deliverable, attachment]);

        let names: Vec<&str> = kept.iter().map(|r| r.filename.as_str()).collect();
        assert_eq!(
            names,
            vec!["q3-review.html", "notes.txt"],
            "prior exports are dropped; the deliverable is part of the record"
        );
    }
}
