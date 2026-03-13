//! Media downloader — resolves attachments to local files and extracts URLs from text.

use std::collections::HashSet;
use std::path::PathBuf;

use tracing::warn;
use uuid::Uuid;

use super::types::{LocalMedia, MediaCategory, MergedMessage};
use crate::gateway::channel::Attachment;

/// Default maximum file size for downloads (50 MB).
const DEFAULT_MAX_FILE_SIZE: u64 = 50 * 1024 * 1024;

/// Trailing punctuation characters to trim from extracted URLs.
const TRAILING_PUNCT: &[char] = &[',', '.', ')', ']', '>', ';', '\u{3002}', '\u{FF0C}'];

// ---------------------------------------------------------------------------
// MediaDownloader
// ---------------------------------------------------------------------------

/// Downloads remote media attachments to local workspace and extracts URLs
/// from message text.
pub struct MediaDownloader {
    workspace_root: PathBuf,
    http_client: reqwest::Client,
    max_file_size: u64,
    #[allow(dead_code)]
    supported_prefixes: HashSet<String>,
}

impl MediaDownloader {
    /// Create a new downloader rooted at `workspace_root`.
    pub fn new(workspace_root: PathBuf) -> Self {
        let mut supported_prefixes = HashSet::new();
        supported_prefixes.insert("http://".to_string());
        supported_prefixes.insert("https://".to_string());

        Self {
            workspace_root,
            http_client: reqwest::Client::new(),
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            supported_prefixes,
        }
    }

    /// Override the maximum file size for downloads.
    pub fn with_max_file_size(mut self, max_bytes: u64) -> Self {
        self.max_file_size = max_bytes;
        self
    }

    /// Main entry: download / resolve all media from a merged message.
    ///
    /// Individual failures are logged as warnings and skipped.
    pub async fn download_all(&self, merged: &MergedMessage) -> Vec<LocalMedia> {
        let mut results = Vec::new();

        // Process attachments
        for attachment in &merged.attachments {
            match self.process_attachment(attachment).await {
                Ok(local) => results.push(local),
                Err(e) => warn!(
                    attachment_id = %attachment.id,
                    "Skipping attachment: {e}"
                ),
            }
        }

        // Extract URLs from text and download as Link entries
        let urls = extract_urls(&merged.text);
        for url in urls {
            match self.download_url(&url).await {
                Ok(local) => results.push(local),
                Err(e) => warn!(url = %url, "Skipping URL: {e}"),
            }
        }

        results
    }

    /// Process a single attachment into a local file.
    async fn process_attachment(&self, attachment: &Attachment) -> Result<LocalMedia, String> {
        // Case 1: already local
        if let Some(ref path) = attachment.path {
            let p = PathBuf::from(path);
            if p.exists() {
                let category = MediaCategory::from_mime(&attachment.mime_type);
                return Ok(LocalMedia {
                    original: attachment.clone(),
                    local_path: p,
                    media_category: category,
                });
            }
        }

        // Case 2: inline data
        if let Some(ref data) = attachment.data {
            let filename = attachment
                .filename
                .as_deref()
                .unwrap_or("attachment.bin");
            let dir = self
                .workspace_root
                .join("media")
                .join(Uuid::new_v4().to_string());
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|e| format!("Failed to create dir: {e}"))?;
            let dest = dir.join(filename);
            tokio::fs::write(&dest, data)
                .await
                .map_err(|e| format!("Failed to write inline data: {e}"))?;
            let category = MediaCategory::from_mime(&attachment.mime_type);
            return Ok(LocalMedia {
                original: attachment.clone(),
                local_path: dest,
                media_category: category,
            });
        }

        // Case 3: remote URL
        if let Some(ref url) = attachment.url {
            return self.download_attachment_url(attachment, url).await;
        }

        Err("Attachment has no path, data, or url".to_string())
    }

    /// Download an attachment from a remote URL.
    async fn download_attachment_url(
        &self,
        attachment: &Attachment,
        url: &str,
    ) -> Result<LocalMedia, String> {
        let response = self
            .http_client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("HTTP {} for {}", response.status(), url));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read body: {e}"))?;

        if bytes.len() as u64 > self.max_file_size {
            return Err(format!(
                "File too large: {} bytes (max {})",
                bytes.len(),
                self.max_file_size
            ));
        }

        let filename = attachment
            .filename
            .as_deref()
            .unwrap_or("download.bin");
        let dir = self
            .workspace_root
            .join("media")
            .join(Uuid::new_v4().to_string());
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| format!("Failed to create dir: {e}"))?;
        let dest = dir.join(filename);
        tokio::fs::write(&dest, &bytes)
            .await
            .map_err(|e| format!("Failed to write downloaded file: {e}"))?;

        let category = MediaCategory::from_mime(&attachment.mime_type);
        Ok(LocalMedia {
            original: attachment.clone(),
            local_path: dest,
            media_category: category,
        })
    }

    /// Download an extracted URL (from message text) and create a Link entry.
    async fn download_url(&self, url: &str) -> Result<LocalMedia, String> {
        let response = self
            .http_client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("HTTP {} for {}", response.status(), url));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read body: {e}"))?;

        if bytes.len() as u64 > self.max_file_size {
            return Err(format!(
                "File too large: {} bytes (max {})",
                bytes.len(),
                self.max_file_size
            ));
        }

        let dir = self
            .workspace_root
            .join("media")
            .join(Uuid::new_v4().to_string());
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| format!("Failed to create dir: {e}"))?;
        let dest = dir.join("page.html");
        tokio::fs::write(&dest, &bytes)
            .await
            .map_err(|e| format!("Failed to write downloaded page: {e}"))?;

        let attachment = Attachment {
            id: Uuid::new_v4().to_string(),
            mime_type: "text/html".to_string(),
            filename: Some("page.html".to_string()),
            size: Some(bytes.len() as u64),
            url: Some(url.to_string()),
            path: None,
            data: None,
        };

        Ok(LocalMedia {
            original: attachment,
            local_path: dest,
            media_category: MediaCategory::Link,
        })
    }
}

