use crate::gateway::channel::ChannelError;
use matrix_sdk::media::{MediaFormat, MediaRequestParameters};
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::Client;

pub const MATRIX_MEDIA_MAX_SIZE: u64 = 100 * 1024 * 1024;

pub async fn upload_media(
    client: &Client,
    content: Vec<u8>,
    mime_type: &str,
    _filename: Option<&str>,
) -> Result<String, ChannelError> {
    if content.len() as u64 > MATRIX_MEDIA_MAX_SIZE {
        return Err(ChannelError::SendFailed(format!(
            "Media file too large: {} bytes (max {})",
            content.len(),
            MATRIX_MEDIA_MAX_SIZE
        )));
    }

    let mime: mime::Mime = mime_type
        .parse()
        .map_err(|e| ChannelError::SendFailed(format!("Invalid MIME type: {e}")))?;

    let response = client
        .media()
        .upload(&mime, content, None)
        .await
        .map_err(|e| ChannelError::SendFailed(format!("Media upload failed: {e}")))?;

    Ok(response.content_uri.to_string())
}

pub async fn download_media(
    client: &Client,
    mxc_uri: &str,
) -> Result<(Vec<u8>, String), ChannelError> {
    let uri = matrix_sdk::ruma::OwnedMxcUri::from(mxc_uri);
    if !uri.is_valid() {
        return Err(ChannelError::ReceiveFailed(format!(
            "Invalid mxc URI: {}",
            mxc_uri
        )));
    }

    let media_request = MediaRequestParameters {
        source: MediaSource::Plain(uri),
        format: MediaFormat::File,
    };

    let data = client
        .media()
        .get_media_content(&media_request, false)
        .await
        .map_err(|e| ChannelError::ReceiveFailed(format!("Media download failed: {e}")))?;

    let content_type = mime_guess::from_path(mxc_uri)
        .first_or_octet_stream()
        .to_string();

    Ok((data, content_type))
}
