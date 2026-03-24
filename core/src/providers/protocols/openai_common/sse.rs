//! SSE line buffering and stream utilities.

use futures::stream::BoxStream;
use futures::TryStreamExt;
use crate::error::{AlephError, Result};

/// Build a stream of individual SSE data lines from an HTTP response.
///
/// Buffers incomplete lines across chunks, strips "data: " prefix,
/// filters out empty lines, comments, and [DONE] sentinel.
pub fn sse_line_stream(response: reqwest::Response) -> BoxStream<'static, Result<String>> {
    let buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));

    let stream = response
        .bytes_stream()
        .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
        .try_filter_map(move |chunk| {
            let buf = buf.clone();
            async move {
                let text = std::str::from_utf8(&chunk)
                    .map_err(|e| AlephError::provider(format!("UTF-8 error: {}", e)))?;
                let mut buf_guard = buf.lock().unwrap_or_else(|e| e.into_inner());
                buf_guard.push_str(text);

                let mut lines = Vec::new();
                while let Some(pos) = buf_guard.find('\n') {
                    let line = buf_guard[..pos].trim_end().to_string();
                    buf_guard.drain(..=pos);
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data != "[DONE]" {
                            lines.push(data.to_string());
                        }
                    }
                }
                if lines.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(lines))
                }
            }
        })
        .map_ok(|lines| futures::stream::iter(lines.into_iter().map(Ok)))
        .try_flatten();

    Box::pin(stream)
}
