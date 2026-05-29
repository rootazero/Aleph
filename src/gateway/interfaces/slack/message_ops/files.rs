//! Inbound Slack file attachment extraction.
//!
//! Slack delivers file metadata in a message event's `files[]` array, but the
//! actual bytes live behind `url_private`, which requires the bot token in an
//! `Authorization: Bearer xoxb-…` header. The generic media pipeline
//! ([`crate::gateway::pipeline::media_download`]) fetches via `safe_fetch`,
//! which has no auth-header support — pointing it at a `url_private` would
//! return an HTML login page, not the file.
//!
//! Therefore the Slack limb (which holds the bot token) downloads its own
//! files here and hands the bytes downstream as inline [`Attachment::data`].
//! Oversized or failed downloads degrade to metadata-only attachments with no
//! `url`, so the pipeline preserves them via its file-id passthrough instead
//! of re-fetching unauthenticated.

use super::*;
use crate::gateway::channel::Attachment;

/// Per-file download timeout — keeps a slow file from stalling the socket loop.
const FILE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15);

/// A Slack file reference parsed from a message `files[]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlackFile {
    pub id: String,
    pub name: String,
    pub mimetype: String,
    pub size: u64,
    pub url_private: Option<String>,
}

/// Parse the `files[]` array from a Slack message event object.
///
/// Handles both top-level message/app_mention events and `message_changed`
/// events (which nest the real message under `message`). Pure and testable.
pub(crate) fn parse_files(event: &serde_json::Value) -> Vec<SlackFile> {
    // `message_changed` nests the actual message (and its files) under "message".
    let container = event.get("message").unwrap_or(event);
    let Some(files) = container.get("files").and_then(|f| f.as_array()) else {
        return Vec::new();
    };
    files.iter().filter_map(parse_one).collect()
}

/// Returns `true` if the event carries at least one file attachment.
///
/// Used by the converters to let a file-only message (empty text) through
/// instead of filtering it as an empty message.
pub(crate) fn has_files(event: &serde_json::Value) -> bool {
    let container = event.get("message").unwrap_or(event);
    container
        .get("files")
        .and_then(|f| f.as_array())
        .is_some_and(|a| !a.is_empty())
}

