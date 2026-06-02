use crate::gateway::channel::{Attachment, ChannelError};
use matrix_sdk::media::{MediaFormat, MediaRequestParameters};
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::Client;
use std::time::Duration;

pub const MATRIX_MEDIA_MAX_SIZE: u64 = 100 * 1024 * 1024;

/// Per-attachment download timeout — keeps a slow file from stalling the sync loop.
const INBOUND_MEDIA_TIMEOUT: Duration = Duration::from_secs(30);

/// Returns `true` when an advertised attachment size is within `max_bytes`.
///
/// An absent size (`None`) is treated as within-cap; the post-download byte
/// check still enforces the limit against the actual payload.
fn within_cap(size: Option<u64>, max_bytes: u64) -> bool {
    size.is_none_or(|s| s <= max_bytes)
}

/// Returns `true` when an attachment carries an undownloaded `mxc://` URL.
///
/// Attachments that already hold inline bytes, or whose URL is HTTP / absent,
/// are left for the generic pipeline and do not need limb hydration.
fn needs_hydration(att: &Attachment) -> bool {
    att.data.is_none() && att.url.as_deref().is_some_and(|u| u.starts_with("mxc://"))
}

/// Hydrate inbound `mxc://` attachments into inline [`Attachment::data`].
///
/// The generic SSRF-guarded fetch path ([`crate::security::ssrf::safe_fetch`])
/// fetches over HTTP, which cannot resolve `mxc://` URIs — those require the
/// Matrix client and its access token. So the Matrix limb downloads its own
/// media here and hands bytes downstream as inline data.
///
/// Each attachment carrying an `mxc://` URL has its `url` cleared (the pipeline
/// cannot use it) and, when the download succeeds within `max_bytes`, its
/// `data` populated. Oversized or failed downloads degrade to metadata-only —
/// matching the Slack limb's behaviour — so nothing re-fetches an unresolvable
/// `mxc://`. Attachments that already carry bytes, or whose URL is not `mxc://`,
/// pass through untouched.
pub async fn hydrate_inbound_attachments(
    client: &Client,
    attachments: Vec<Attachment>,
    max_bytes: u64,
) -> Vec<Attachment> {
    let mut out = Vec::with_capacity(attachments.len());
    for mut att in attachments {
        if !needs_hydration(&att) {
            out.push(att);
            continue;
        }

        let mxc = match att.url.take() {
            Some(url) => url,
            None => {
                tracing::warn!("Attachment marked for hydration but URL is absent; passing through metadata-only");
                out.push(att);
                continue;
            }
        };
        if !within_cap(att.size, max_bytes) {
            tracing::debug!(
                "Matrix inbound media {} exceeds cap ({:?} > {} bytes), metadata only",
                mxc,
                att.size,
                max_bytes
            );
            out.push(att);
            continue;
        }

        match tokio::time::timeout(INBOUND_MEDIA_TIMEOUT, download_media(client, &mxc)).await {
            Ok(Ok((bytes, _mime))) if bytes.len() as u64 <= max_bytes => {
                att.size = Some(bytes.len() as u64);
                att.data = Some(bytes);
            }
            Ok(Ok((bytes, _mime))) => {
                tracing::warn!(
                    "Matrix inbound media {} body {} bytes exceeds cap {} bytes, dropping bytes",
                    mxc,
                    bytes.len(),
                    max_bytes
                );
            }
            Ok(Err(e)) => tracing::warn!("Matrix inbound media {} download failed: {e}", mxc),
            Err(_elapsed) => {
                tracing::warn!("Matrix inbound media {} download timed out", mxc)
            }
        }
        out.push(att);
    }
    out
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn att(url: Option<&str>, data: Option<Vec<u8>>, size: Option<u64>) -> Attachment {
        Attachment {
            id: "id".to_string(),
            mime_type: "image/png".to_string(),
            filename: Some("f.png".to_string()),
            size,
            url: url.map(String::from),
            path: None,
            data,
        }
    }

    #[test]
    fn within_cap_treats_absent_size_as_ok() {
        assert!(within_cap(None, 10));
        assert!(within_cap(Some(10), 10));
        assert!(!within_cap(Some(11), 10));
    }

    #[test]
    fn needs_hydration_only_for_undownloaded_mxc() {
        assert!(needs_hydration(&att(Some("mxc://h/abc"), None, Some(1))));
        // already has bytes
        assert!(!needs_hydration(&att(
            Some("mxc://h/abc"),
            Some(vec![1]),
            Some(1)
        )));
        // http url belongs to the generic pipeline
        assert!(!needs_hydration(&att(Some("https://h/abc"), None, Some(1))));
        // no url
        assert!(!needs_hydration(&att(None, None, Some(1))));
    }
}
