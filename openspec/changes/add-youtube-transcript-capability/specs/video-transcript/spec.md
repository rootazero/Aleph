# Spec: Video Transcript Capability

**Capability:** `video-transcript`
**Version:** 1.0.0

## Overview

This capability enables Aleph to extract transcripts from YouTube videos and inject them into the AI context for analysis. Users can analyze video content by simply pasting a YouTube URL into their input.

---

## ADDED Requirements

### Requirement: YouTube URL Auto-Detection

The system SHALL automatically detect YouTube URLs in user input and enable the Video capability.

**Acceptance Criteria:**
- System recognizes standard YouTube URL formats
- Video capability is automatically added to capability list
- Detection works regardless of surrounding text

#### Scenario: Standard YouTube URL Detection

**Given** user input contains "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
**When** the router processes the input
**Then** the Video capability is automatically enabled
**And** the transcript extraction is triggered

#### Scenario: Short YouTube URL Detection

**Given** user input contains "https://youtu.be/dQw4w9WgXcQ"
**When** the router processes the input
**Then** the Video capability is automatically enabled

#### Scenario: YouTube URL with Timestamp Parameter

**Given** user input contains "https://youtube.com/watch?v=dQw4w9WgXcQ&t=152s"
**When** the router processes the input
**Then** the Video capability is enabled
**And** the video ID is correctly extracted as "dQw4w9WgXcQ"

#### Scenario: Mixed Text and URL Input

**Given** user input is "分析这个视频的内容：https://youtube.com/watch?v=xyz"
**When** the router processes the input
**Then** the Video capability is enabled
**And** the full user input text is preserved for the AI

---

### Requirement: Transcript Extraction

The system SHALL extract video transcripts from YouTube using available captions.

**Acceptance Criteria:**
- System fetches transcript from YouTube's caption API
- Supports both auto-generated and manual captions
- Returns structured transcript with timestamps

#### Scenario: Video with Manual Captions

**Given** a YouTube video has manual captions in English
**When** transcript extraction is requested
**Then** the system returns the manual caption content
**And** each segment includes start time and duration

#### Scenario: Video with Auto-Generated Captions

**Given** a YouTube video only has auto-generated captions
**When** transcript extraction is requested
**Then** the system returns the auto-generated caption content
**And** the transcript is properly formatted

#### Scenario: Video without Captions

**Given** a YouTube video has no captions available
**When** transcript extraction is requested
**Then** the system returns an appropriate error
**And** the request continues without video context
**And** a warning is logged for debugging

#### Scenario: Private or Unavailable Video

**Given** a YouTube video URL points to a private or deleted video
**When** transcript extraction is requested
**Then** the system returns a "video unavailable" error
**And** the request continues gracefully

---

### Requirement: Transcript Length Management

The system SHALL enforce configurable limits on transcript length.

**Acceptance Criteria:**
- Maximum transcript length is configurable
- Long transcripts are truncated with indication
- Truncation preserves complete segments

#### Scenario: Transcript Exceeds Maximum Length

**Given** video transcript is 80,000 characters
**And** max_transcript_length is configured as 50,000
**When** transcript is extracted
**Then** the transcript is truncated to approximately 50,000 characters
**And** truncation occurs at a segment boundary
**And** a truncation notice is included

#### Scenario: Short Transcript Within Limits

**Given** video transcript is 10,000 characters
**And** max_transcript_length is configured as 50,000
**When** transcript is extracted
**Then** the full transcript is returned without truncation

---

### Requirement: Language Preference

The system SHALL respect user's preferred language for transcript selection.

**Acceptance Criteria:**
- Preferred language is configurable
- System tries preferred language first
- Falls back to any available language if preferred unavailable

#### Scenario: Preferred Language Available

**Given** video has captions in English, Spanish, and Japanese
**And** preferred_language is configured as "en"
**When** transcript extraction is requested
**Then** the English transcript is returned

#### Scenario: Preferred Language Unavailable

**Given** video only has captions in Japanese
**And** preferred_language is configured as "en"
**When** transcript extraction is requested
**Then** the Japanese transcript is returned as fallback
**And** the actual language is noted in the transcript metadata

---

### Requirement: Context Integration

The system SHALL integrate video transcripts into the AI context using the established format.

**Acceptance Criteria:**
- Transcript is formatted with timestamps
- Video metadata (title, duration) is included
- Context follows existing PromptAssembler patterns

#### Scenario: Transcript Context Assembly

**Given** a video transcript has been extracted
**When** the PromptAssembler builds the context
**Then** the transcript appears with a header section
**And** includes video title and duration
**And** each line includes timestamp in [MM:SS] format
**And** the transcript section is placed after search results

---

### Requirement: Builtin /video Command

The system SHALL provide a builtin `/video` command for explicit video analysis.

**Acceptance Criteria:**
- `/video` command appears in Settings > Routing preset list
- Command triggers video analysis with optimized system prompt
- Users can use `/video URL` for explicit invocation

