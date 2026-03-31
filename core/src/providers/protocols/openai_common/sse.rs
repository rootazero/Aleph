//! SSE line buffering and stream utilities.

use crate::error::{AlephError, Result};
use futures::stream::BoxStream;
use futures::TryStreamExt;

/// Build a stream of individual SSE data lines from an HTTP response.
///
/// Buffers incomplete lines across chunks, strips "data: " prefix,
/// filters out empty lines, comments, and [DONE] sentinel.
///
/// Uses a byte buffer to handle UTF-8 multi-byte characters that may be
/// split across HTTP chunk boundaries (e.g., Chinese characters).
pub fn sse_line_stream(response: reqwest::Response) -> BoxStream<'static, Result<String>> {
    let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));

    let stream = response
        .bytes_stream()
        .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
        .try_filter_map(move |chunk| {
            let buf = buf.clone();
            async move {
                let mut buf_guard = buf.lock().unwrap_or_else(|e| e.into_inner());
                buf_guard.extend_from_slice(&chunk);

                let mut lines = Vec::new();

                // Process complete lines from buffer.
                // Only process up to the last newline — everything after is
                // a partial line (possibly with incomplete UTF-8 at the end).
                loop {
                    let newline_pos = match buf_guard.iter().position(|&b| b == b'\n') {
                        Some(pos) => pos,
                        None => break,
                    };

                    let line_bytes = buf_guard[..newline_pos].to_vec();
                    buf_guard.drain(..=newline_pos);

                    // Complete lines are guaranteed to be valid UTF-8 in SSE
                    // (JSON text terminated by newline). If somehow invalid,
                    // use lossy conversion to avoid crashing.
                    let line = String::from_utf8(line_bytes)
                        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
                    let line = line.trim_end();

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
