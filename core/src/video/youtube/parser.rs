//! Transcript parsing utilities for various formats (XML, JSON3, VTT)

use crate::error::{AlephError, Result};
use crate::video::transcript::TranscriptSegment;
use regex::Regex;

/// Parse transcript data (XML, JSON3, or VTT format)
pub fn parse_transcript_data(data: &str) -> Result<Vec<TranscriptSegment>> {
    let trimmed = data.trim();

    // YouTube transcripts come in XML format
    if trimmed.starts_with("<?xml") || data.contains("<transcript>") {
        parse_transcript_xml(data)
    } else if trimmed.starts_with('{') {
        parse_transcript_json(data)
    } else if trimmed.starts_with("WEBVTT") {
        parse_transcript_vtt(data)
    } else {
        Err(AlephError::video("Unknown transcript format"))
    }
}

/// Parse WebVTT format transcript (from yt-dlp)
pub fn parse_transcript_vtt(vtt: &str) -> Result<Vec<TranscriptSegment>> {
    let mut segments = Vec::new();
    let mut current_text = String::new();
    let mut current_start = 0.0;
    let mut current_end = 0.0;

    // VTT timestamp regex: 00:00:00.000 --> 00:00:00.000
    let timestamp_regex = Regex::new(
        r"(\d{2}):(\d{2}):(\d{2})\.(\d{3})\s*-->\s*(\d{2}):(\d{2}):(\d{2})\.(\d{3})",
    )
    .map_err(|e| AlephError::video(format!("Invalid VTT regex: {}", e)))?;

    // Tag removal regex for VTT formatting tags
    let tag_regex = Regex::new(r"<[^>]+>")
        .map_err(|e| AlephError::video(format!("Invalid tag regex: {}", e)))?;

    let lines: Vec<&str> = vtt.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();

        // Skip WEBVTT header and metadata
        if line.starts_with("WEBVTT")
            || line.starts_with("Kind:")
            || line.starts_with("Language:")
            || line.is_empty()
        {
            i += 1;
            continue;
        }

        // Check for timestamp line
        if let Some(caps) = timestamp_regex.captures(line) {
            // If we have accumulated text, save the previous segment
            if !current_text.is_empty() {
                let text = current_text.trim().to_string();
                if !text.is_empty() {
                    segments.push(TranscriptSegment::new(
                        current_start,
                        current_end - current_start,
                        text,
                    ));
                }
                current_text.clear();
            }

            // Parse timestamps
            current_start = parse_vtt_timestamp(&caps, 1);
            current_end = parse_vtt_timestamp(&caps, 5);

            i += 1;

            // Collect text lines until empty line or next timestamp
            while i < lines.len() {
                let text_line = lines[i].trim();
                if text_line.is_empty() || timestamp_regex.is_match(text_line) {
                    break;
                }

                // Remove VTT tags like <c> and timing info
                let clean_text = tag_regex.replace_all(text_line, "").to_string();
                let clean_text = clean_text.trim();

                if !clean_text.is_empty() {
                    if !current_text.is_empty() {
                        current_text.push(' ');
                    }
                    current_text.push_str(clean_text);
                }
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    // Don't forget the last segment
    if !current_text.is_empty() {
        let text = current_text.trim().to_string();
        if !text.is_empty() {
            segments.push(TranscriptSegment::new(
                current_start,
                current_end - current_start,
                text,
            ));
        }
    }

    // Deduplicate consecutive segments with same or similar text
    // YouTube VTT often has duplicates due to styling
    let mut deduped: Vec<TranscriptSegment> = Vec::new();
    for seg in segments {
        if let Some(last) = deduped.last() {
            // Skip if text is same or very similar to last segment
            if last.text != seg.text {
                deduped.push(seg);
            }
        } else {
            deduped.push(seg);
        }
    }

    if deduped.is_empty() {
        return Err(AlephError::video("No transcript segments found in VTT"));
    }

    Ok(deduped)
}

/// Parse VTT timestamp components to seconds
fn parse_vtt_timestamp(caps: &regex::Captures, start_group: usize) -> f64 {
    let hours: f64 = caps
        .get(start_group)
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0.0);
    let minutes: f64 = caps
        .get(start_group + 1)
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0.0);
    let seconds: f64 = caps
        .get(start_group + 2)
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0.0);
    let millis: f64 = caps
        .get(start_group + 3)
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0.0);

    hours * 3600.0 + minutes * 60.0 + seconds + millis / 1000.0
}

