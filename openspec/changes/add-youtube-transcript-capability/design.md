# Design: YouTube Transcript Capability

## Context

Aleph's capability system currently supports Memory (local vector search) and Search (web search). Users want to analyze YouTube video content directly through the hotkey workflow. This design adds a new `Video` capability that extracts transcripts from YouTube videos.

**Stakeholders:**
- End users: Want frictionless video content analysis
- AI providers: Already support long-context text (transcripts work well)
- Rust core: Needs new capability module
- Swift UI: No changes needed (capability is invisible to UI)

**Constraints:**
- Must fit existing capability execution pipeline
- Must not require video download (bandwidth/storage concerns)
- Must handle missing captions gracefully
- Must respect configurable transcript length limits

## YouTube Transcript Extraction Methods

YouTube provides transcripts through a publicly accessible API. The extraction process involves:

### Method 1: Video Page Parsing (Recommended)

1. Fetch YouTube video page HTML
2. Extract `ytInitialPlayerResponse` JSON from `<script>` tags
3. Parse `captionTracks` array to find available languages
4. Fetch transcript XML/JSON from caption URL
5. Parse and format transcript text

```
GET https://www.youtube.com/watch?v=VIDEO_ID
    ↓
Parse HTML → Extract ytInitialPlayerResponse JSON
    ↓
Find captionTracks[].baseUrl
    ↓
GET baseUrl (transcript XML)
    ↓
Parse XML → Extract text + timestamps
```

### Transcript Data Structure

YouTube returns transcript in XML format:

```xml
<?xml version="1.0" encoding="utf-8" ?>
<transcript>
  <text start="0.0" dur="2.5">Hello everyone</text>
  <text start="2.5" dur="3.1">Welcome to this video</text>
  ...
</transcript>
```

Parsed into:

```rust
struct TranscriptSegment {
    start_seconds: f64,
    duration_seconds: f64,
    text: String,
}

struct VideoTranscript {
    video_id: String,
    title: String,
    language: String,
    segments: Vec<TranscriptSegment>,
    total_duration_seconds: f64,
}
```

## Architecture

### Module Structure

```
Aleph/core/src/
├── video/
│   ├── mod.rs              # Module exports, VideoCapability trait
│   ├── youtube.rs          # YouTube-specific extraction logic
│   ├── transcript.rs       # Transcript formatting utilities
│   └── error.rs            # Video-specific error types
├── payload/
│   ├── capability.rs       # Add Video = 3 variant (MODIFIED)
│   └── mod.rs              # Add video_transcript field (MODIFIED)
├── capability/
│   └── mod.rs              # Add execute_video method (MODIFIED)
└── config/
    └── mod.rs              # Add VideoConfig section (MODIFIED)
```

### Capability Integration

```rust
// payload/capability.rs - MODIFIED
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Capability {
    Memory = 0,
    Search = 1,
    Mcp = 2,
    Video = 3,  // NEW
}

// payload/mod.rs - MODIFIED
pub struct AgentContext {
    pub memory_snippets: Option<Vec<MemoryEntry>>,
    pub search_results: Option<Vec<SearchResult>>,
    pub mcp_resources: Option<HashMap<String, serde_json::Value>>,
    pub workflow_state: Option<WorkflowState>,
    pub attachments: Option<Vec<MediaAttachment>>,
    pub video_transcript: Option<VideoTranscript>,  // NEW
}
```

### CapabilityExecutor Extension

```rust
// capability/mod.rs - MODIFIED
impl CapabilityExecutor {
    async fn execute_capability(&self, mut payload: AgentPayload, capability: Capability) -> Result<AgentPayload> {
        match capability {
            Capability::Memory => { /* existing */ }
            Capability::Search => { /* existing */ }
            Capability::Mcp => { /* existing */ }
            Capability::Video => {
                payload = self.execute_video(payload).await?;
            }
        }
        Ok(payload)
    }

    async fn execute_video(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        // 1. Check if video capability is configured
        let Some(config) = &self.video_config else {
            warn!("Video capability requested but not configured");
            return Ok(payload);
        };

        // 2. Extract YouTube URL from user input
        let Some(video_url) = extract_youtube_url(&payload.user_input) else {
            debug!("No YouTube URL found in input");
            return Ok(payload);
        };

        // 3. Fetch transcript
        let youtube = YouTubeExtractor::new(config.clone());
        match youtube.extract_transcript(&video_url).await {
            Ok(transcript) => {
                info!(
                    video_id = %transcript.video_id,
                    segments = transcript.segments.len(),
                    "Extracted YouTube transcript"
                );
                payload.context.video_transcript = Some(transcript);
            }
            Err(e) => {
                warn!(error = %e, "Failed to extract transcript");
                // Continue without transcript - don't fail the request
            }
        }

        Ok(payload)
    }
}
```