fn parse_one(f: &serde_json::Value) -> Option<SlackFile> {
    let id = f["id"].as_str()?.to_string();
    let name = f["name"].as_str().unwrap_or("file").to_string();
    let mimetype = f["mimetype"]
        .as_str()
        .unwrap_or("application/octet-stream")
        .to_string();
    let size = f["size"].as_u64().unwrap_or(0);
    // Prefer the explicit download URL; fall back to the inline-view URL.
    let url_private = f["url_private_download"]
        .as_str()
        .or_else(|| f["url_private"].as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    Some(SlackFile {
        id,
        name,
        mimetype,
        size,
        url_private,
    })
}

/// Download Slack files into [`Attachment`]s using the bot token for auth.
///
/// Files whose advertised size is within `max_bytes` are downloaded inline;
/// oversized, missing-URL, or failed downloads degrade to metadata-only
/// attachments (no `url`) so the generic media pipeline keeps the metadata
/// rather than re-fetching an unauthenticated `url_private`.
pub(crate) async fn fetch_attachments(
    client: &reqwest::Client,
    bot_token: &str,
    files: Vec<SlackFile>,
    max_bytes: u64,
) -> Vec<Attachment> {
    let mut out = Vec::with_capacity(files.len());
    for file in files {
        let data = match &file.url_private {
            // size == 0 means Slack omitted it — attempt the download and let
            // the post-fetch byte check enforce the cap.
            Some(url) if file.size <= max_bytes => {
                download_one(client, bot_token, url, max_bytes).await
            }
            Some(_) => {
                tracing::debug!(
                    "Slack file {} exceeds media cap ({} > {} bytes), metadata only",
                    file.id,
                    file.size,
                    max_bytes
                );
                None
            }
            None => None,
        };
        out.push(Attachment {
            id: file.id,
            mime_type: file.mimetype,
            filename: Some(file.name),
            size: Some(file.size),
            // Never expose the unauthenticated url_private to the pipeline.
            url: None,
            path: None,
            data,
        });
    }
    out
}

async fn download_one(
    client: &reqwest::Client,
    bot_token: &str,
    url: &str,
    max_bytes: u64,
) -> Option<Vec<u8>> {
    let fetch = async {
        let resp = client
            .get(url)
            .header("Authorization", format!("Bearer {bot_token}"))
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            tracing::warn!("Slack file download failed: HTTP {}", resp.status());
            return None;
        }
        resp.bytes().await.ok()
    };

    let bytes = match tokio::time::timeout(FILE_DOWNLOAD_TIMEOUT, fetch).await {
        Ok(Some(b)) => b,
        Ok(None) => return None,
        Err(_elapsed) => {
            tracing::warn!("Slack file download timed out after {FILE_DOWNLOAD_TIMEOUT:?}");
            return None;
        }
    };

    // Guard against Slack lying about (or omitting) the size.
    if bytes.len() as u64 > max_bytes {
        tracing::warn!(
            "Slack file body {} bytes exceeds cap {} bytes, dropping bytes",
            bytes.len(),
            max_bytes
        );
        return None;
    }

    Some(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_files_empty_when_absent() {
        let event = serde_json::json!({ "type": "message", "text": "hi" });
        assert!(parse_files(&event).is_empty());
    }

    #[test]
    fn parse_files_extracts_basic_metadata() {
        let event = serde_json::json!({
            "type": "message",
            "files": [{
                "id": "F123",
                "name": "diagram.png",
                "mimetype": "image/png",
                "size": 2048,
                "url_private": "https://files.slack.com/F123/diagram.png"
            }]
        });
        let files = parse_files(&event);
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0],
            SlackFile {
                id: "F123".to_string(),
                name: "diagram.png".to_string(),
                mimetype: "image/png".to_string(),
                size: 2048,
                url_private: Some("https://files.slack.com/F123/diagram.png".to_string()),
            }
        );
    }

    #[test]
    fn parse_files_prefers_download_url() {
        let event = serde_json::json!({
            "files": [{
                "id": "F1",
                "name": "a.bin",
                "mimetype": "application/octet-stream",
                "size": 10,
                "url_private": "https://files.slack.com/view",
                "url_private_download": "https://files.slack.com/download"
            }]
        });
        let files = parse_files(&event);
        assert_eq!(
            files[0].url_private.as_deref(),
            Some("https://files.slack.com/download")
        );
    }

    #[test]
    fn parse_files_reads_message_changed_inner() {
        let event = serde_json::json!({
            "type": "message",
            "subtype": "message_changed",
            "message": {
                "type": "message",
                "files": [{ "id": "F9", "name": "x.txt", "mimetype": "text/plain", "size": 5 }]
            }
        });
        let files = parse_files(&event);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id, "F9");
        assert_eq!(files[0].url_private, None);
    }

    #[test]
    fn parse_files_applies_defaults_for_missing_fields() {
        let event = serde_json::json!({
            "files": [{ "id": "F2" }]
        });
        let files = parse_files(&event);
        assert_eq!(files[0].name, "file");
        assert_eq!(files[0].mimetype, "application/octet-stream");
        assert_eq!(files[0].size, 0);
        assert_eq!(files[0].url_private, None);
    }

    #[test]
    fn has_files_detects_presence_and_absence() {
        assert!(!has_files(&serde_json::json!({ "text": "hi" })));
        assert!(!has_files(&serde_json::json!({ "files": [] })));
        assert!(has_files(
            &serde_json::json!({ "files": [{ "id": "F1" }] })
        ));
        // message_changed nesting
        assert!(has_files(&serde_json::json!({
            "subtype": "message_changed",
            "message": { "files": [{ "id": "F2" }] }
        })));
    }

    #[test]
    fn parse_files_skips_entries_without_id() {
        let event = serde_json::json!({
            "files": [{ "name": "noid.png", "mimetype": "image/png", "size": 1 }]
        });
        assert!(parse_files(&event).is_empty());
    }

    #[tokio::test]
    async fn fetch_attachments_oversized_is_metadata_only() {
        let client = reqwest::Client::new();
        let files = vec![SlackFile {
            id: "F1".to_string(),
            name: "huge.bin".to_string(),
            mimetype: "application/octet-stream".to_string(),
            size: 100 * 1024 * 1024,
            url_private: Some("https://files.slack.com/huge".to_string()),
        }];
        // max 1 MB — file is 100 MB, so no network call is made.
        let out = fetch_attachments(&client, "xoxb-test", files, 1024 * 1024).await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "F1");
        assert_eq!(out[0].data, None);
        assert_eq!(out[0].url, None);
        assert_eq!(out[0].size, Some(100 * 1024 * 1024));
        assert_eq!(out[0].filename.as_deref(), Some("huge.bin"));
    }

    #[tokio::test]
    async fn fetch_attachments_no_url_is_metadata_only() {
        let client = reqwest::Client::new();
        let files = vec![SlackFile {
            id: "F2".to_string(),
            name: "x.txt".to_string(),
            mimetype: "text/plain".to_string(),
            size: 5,
            url_private: None,
        }];
        let out = fetch_attachments(&client, "xoxb-test", files, 1024 * 1024).await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].data, None);
        assert_eq!(out[0].url, None);
        assert_eq!(out[0].mime_type, "text/plain");
    }
}
