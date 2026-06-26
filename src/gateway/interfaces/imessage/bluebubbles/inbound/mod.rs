pub mod dedup;
pub mod mapper;
pub mod poll;
pub mod webhook_server;

use crate::gateway::channel::Attachment;
use crate::gateway::interfaces::imessage::bluebubbles::api::BlueBubblesApi;

/// Download all attachment GUIDs and return `Attachment` values for each
/// successful download. Called by both the webhook handler and the catch-up
/// poll to keep the download loop in one place.
pub async fn download_attachments(
    api: &BlueBubblesApi,
    guids: &[(String, String)],
) -> Vec<Attachment> {
    let mut result = Vec::new();
    for (g, mime) in guids {
        if let Some(path) = api.download_attachment(g, mime).await {
            result.push(Attachment {
                id: g.clone(),
                mime_type: mime.clone(),
                filename: None,
                size: None,
                url: None,
                path: Some(path),
                data: None,
            });
        }
    }
    result
}