// ---------------------------------------------------------------------------
// extract_urls
// ---------------------------------------------------------------------------

/// Extract http:// and https:// URLs from text, trimming trailing punctuation.
pub fn extract_urls(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter_map(|word| {
            if (word.starts_with("http://") || word.starts_with("https://")) && word.len() > 10 {
                let trimmed = word.trim_end_matches(TRAILING_PUNCT);
                Some(trimmed.to_string())
            } else {
                None
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, MessageId, UserId};
    use crate::gateway::inbound_context::{InboundContext, ReplyRoute};
    use crate::gateway::router::SessionKey;
    use chrono::Utc;

    fn make_merged(text: &str, attachments: Vec<Attachment>) -> MergedMessage {
        let msg = InboundMessage {
            id: MessageId::new("msg-1"),
            channel_id: ChannelId::new("ch-1"),
            conversation_id: ConversationId::new("conv-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: text.to_string(),
            attachments: attachments.clone(),
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        };
        let route = ReplyRoute::new(ChannelId::new("ch-1"), ConversationId::new("conv-1"));
        let session_key = SessionKey::main("main");
        let ctx = InboundContext::new(msg, route, session_key);
        MergedMessage {
            text: text.to_string(),
            attachments,
            primary_context: ctx,
            merged_message_ids: vec![MessageId::new("msg-1")],
            merge_count: 1,
        }
    }

    // --- extract_urls ---

    #[test]
    fn test_extract_urls_basic() {
        let urls = extract_urls("Check out https://example.com/page please");
        assert_eq!(urls, vec!["https://example.com/page"]);
    }

    #[test]
    fn test_extract_urls_multiple() {
        let urls = extract_urls(
            "Visit https://example.com and http://other.org/path for details",
        );
        assert_eq!(
            urls,
            vec![
                "https://example.com".to_string(),
                "http://other.org/path".to_string(),
            ]
        );
    }

    #[test]
    fn test_extract_urls_trailing_punctuation() {
        let urls = extract_urls("See https://example.com/page. Thanks");
        assert_eq!(urls, vec!["https://example.com/page"]);
    }

    #[test]
    fn test_extract_urls_no_urls() {
        let urls = extract_urls("No links here, just plain text");
        assert!(urls.is_empty());
    }

    #[test]
    fn test_extract_urls_short_rejected() {
        // "http://x" is only 8 chars, below the > 10 threshold
        let urls = extract_urls("Try http://x end");
        assert!(urls.is_empty());
    }

    // --- download_all ---

    #[tokio::test]
    async fn test_download_inline_data() {
        let tmp = tempfile::tempdir().unwrap();
        let downloader = MediaDownloader::new(tmp.path().to_path_buf());

        let attachment = Attachment {
            id: "a1".to_string(),
            mime_type: "text/plain".to_string(),
            filename: Some("note.txt".to_string()),
            size: None,
            url: None,
            path: None,
            data: Some(b"hello world".to_vec()),
        };

        let merged = make_merged("", vec![attachment]);
        let results = downloader.download_all(&merged).await;

        assert_eq!(results.len(), 1);
        let local = &results[0];
        assert!(local.local_path.exists());
        assert_eq!(local.media_category, MediaCategory::Document);
        let content = tokio::fs::read_to_string(&local.local_path).await.unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn test_download_local_path() {
        let tmp = tempfile::tempdir().unwrap();
        let local_file = tmp.path().join("existing.txt");
        tokio::fs::write(&local_file, "already here").await.unwrap();

        let downloader = MediaDownloader::new(tmp.path().to_path_buf());

        let attachment = Attachment {
            id: "a2".to_string(),
            mime_type: "text/plain".to_string(),
            filename: Some("existing.txt".to_string()),
            size: None,
            url: None,
            path: Some(local_file.to_string_lossy().to_string()),
            data: None,
        };

        let merged = make_merged("", vec![attachment]);
        let results = downloader.download_all(&merged).await;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].local_path, local_file);
        assert_eq!(results[0].media_category, MediaCategory::Document);
    }

    #[tokio::test]
    async fn test_download_no_attachments_no_urls() {
        let tmp = tempfile::tempdir().unwrap();
        let downloader = MediaDownloader::new(tmp.path().to_path_buf());

        let merged = make_merged("Just text, no links", vec![]);
        let results = downloader.download_all(&merged).await;

        assert!(results.is_empty());
    }
}
