//! Session-artifact RPC wrappers: list what a session produced, and export the
//! whole transcript as a self-contained HTML document.
//!
//! Mirrors the `rpc_call` pattern used across `api/*`. Both methods are pure
//! I/O — the core decides where bytes live, when they expire, and how the
//! export document is built (R4).

use crate::context::DashboardState;
use serde::Deserialize;
use serde_json::{json, Value};

/// Topic the gateway pings when a session's artifact set changed. The event is
/// content-free by design (`{"session_key": …}`), so the Panel must re-read
/// [`ArtifactsApi::list`] — the authoritative source — rather than trying to
/// patch its local list from the frame.
pub const ARTIFACT_TOPIC: &str = "session.artifact";

/// One row of `artifacts.list`. Mirrors the backend `ArtifactRecord` plus the
/// capability-scoped `url` the handler mints (`/artifact/{cap}/{id}/{name}`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ArtifactItem {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
    /// `"inbound"` | `"outbound"` | `"export"`.
    pub origin: String,
    #[serde(default)]
    pub run_id: Option<String>,
    /// Unix millis.
    pub created_at: i64,
    /// Root-relative byte URL. The Panel is served from the gateway's own
    /// origin, so this resolves without a base — and keeping it relative is
    /// what lets the same markup work behind a reverse proxy.
    pub url: String,
}

impl ArtifactItem {
    /// True when the row can be shown as a thumbnail rather than a file chip.
    #[must_use]
    pub fn is_image(&self) -> bool {
        self.mime_type.starts_with("image/")
    }

    /// Human-readable size, matching the core's export copy (KB/MB, 1 decimal).
    #[must_use]
    pub fn human_size(&self) -> String {
        human_size(self.size)
    }
}

/// Format a byte count the way the rest of the product does.
#[must_use]
pub fn human_size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let b = bytes as f64;
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Result of `session.export_html` — the export is itself stored as an
/// artifact, so it comes back as a byte URL like any other row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ExportResult {
    pub url: String,
    pub filename: String,
    pub size: u64,
}

pub struct ArtifactsApi;

impl ArtifactsApi {
    /// List a session's stored artifacts, newest first.
    pub async fn list(
        state: &DashboardState,
        session_key: &str,
    ) -> Result<Vec<ArtifactItem>, String> {
        let result = state
            .rpc_call("artifacts.list", json!({ "session_key": session_key }))
            .await?;
        let items = result.get("items").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(items).map_err(|e| e.to_string())
    }

    /// Render the session transcript to a self-contained HTML document and
    /// store it as an `export` artifact. Returns its byte URL.
    pub async fn export_html(
        state: &DashboardState,
        session_key: &str,
    ) -> Result<ExportResult, String> {
        let result = state
            .rpc_call("session.export_html", json!({ "session_key": session_key }))
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }
}

/// Does this `session.artifact` frame concern `session_key`?
///
/// The gateway subscribes topic-wide, so a background session's ping arrives in
/// every Panel. Scoping here keeps a background run from forcing a re-read (and
/// a visible flicker) in the conversation the user is actually looking at —
/// the same discipline `team.*` events already follow.
#[must_use]
pub fn ping_is_for_session(data: &Value, session_key: &str) -> bool {
    data.get("session_key").and_then(Value::as_str) == Some(session_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_a_list_row() {
        let j = r#"{"id":"3f2a","filename":"chart one.png","mime_type":"image/png",
                    "size":16384,"origin":"outbound","run_id":"run-1",
                    "created_at":1753500000000,
                    "url":"/artifact/cap/3f2a/chart%20one.png"}"#;
        let it: ArtifactItem = serde_json::from_str(j).unwrap();
        assert_eq!(it.id, "3f2a");
        assert_eq!(it.origin, "outbound");
        assert_eq!(it.run_id.as_deref(), Some("run-1"));
        assert!(it.is_image());
        assert_eq!(it.human_size(), "16.0 KB");
    }

    #[test]
    fn a_row_without_a_run_id_still_deserializes() {
        // `run_id` is null for anything the core could not attribute to a run
        // (an inbound attachment on a session's very first turn, an export).
        let j = r#"{"id":"a","filename":"f.pdf","mime_type":"application/pdf",
                    "size":10,"origin":"inbound","run_id":null,
                    "created_at":1,"url":"/artifact/c/a/f.pdf"}"#;
        let it: ArtifactItem = serde_json::from_str(j).unwrap();
        assert_eq!(it.run_id, None);
        assert!(!it.is_image());
    }

    #[test]
    fn deserializes_an_export_result() {
        let j = r#"{"url":"/artifact/c/i/aleph-session.html",
                    "filename":"aleph-session.html","size":20481}"#;
        let r: ExportResult = serde_json::from_str(j).unwrap();
        assert_eq!(r.filename, "aleph-session.html");
        assert_eq!(r.size, 20481);
    }

    #[test]
    fn human_size_switches_units() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn a_ping_for_another_session_is_ignored() {
        let mine = json!({ "session_key": "agent:main:main" });
        let theirs = json!({ "session_key": "agent:main:other" });
        assert!(ping_is_for_session(&mine, "agent:main:main"));
        assert!(!ping_is_for_session(&theirs, "agent:main:main"));
        // A malformed frame must not be read as "everyone".
        assert!(!ping_is_for_session(&json!({}), "agent:main:main"));
    }
}