### YouTubeExtractor Implementation

```rust
// video/youtube.rs
pub struct YouTubeExtractor {
    client: reqwest::Client,
    config: VideoConfig,
}

impl YouTubeExtractor {
    pub fn new(config: VideoConfig) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (compatible; Aleph/1.0)")
            .timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to create HTTP client");

        Self { client, config }
    }

    pub async fn extract_transcript(&self, url: &str) -> Result<VideoTranscript> {
        // 1. Parse video ID from URL
        let video_id = Self::parse_video_id(url)?;

        // 2. Fetch video page
        let page_url = format!("https://www.youtube.com/watch?v={}", video_id);
        let html = self.client.get(&page_url).send().await?.text().await?;

        // 3. Extract player response JSON
        let player_response = Self::extract_player_response(&html)?;

        // 4. Find caption track URL
        let caption_url = Self::find_caption_url(&player_response, &self.config.preferred_language)?;

        // 5. Fetch and parse transcript
        let transcript_xml = self.client.get(&caption_url).send().await?.text().await?;
        let segments = Self::parse_transcript_xml(&transcript_xml)?;

        // 6. Truncate if needed
        let segments = Self::truncate_segments(segments, self.config.max_transcript_length);

        Ok(VideoTranscript {
            video_id: video_id.to_string(),
            title: Self::extract_title(&player_response),
            language: self.config.preferred_language.clone(),
            total_duration_seconds: segments.last().map(|s| s.start_seconds + s.duration_seconds).unwrap_or(0.0),
            segments,
        })
    }

    fn parse_video_id(url: &str) -> Result<&str> {
        // Match patterns:
        // - youtube.com/watch?v=VIDEO_ID
        // - youtu.be/VIDEO_ID
        // - youtube.com/embed/VIDEO_ID
        lazy_static! {
            static ref YOUTUBE_REGEX: Regex = Regex::new(
                r"(?:youtube\.com/watch\?v=|youtu\.be/|youtube\.com/embed/)([a-zA-Z0-9_-]{11})"
            ).unwrap();
        }

        YOUTUBE_REGEX.captures(url)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| AlephError::video("Invalid YouTube URL"))
    }

    fn extract_player_response(html: &str) -> Result<serde_json::Value> {
        // Find ytInitialPlayerResponse in script tags
        let start_marker = "var ytInitialPlayerResponse = ";
        let start = html.find(start_marker)
            .ok_or_else(|| AlephError::video("Player response not found"))?;

        let json_start = start + start_marker.len();
        let json_end = html[json_start..].find(";</script>")
            .ok_or_else(|| AlephError::video("Failed to parse player response"))?;

        let json_str = &html[json_start..json_start + json_end];
        serde_json::from_str(json_str)
            .map_err(|e| AlephError::video(format!("Failed to parse JSON: {}", e)))
    }

    fn find_caption_url(player_response: &serde_json::Value, preferred_lang: &str) -> Result<String> {
        let caption_tracks = player_response
            .get("captions")
            .and_then(|c| c.get("playerCaptionsTracklistRenderer"))
            .and_then(|r| r.get("captionTracks"))
            .and_then(|t| t.as_array())
            .ok_or_else(|| AlephError::video("No captions available for this video"))?;

        // Try preferred language first, then fall back to first available
        let track = caption_tracks.iter()
            .find(|t| t.get("languageCode").and_then(|l| l.as_str()) == Some(preferred_lang))
            .or_else(|| caption_tracks.first())
            .ok_or_else(|| AlephError::video("No caption tracks found"))?;

        track.get("baseUrl")
            .and_then(|u| u.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| AlephError::video("Caption URL not found"))
    }

    fn parse_transcript_xml(xml: &str) -> Result<Vec<TranscriptSegment>> {
        // Parse YouTube transcript XML format
        let mut segments = Vec::new();

        // Simple XML parsing (or use quick-xml crate)
        for line in xml.lines() {
            if line.contains("<text ") {
                // Extract start, dur, and text content
                // ...
            }
        }

        Ok(segments)
    }

    fn truncate_segments(segments: Vec<TranscriptSegment>, max_chars: usize) -> Vec<TranscriptSegment> {
        let mut total_chars = 0;
        let mut result = Vec::new();

        for segment in segments {
            total_chars += segment.text.len();
            if total_chars > max_chars {
                break;
            }
            result.push(segment);
        }

        result
    }
}
```

