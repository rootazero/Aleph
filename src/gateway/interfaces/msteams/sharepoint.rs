//! SharePoint File Upload Client

use serde::Deserialize;
use std::borrow::Cow;

use crate::gateway::channel::ChannelError;
use crate::sync_primitives::Arc;

#[derive(Debug)]
pub struct ShareLink {
    pub web_url: url::Url,
    pub expiration_date_time: Option<chrono::DateTime<chrono::Utc>>,
}

pub struct SharePointClient {
    graph_client: Arc<super::graph::GraphClient>,
}

impl SharePointClient {
    pub fn new(graph_client: Arc<super::graph::GraphClient>) -> Self {
        Self { graph_client }
    }

    async fn get_channel_drive(&self, team_id: &str, channel_id: &str) -> Result<String, ChannelError> {
        #[derive(Deserialize)]
        struct DriveResponse {
            id: String,
        }

        let response: DriveResponse = self
            .graph_client
            .get(&format!("/teams/{}/channels/{}/drive", team_id, channel_id))
            .await?;

        Ok(response.id)
    }

    pub async fn upload_file(
        &self,
        team_id: &str,
        channel_id: &str,
        file_name: &str,
        content: Vec<u8>,
    ) -> Result<ShareLink, ChannelError> {
        let drive_id = self.get_channel_drive(team_id, channel_id).await?;

        #[derive(Deserialize)]
        struct UploadSessionResponse {
            #[serde(rename = "uploadUrl")]
            upload_url: String,
        }

        let session: UploadSessionResponse = self
            .graph_client
            .put_with_empty(&format!(
                "/drives/{}/root:/{}/createUploadSession",
                drive_id, file_name
            ))
            .await?;

        self.upload_chunks(&session.upload_url, &content).await?;
        self.create_share_link(&drive_id, file_name).await
    }

    async fn upload_chunks(&self, upload_url: &str, content: &[u8]) -> Result<(), ChannelError> {
        const CHUNK_SIZE: usize = 4 * 1024 * 1024;

        let total_len = content.len();
        let chunks: Vec<&[u8]> = content.chunks(CHUNK_SIZE).collect();

        for (index, chunk) in chunks.iter().enumerate() {
            let start = index * CHUNK_SIZE;
            let end = start + chunk.len() - 1;
            let range_header = format!("bytes {}-{}/{}", start, end, total_len);

            let _: serde_json::Value = self
                .graph_client
                .put_raw(upload_url, chunk, &[("Content-Range", range_header.as_str())])
                .await?;
        }

        Ok(())
    }

    async fn create_share_link(&self, drive_id: &str, file_name: &str) -> Result<ShareLink, ChannelError> {
        #[derive(Deserialize)]
        struct PermissionResponse {
            link: ShareLinkResponse,
        }

        #[derive(Deserialize)]
        struct ShareLinkResponse {
            web_url: String,
            #[serde(rename = "expirationDateTime")]
            expiration_date_time: Option<String>,
        }

        let response: PermissionResponse = self
            .graph_client
            .post_json(
                &format!("/drives/{}/items/root:/{}/createLink", drive_id, file_name),
                serde_json::json!({
                    "type": "view",
                    "scope": "organization"
                }),
            )
            .await?;

        let expiration: Option<chrono::DateTime<chrono::Utc>> = response
            .link
            .expiration_date_time
            .and_then(|s: String| {
                chrono::DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|dt| dt.with_timezone(&chrono::Utc))
            });

        Ok(ShareLink {
            web_url: url::Url::parse(&response.link.web_url)
                .map_err(|e| ChannelError::Internal(format!("Invalid share URL: {e}")))?,
            expiration_date_time: expiration,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_share_link_response_parsing() {
        let json = r#"{
            "link": {
                "web_url": "https://tenant.sharepoint.com/sites/site/Shared%20Documents/file.txt",
                "expirationDateTime": "2026-12-31T23:59:59Z"
            }
        }"#;

        #[derive(Deserialize)]
        struct PermissionResponse {
            link: ShareLinkResponse,
        }

        #[derive(Deserialize)]
        struct ShareLinkResponse {
            web_url: String,
            #[serde(rename = "expirationDateTime")]
            expiration_date_time: Option<String>,
        }

        let response: PermissionResponse = serde_json::from_str(json).unwrap();
        assert!(response.link.web_url.contains("sharepoint.com"));
        assert!(response.link.expiration_date_time.is_some());
    }

    #[test]
    fn test_share_link_without_expiration() {
        let json = r#"{
            "link": {
                "web_url": "https://tenant.sharepoint.com/sites/site/Shared%20Documents/file.txt"
            }
        }"#;

        #[derive(Deserialize)]
        struct PermissionResponse {
            link: ShareLinkResponse,
        }

        #[derive(Deserialize)]
        struct ShareLinkResponse {
            web_url: String,
            #[serde(rename = "expirationDateTime")]
            expiration_date_time: Option<String>,
        }

        let response: PermissionResponse = serde_json::from_str(json).unwrap();
        assert!(response.link.web_url.contains("sharepoint.com"));
        assert!(response.link.expiration_date_time.is_none());
    }
}