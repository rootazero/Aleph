//! Video transcript data structures and formatting utilities

use serde::{Deserialize, Serialize};

/// A single segment of a video transcript with timing information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    /// Start time in seconds from the beginning of the video
    pub start_seconds: f64,
    /// Duration of this segment in seconds
    pub duration_seconds: f64,
    /// The transcript text for this segment
    pub text: String,
}

impl TranscriptSegment {
    /// Create a new transcript segment
    pub fn new(start_seconds: f64, duration_seconds: f64, text: String) -> Self {
        Self {
            start_seconds,
            duration_seconds,
            text,
        }
    }

    /// Get the end time of this segment
    pub fn end_seconds(&self) -> f64 {
        self.start_seconds + self.duration_seconds
    }
}

/// A complete video transcript with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoTranscript {
    /// The video ID (e.g., YouTube video ID)
    pub video_id: String,
    /// The video title
    pub title: String,
    /// The language of the transcript (ISO 639-1 code)
    pub language: String,
    /// All transcript segments in chronological order
    pub segments: Vec<TranscriptSegment>,
    /// Total duration of the video in seconds
    pub total_duration_seconds: f64,
    /// Whether the transcript was truncated due to length limits
    pub was_truncated: bool,
}

impl VideoTranscript {
    /// Create a new video transcript
    pub fn new(
        video_id: String,
        title: String,
        language: String,
        segments: Vec<TranscriptSegment>,
    ) -> Self {
        let total_duration_seconds = segments
            .last()
            .map(|s| s.start_seconds + s.duration_seconds)
            .unwrap_or(0.0);

        Self {
            video_id,
            title,
            language,
            segments,
            total_duration_seconds,
            was_truncated: false,
        }
    }

    /// Get the total character count of the transcript
    pub fn total_chars(&self) -> usize {
        self.segments.iter().map(|s| s.text.len()).sum()
    }

    /// Format the transcript for AI context injection
    ///
    /// Returns a formatted string with metadata header and timestamped segments
    pub fn format_for_context(&self) -> String {
        let mut output = String::new();

        // Header with metadata
        output.push_str(&format!("## Video Transcript: {}\n", self.title));
        output.push_str(&format!(
            "- Duration: {}\n",
            Self::format_duration(self.total_duration_seconds)
        ));
        output.push_str(&format!("- Language: {}\n", self.language));
        if self.was_truncated {
            output.push_str("- Note: Transcript was truncated due to length limits\n");
        }
        output.push('\n');

        // Transcript with timestamps
        for segment in &self.segments {
            let timestamp = Self::format_timestamp(segment.start_seconds);
            output.push_str(&format!("[{}] {}\n", timestamp, segment.text));
        }

        output
    }

    /// Format seconds as MM:SS timestamp
    pub fn format_timestamp(seconds: f64) -> String {
        let total_secs = seconds as u32;
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        format!("{:02}:{:02}", mins, secs)
    }

    /// Format seconds as human-readable duration (HH:MM:SS or MM:SS)
    pub fn format_duration(seconds: f64) -> String {
        let total_secs = seconds as u32;
        let hours = total_secs / 3600;
        let mins = (total_secs % 3600) / 60;
        let secs = total_secs % 60;

        if hours > 0 {
            format!("{}:{:02}:{:02}", hours, mins, secs)
        } else {
            format!("{}:{:02}", mins, secs)
        }
    }