### Trigger Mechanism Design

Users can trigger video analysis in two ways:

#### Method 1: Automatic URL Detection (Recommended)

When user input contains a YouTube URL, the Video capability is automatically enabled:

```
User input: "分析这个视频：https://youtube.com/watch?v=xyz"
              ↓
Router: Detects YouTube URL → Adds Video capability automatically
              ↓
CapabilityExecutor: Extracts transcript
              ↓
AI: Analyzes video content
```

**Advantages:**
- Zero learning curve - users just paste URLs naturally
- Works with any existing command (e.g., `/zh 翻译这个视频: https://...`)
- Consistent with how users share video links in chat

#### Method 2: Explicit `/video` Command (Optional)

For users who want explicit control:

```
User input: "/video https://youtube.com/watch?v=xyz"
              ↓
Router: Matches /video rule → Uses video-specific system prompt
              ↓
CapabilityExecutor: Extracts transcript
              ↓
AI: Analyzes with video-optimized prompt
```

**Configuration:**

```toml
# Built-in rule (auto-added, user can override)
[[rules]]
regex = "^/video"
provider = "claude"  # or user's default_provider
system_prompt = "You are a video content analyst. Analyze the following video transcript and provide insights."
capabilities = ["video", "memory"]
intent_type = "video_analysis"
```

### URL Detection Implementation

```rust
// router/mod.rs
impl Router {
    /// Detect capabilities based on user input content
    fn detect_content_capabilities(&self, input: &str) -> Vec<Capability> {
        let mut caps = Vec::new();

        // Automatic YouTube URL detection
        if Self::contains_youtube_url(input) && self.video_config.enabled {
            caps.push(Capability::Video);
        }

        caps
    }

    fn contains_youtube_url(input: &str) -> bool {
        lazy_static! {
            static ref YOUTUBE_REGEX: Regex = Regex::new(
                r"(?:youtube\.com/watch\?v=|youtu\.be/|youtube\.com/embed/)[a-zA-Z0-9_-]{11}"
            ).unwrap();
        }
        YOUTUBE_REGEX.is_match(input)
    }
}
```

### Why Not Reuse `/search`?

We considered using `/search` to trigger video analysis but decided against it:

| Aspect | /search | /video (or auto-detect) |
|--------|---------|-------------------------|
| Semantic | Web search | Video content extraction |
| Data source | Search APIs (Tavily, Google) | YouTube transcript API |
| Cost | May incur API costs | Free |
| Response type | Multiple results + snippets | Single transcript |
| Use case | "Find information about X" | "Analyze this specific video" |

Using `/search` would be confusing because:
1. Users expect search to return multiple web results
2. Video transcript is not "searching" - it's content extraction
3. Different system prompts are optimal for each use case

### Transcript Formatting for AI

```rust
// video/transcript.rs
impl VideoTranscript {
    /// Format transcript for AI consumption with timestamps
    pub fn format_for_context(&self) -> String {
        let mut output = String::new();

        // Header with metadata
        output.push_str(&format!("## Video Transcript: {}\n", self.title));
        output.push_str(&format!("- Duration: {}\n", Self::format_duration(self.total_duration_seconds)));
        output.push_str(&format!("- Language: {}\n\n", self.language));

        // Transcript with timestamps
        for segment in &self.segments {
            let timestamp = Self::format_timestamp(segment.start_seconds);
            output.push_str(&format!("[{}] {}\n", timestamp, segment.text));
        }

        output
    }

    fn format_timestamp(seconds: f64) -> String {
        let mins = (seconds / 60.0) as u32;
        let secs = (seconds % 60.0) as u32;
        format!("{:02}:{:02}", mins, secs)
    }

    fn format_duration(seconds: f64) -> String {
        let hours = (seconds / 3600.0) as u32;
        let mins = ((seconds % 3600.0) / 60.0) as u32;
        let secs = (seconds % 60.0) as u32;

        if hours > 0 {
            format!("{}:{:02}:{:02}", hours, mins, secs)
        } else {
            format!("{}:{:02}", mins, secs)
        }
    }
}
```

### PromptAssembler Integration