#### Scenario: Using /video Command

**Given** user input is "/video https://youtube.com/watch?v=xyz"
**When** the router processes the input
**Then** the /video rule matches
**And** Video capability is enabled
**And** A video-analysis-optimized system prompt is used

---

### Requirement: UI Preset Rule Display

The system SHALL display `/video` in the Settings UI as a builtin preset command.

**Acceptance Criteria:**
- `/video` appears in the "Preset Commands" section of Routing settings
- Shows "Implemented" badge when feature is complete
- Displays usage example and description
- Follows existing PresetRule pattern

#### Scenario: Preset Rule in Settings UI

**Given** user opens Settings > Routing
**When** the Preset Commands section is displayed
**Then** `/video` command is listed with:
  - Icon: "play.rectangle" (or similar video icon)
  - Description: "Analyze YouTube video content via transcript extraction"
  - Usage: "/video <youtube_url>"
  - Status: "Implemented" badge

---

### Requirement: Video Configuration

The system SHALL provide configuration options for video transcript features.

**Acceptance Criteria:**
- Video features can be enabled/disabled
- Preferred language is configurable
- Maximum transcript length is configurable

#### Scenario: Configuration Structure

**Given** user wants to configure video settings
**When** editing config.toml
**Then** the following structure is available:
```toml
[video]
enabled = true
youtube_transcript = true
preferred_language = "en"
max_transcript_length = 50000
```

---

## MODIFIED Requirements

### Requirement: Capability Enum Extension

The Capability enum SHALL be extended to include Video.

**Previous State:**
- Capability enum has: Memory = 0, Search = 1, Mcp = 2

**New State:**
- Capability enum has: Memory = 0, Search = 1, Mcp = 2, Video = 3

#### Scenario: Capability Parsing

**Given** a rule configuration specifies capabilities = ["video", "memory"]
**When** the configuration is parsed
**Then** both Video and Memory capabilities are recognized
**And** capabilities are sorted by priority (Memory, Search, Mcp, Video)

---

### Requirement: AgentContext Extension

The AgentContext struct SHALL include video transcript storage.

**Previous State:**
- AgentContext has: memory_snippets, search_results, mcp_resources, workflow_state, attachments

**New State:**
- AgentContext adds: video_transcript: Option<VideoTranscript>

#### Scenario: Context with Video Transcript

**Given** a request includes a YouTube URL
**When** the CapabilityExecutor processes Video capability
**Then** the extracted transcript is stored in context.video_transcript
**And** the PromptAssembler includes it in the final context

---

### Requirement: CapabilityExecutor Extension

The CapabilityExecutor SHALL handle Video capability execution.

**Previous State:**
- execute_capability matches: Memory, Search, Mcp

**New State:**
- execute_capability matches: Memory, Search, Mcp, Video

#### Scenario: Video Capability Execution Order

**Given** a request has capabilities [Memory, Video, Search]
**When** capabilities are executed
**Then** they execute in order: Memory (0), Search (1), Video (3)
**And** video transcript is available for final context assembly

---

### Requirement: Swift UI Preset Rules Extension

The PresetRules enum in RoutingView.swift SHALL include /video command.

**Previous State:**
- PresetRules.all contains: /search, /mcp, /skill

**New State:**
- PresetRules.all contains: /search, /video, /mcp, /skill

#### Scenario: /video in Preset Commands List

**Given** user views Settings > Routing
**When** the preset commands section loads
**Then** /video is displayed between /search and /mcp
**And** shows the video play icon
**And** displays appropriate description and usage

---

## Cross-References

- **Related Capability:** `ai-routing` - URL detection integrates with routing logic
- **Related Capability:** `clipboard-management` - Video URL may come from clipboard
- **Related Spec:** `add-search-capability-integration` - Similar capability pattern
- **Related Spec:** `add-multimodal-content-support` - Similar context extension pattern

---

## Implementation Notes

### Data Structures

```rust
// video/transcript.rs
pub struct TranscriptSegment {
    pub start_seconds: f64,
    pub duration_seconds: f64,
    pub text: String,
}

pub struct VideoTranscript {
    pub video_id: String,
    pub title: String,
    pub language: String,
    pub segments: Vec<TranscriptSegment>,
    pub total_duration_seconds: f64,
}
```

### Localization Keys Required

```
settings.routing.preset.video.description = "Analyze YouTube video content via transcript extraction"
settings.routing.preset.video.usage = "/video <youtube_url>"
```

### Error Messages

| Error Code | Message | User Action |
|------------|---------|-------------|
| `VIDEO_NO_CAPTIONS` | "This video doesn't have captions available" | Try another video |
| `VIDEO_UNAVAILABLE` | "Video is private or unavailable" | Check video URL |
| `VIDEO_EXTRACT_FAILED` | "Failed to extract transcript" | Retry or report issue |