    /// Truncate transcript to fit within character limit
    ///
    /// Truncates at segment boundaries to avoid cutting mid-sentence
    pub fn truncate_to_chars(&mut self, max_chars: usize) {
        let mut total_chars = 0;
        let mut keep_count = 0;

        for segment in &self.segments {
            total_chars += segment.text.len();
            if total_chars > max_chars {
                self.was_truncated = true;
                break;
            }
            keep_count += 1;
        }

        if self.was_truncated {
            self.segments.truncate(keep_count);
            // Update total duration based on remaining segments
            self.total_duration_seconds = self
                .segments
                .last()
                .map(|s| s.start_seconds + s.duration_seconds)
                .unwrap_or(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transcript_segment_creation() {
        let segment = TranscriptSegment::new(10.5, 3.2, "Hello world".to_string());
        assert_eq!(segment.start_seconds, 10.5);
        assert_eq!(segment.duration_seconds, 3.2);
        assert_eq!(segment.text, "Hello world");
        assert!((segment.end_seconds() - 13.7).abs() < 0.001);
    }

    #[test]
    fn test_video_transcript_creation() {
        let segments = vec![
            TranscriptSegment::new(0.0, 2.5, "Hello".to_string()),
            TranscriptSegment::new(2.5, 3.0, "World".to_string()),
        ];
        let transcript = VideoTranscript::new(
            "abc123".to_string(),
            "Test Video".to_string(),
            "en".to_string(),
            segments,
        );

        assert_eq!(transcript.video_id, "abc123");
        assert_eq!(transcript.title, "Test Video");
        assert_eq!(transcript.language, "en");
        assert_eq!(transcript.segments.len(), 2);
        assert!((transcript.total_duration_seconds - 5.5).abs() < 0.001);
        assert!(!transcript.was_truncated);
    }

    #[test]
    fn test_total_chars() {
        let segments = vec![
            TranscriptSegment::new(0.0, 2.5, "Hello".to_string()), // 5 chars
            TranscriptSegment::new(2.5, 3.0, "World".to_string()), // 5 chars
            TranscriptSegment::new(5.5, 2.0, "Test".to_string()),  // 4 chars
        ];
        let transcript = VideoTranscript::new(
            "id".to_string(),
            "Title".to_string(),
            "en".to_string(),
            segments,
        );

        assert_eq!(transcript.total_chars(), 14);
    }

    #[test]
    fn test_format_timestamp() {
        assert_eq!(VideoTranscript::format_timestamp(0.0), "00:00");
        assert_eq!(VideoTranscript::format_timestamp(65.0), "01:05");
        assert_eq!(VideoTranscript::format_timestamp(3661.0), "61:01");
        assert_eq!(VideoTranscript::format_timestamp(59.9), "00:59");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(VideoTranscript::format_duration(0.0), "0:00");
        assert_eq!(VideoTranscript::format_duration(65.0), "1:05");
        assert_eq!(VideoTranscript::format_duration(3661.0), "1:01:01");
        assert_eq!(VideoTranscript::format_duration(7325.0), "2:02:05");
    }

    #[test]
    fn test_format_for_context() {
        let segments = vec![
            TranscriptSegment::new(0.0, 2.5, "Hello everyone".to_string()),
            TranscriptSegment::new(2.5, 3.0, "Welcome to the video".to_string()),
        ];
        let transcript = VideoTranscript::new(
            "abc123".to_string(),
            "My Test Video".to_string(),
            "en".to_string(),
            segments,
        );

        let formatted = transcript.format_for_context();
        assert!(formatted.contains("## Video Transcript: My Test Video"));
        assert!(formatted.contains("- Duration: 0:05"));
        assert!(formatted.contains("- Language: en"));
        assert!(formatted.contains("[00:00] Hello everyone"));
        assert!(formatted.contains("[00:02] Welcome to the video"));
    }

    #[test]
    fn test_truncate_to_chars() {
        let segments = vec![
            TranscriptSegment::new(0.0, 2.0, "12345".to_string()), // 5 chars
            TranscriptSegment::new(2.0, 2.0, "67890".to_string()), // 5 chars
            TranscriptSegment::new(4.0, 2.0, "ABCDE".to_string()), // 5 chars
        ];
        let mut transcript = VideoTranscript::new(
            "id".to_string(),
            "Title".to_string(),
            "en".to_string(),
            segments,
        );

        // Truncate to 12 chars - should keep first 2 segments (10 chars)
        transcript.truncate_to_chars(12);

        assert_eq!(transcript.segments.len(), 2);
        assert!(transcript.was_truncated);
        assert!((transcript.total_duration_seconds - 4.0).abs() < 0.001);
    }

    #[test]
    fn test_truncate_no_change_when_under_limit() {
        let segments = vec![TranscriptSegment::new(0.0, 2.0, "Hello".to_string())];
        let mut transcript = VideoTranscript::new(
            "id".to_string(),
            "Title".to_string(),
            "en".to_string(),
            segments,
        );

        transcript.truncate_to_chars(100);

        assert_eq!(transcript.segments.len(), 1);
        assert!(!transcript.was_truncated);
    }

    #[test]
    fn test_format_for_context_truncated() {
        let segments = vec![TranscriptSegment::new(0.0, 2.0, "Hello".to_string())];
        let mut transcript = VideoTranscript::new(
            "id".to_string(),
            "Title".to_string(),
            "en".to_string(),
            segments,
        );
        transcript.was_truncated = true;

        let formatted = transcript.format_for_context();
        assert!(formatted.contains("Note: Transcript was truncated"));
    }
}