/// Parse YouTube transcript XML format
pub fn parse_transcript_xml(xml: &str) -> Result<Vec<TranscriptSegment>> {
    let mut segments = Vec::new();

    // Simple XML parsing for YouTube transcript format:
    // <text start="0.0" dur="2.5">Hello everyone</text>
    let text_regex =
        Regex::new(r#"<text\s+start="([^"]+)"\s+dur="([^"]+)"[^>]*>([^<]*)</text>"#)
            .map_err(|e| AlephError::video(format!("Invalid regex: {}", e)))?;

    for caps in text_regex.captures_iter(xml) {
        let start: f64 = caps
            .get(1)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0.0);

        let dur: f64 = caps
            .get(2)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0.0);

        let text = caps
            .get(3)
            .map(|m| decode_html_entities(m.as_str()))
            .unwrap_or_default();

        if !text.is_empty() {
            segments.push(TranscriptSegment::new(start, dur, text));
        }
    }

    if segments.is_empty() {
        return Err(AlephError::video("No transcript segments found in XML"));
    }

    Ok(segments)
}

/// Parse YouTube transcript JSON3 format (alternative format)
pub fn parse_transcript_json(json: &str) -> Result<Vec<TranscriptSegment>> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| AlephError::video(format!("Failed to parse transcript JSON: {}", e)))?;

    let events = value
        .get("events")
        .and_then(|e| e.as_array())
        .ok_or_else(|| AlephError::video("No events in transcript JSON"))?;

    let mut segments = Vec::new();

    for event in events {
        // Skip events without segments (like style events)
        let segs = match event.get("segs").and_then(|s| s.as_array()) {
            Some(s) => s,
            None => continue,
        };

        let start_ms = event.get("tStartMs").and_then(|t| t.as_i64()).unwrap_or(0);

        let dur_ms = event
            .get("dDurationMs")
            .and_then(|d| d.as_i64())
            .unwrap_or(0);

        let text: String = segs
            .iter()
            .filter_map(|seg| seg.get("utf8").and_then(|u| u.as_str()))
            .collect::<Vec<_>>()
            .join("");

        let text = text.trim().to_string();
        if !text.is_empty() && text != "\n" {
            segments.push(TranscriptSegment::new(
                start_ms as f64 / 1000.0,
                dur_ms as f64 / 1000.0,
                text,
            ));
        }
    }

    if segments.is_empty() {
        return Err(AlephError::video("No transcript segments found in JSON"));
    }

    Ok(segments)
}

/// Decode HTML entities in transcript text
pub fn decode_html_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
        .replace("\n", " ")
        .trim()
        .to_string()
}

/// Extract a JSON object from the beginning of a string
///
/// This uses a more robust approach that handles all escape sequences correctly,
/// including Unicode escapes (\uXXXX) and escaped quotes (\").
pub fn extract_json_object(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes[0] != b'{' {
        return None;
    }

    let mut depth = 0;
    let mut in_string = false;
    let mut i = 0;

    while i < bytes.len() {
        let c = bytes[i];

        if in_string {
            if c == b'\\' && i + 1 < bytes.len() {
                // Skip the next character (handles \", \\, \n, \uXXXX, etc.)
                i += 2;
                continue;
            } else if c == b'"' {
                in_string = false;
            }
        } else {
            match c {
                b'"' => in_string = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(s[..=i].to_string());
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }

    None
}
