use crate::error::{AlephError, Result};
use futures::stream::BoxStream;
use futures::TryStreamExt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseLine {
    Data(String),
    Done,
    Comment(String),
    Empty,
}

/// Maximum bytes the shared SSE line buffer is allowed to accumulate before
/// a newline is observed. A misbehaving provider that omits newlines (or a
/// malicious one that withholds them) would otherwise grow this buffer
/// without bound until the per-turn watchdog fires — too late to fail over
/// cleanly. 1 MiB is well above any legitimate single SSE event.
pub(crate) const MAX_SSE_LINE_BYTES: usize = 1024 * 1024;

pub fn parse_sse_line(line: &str) -> Result<Option<SseLine>> {
    let line = line.trim_end();

    if line.is_empty() {
        return Ok(Some(SseLine::Empty));
    }

    if let Some(comment) = line.strip_prefix(':') {
        return Ok(Some(SseLine::Comment(comment.to_string())));
    }

    if let Some(data) = line
        .strip_prefix("data: ")
        .or_else(|| line.strip_prefix("data:"))
    {
        let data = data.trim_start();
        if data == "[DONE]" {
            return Ok(Some(SseLine::Done));
        }
        return Ok(Some(SseLine::Data(data.to_string())));
    }

    Ok(None)
}

#[must_use]
pub fn sse_event_stream(response: reqwest::Response) -> BoxStream<'static, Result<SseLine>> {
    let buf =
        crate::sync_primitives::Arc::new(crate::sync_primitives::Mutex::new(Vec::<u8>::new()));

    let stream = response
        .bytes_stream()
        .map_err(|e| AlephError::network(format!("Stream error: {e}")))
        .try_filter_map(move |chunk| {
            let buf = buf.clone();
            async move {
                let mut buf_guard = buf.lock().unwrap_or_else(|e| e.into_inner());
                buf_guard.extend_from_slice(&chunk);

                // Bound the buffer: a provider that withholds newlines (or
                // hands us a deliberately unbounded payload) must not let
                // `buf_guard` grow without limit. Surface a typed network
                // error so the failover walk promotes it to a retry.
                if buf_guard.len() > MAX_SSE_LINE_BYTES {
                    return Err(AlephError::network(format!(
                        "SSE line buffer exceeded {} bytes without a newline; \
                         provider is sending malformed or hostile stream",
                        MAX_SSE_LINE_BYTES
                    )));
                }

                let mut events = Vec::new();

                while let Some(newline_pos) = buf_guard.iter().position(|&b| b == b'\n') {
                    let line_bytes = buf_guard[..newline_pos].to_vec();
                    buf_guard.drain(..=newline_pos);

                    let line = String::from_utf8(line_bytes).map_err(|e| {
                        AlephError::network(format!(
                            "Invalid UTF-8 in SSE stream: {}",
                            String::from_utf8_lossy(e.as_bytes())
                        ))
                    })?;

                    if let Some(event) = parse_sse_line(&line)? {
                        events.push(event);
                    }
                }

                if events.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(events))
                }
            }
        })
        .map_ok(|events| futures::stream::iter(events.into_iter().map(Ok)))
        .try_flatten();

    Box::pin(stream)
}

#[must_use]
pub fn sse_line_stream(response: reqwest::Response) -> BoxStream<'static, Result<String>> {
    let stream = sse_event_stream(response).try_filter_map(|event| async move {
        match event {
            SseLine::Data(data) => Ok(Some(data)),
            _ => Ok(None),
        }
    });

    Box::pin(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_data_line() {
        let result = parse_sse_line("data: {\"key\": \"value\"}").unwrap();
        assert_eq!(
            result,
            Some(SseLine::Data("{\"key\": \"value\"}".to_string()))
        );
    }

    #[test]
    fn test_parse_done_line() {
        let result = parse_sse_line("data: [DONE]").unwrap();
        assert_eq!(result, Some(SseLine::Done));
    }

    #[test]
    fn test_parse_comment_line() {
        let result = parse_sse_line(": heartbeat").unwrap();
        assert_eq!(result, Some(SseLine::Comment(" heartbeat".to_string())));
    }

    #[test]
    fn test_parse_empty_line() {
        let result = parse_sse_line("").unwrap();
        assert_eq!(result, Some(SseLine::Empty));
    }

    #[test]
    fn test_parse_no_space_after_colon() {
        let result = parse_sse_line("data:{\"key\": \"value\"}").unwrap();
        assert_eq!(
            result,
            Some(SseLine::Data("{\"key\": \"value\"}".to_string()))
        );
    }

    #[test]
    fn test_parse_unknown_line_ignored() {
        let result = parse_sse_line("event: message").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_done_with_whitespace() {
        let result = parse_sse_line("data:  [DONE]  ").unwrap();
        assert_eq!(result, Some(SseLine::Done));
    }
}