```rust
// payload/assembler.rs - MODIFIED
impl PromptAssembler {
    pub fn assemble_context(&self, payload: &AgentPayload) -> String {
        let mut parts = Vec::new();

        // Existing: Memory snippets
        if let Some(memories) = &payload.context.memory_snippets {
            parts.push(self.format_memory(memories));
        }

        // Existing: Search results
        if let Some(results) = &payload.context.search_results {
            parts.push(self.format_search(results));
        }

        // NEW: Video transcript
        if let Some(transcript) = &payload.context.video_transcript {
            parts.push(transcript.format_for_context());
        }

        parts.join("\n\n---\n\n")
    }
}
```

## Configuration

```rust
// config/mod.rs - MODIFIED
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoConfig {
    /// Enable video transcript extraction
    #[serde(default = "default_video_enabled")]
    pub enabled: bool,

    /// Enable YouTube transcript extraction
    #[serde(default = "default_youtube_transcript")]
    pub youtube_transcript: bool,

    /// Preferred language for transcripts (ISO 639-1 code)
    #[serde(default = "default_preferred_language")]
    pub preferred_language: String,

    /// Maximum transcript length in characters
    #[serde(default = "default_max_transcript_length")]
    pub max_transcript_length: usize,
}

fn default_video_enabled() -> bool { true }
fn default_youtube_transcript() -> bool { true }
fn default_preferred_language() -> String { "en".to_string() }
fn default_max_transcript_length() -> usize { 50000 }

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            enabled: default_video_enabled(),
            youtube_transcript: default_youtube_transcript(),
            preferred_language: default_preferred_language(),
            max_transcript_length: default_max_transcript_length(),
        }
    }
}
```

## Error Handling

```rust
// video/error.rs
#[derive(Debug, thiserror::Error)]
pub enum VideoError {
    #[error("Invalid YouTube URL: {0}")]
    InvalidUrl(String),

    #[error("No captions available for this video")]
    NoCaptions,

    #[error("Failed to fetch video page: {0}")]
    FetchError(#[from] reqwest::Error),

    #[error("Failed to parse transcript: {0}")]
    ParseError(String),

    #[error("Video is private or unavailable")]
    VideoUnavailable,

    #[error("Transcript too long (>{0} chars)")]
    TranscriptTooLong(usize),
}
```

## Decisions

### Decision 1: Capability vs. Extractor Pattern

**What:** Implement as a new `Capability` (Video = 3) rather than as a content extractor.

**Why:**
- Video URL is in user input, not clipboard
- Processing happens after routing, not during content capture
- Fits naturally into capability execution pipeline
- Consistent with Search capability pattern

**Alternatives considered:**
1. Content Extractor: Would need URL in clipboard, breaks workflow
2. Router plugin: Too tightly coupled to routing logic

### Decision 2: No Caching

**What:** Don't cache transcripts locally.

**Why:**
- Transcripts are typically accessed once per session
- Caching adds complexity (expiration, storage management)
- Re-fetching is fast (~1-2s) and free

**Trade-off:** Slightly slower for repeated analysis of same video.

### Decision 3: Graceful Degradation

**What:** When transcript extraction fails, continue without video context rather than failing the entire request.

**Why:**
- User might have YouTube URL + other content
- Better UX to get partial response than error
- Error is logged for debugging

## Risks / Trade-offs

### Risk: YouTube Page Structure Changes

**Issue:** YouTube may change HTML structure, breaking extraction.

**Mitigation:**
- Use multiple extraction patterns
- Log extraction failures for monitoring
- Consider fallback to youtube-transcript-api via Python subprocess (emergency)

### Risk: Rate Limiting

**Issue:** YouTube may rate-limit transcript requests.

**Mitigation:**
- Add retry with exponential backoff
- Use reasonable User-Agent
- Don't batch requests

### Trade-off: No Video Frame Analysis

**Decision:** MVP only extracts text transcript, no visual analysis.

**Rationale:**
- Visual analysis requires video download (slow, storage)
- Transcript covers most use cases (summarization, Q&A, translation)
- Can add visual analysis as future enhancement with Gemini API

## Testing Strategy

### Unit Tests

1. URL parsing: Various YouTube URL formats
2. Transcript XML parsing: Sample transcript data
3. Truncation logic: Respect character limits
4. Timestamp formatting: Edge cases

### Integration Tests

1. Mock YouTube server with sample responses
2. Test capability execution pipeline
3. Test prompt assembly with transcript context

### Manual Testing

1. Test with videos that have manual captions
2. Test with auto-generated captions
3. Test with videos without captions (error handling)
4. Test with very long videos (truncation)
5. Test with non-English videos
