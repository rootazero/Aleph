# Tasks: Add YouTube Transcript Capability

## Overview

Total estimated tasks: 25
Dependencies: None (can start immediately)

---

## Phase 1: Core Infrastructure (8 tasks)

### 1.1 Add Video Capability Enum

**File:** `Aleph/core/src/payload/capability.rs`

- [ ] Add `Video = 3` variant to `Capability` enum
- [ ] Update `Capability::parse()` to handle "video" string
- [ ] Update `Capability::as_str()` to return "video"
- [ ] Add unit tests for new capability value

**Validation:** `cargo test capability`

### 1.2 Create Video Module Structure

**Files to create:**
- `Aleph/core/src/video/mod.rs`
- `Aleph/core/src/video/youtube.rs`
- `Aleph/core/src/video/transcript.rs`
- `Aleph/core/src/video/error.rs`

- [ ] Create `video/` directory
- [ ] Create `mod.rs` with module exports
- [ ] Create `error.rs` with `VideoError` enum
- [ ] Add `pub mod video;` to `lib.rs`

**Validation:** `cargo build`

### 1.3 Define Video Data Structures

**File:** `Aleph/core/src/video/transcript.rs`

- [ ] Define `TranscriptSegment` struct (start, duration, text)
- [ ] Define `VideoTranscript` struct (video_id, title, language, segments)
- [ ] Implement `VideoTranscript::format_for_context()` method
- [ ] Implement timestamp formatting helpers
- [ ] Add unit tests for formatting

**Validation:** `cargo test video::transcript`

### 1.4 Add VideoConfig to Configuration

**File:** `Aleph/core/src/config/mod.rs`

- [ ] Define `VideoConfig` struct with fields:
  - `enabled: bool`
  - `youtube_transcript: bool`
  - `preferred_language: String`
  - `max_transcript_length: usize`
- [ ] Add default implementations
- [ ] Add `video: Option<VideoConfig>` to `Config` struct
- [ ] Update `FullConfig` to include video settings

**Validation:** `cargo test config`

### 1.5 Update UniFFI Interface

**File:** `Aleph/core/src/aleph.udl`

- [ ] Add `Video` to Capability enum (if exposed via UniFFI)
- [ ] Add `VideoConfig` dictionary (if needed for Swift UI)

**Validation:** `cargo build && uniffi-bindgen generate`

### 1.6 Extend AgentContext for Video

**File:** `Aleph/core/src/payload/mod.rs`

- [ ] Add `video_transcript: Option<VideoTranscript>` to `AgentContext`
- [ ] Update `AgentContext::default()` implementation

**Validation:** `cargo test payload`

### 1.7 Update PromptAssembler

**File:** `Aleph/core/src/payload/assembler.rs`

- [ ] Add video transcript formatting to context assembly
- [ ] Insert transcript section after search results, before user input
- [ ] Add unit tests for transcript context formatting

**Validation:** `cargo test assembler`

### 1.8 Add VideoError to AlephError

**File:** `Aleph/core/src/error.rs`

- [ ] Add `VideoError` variant to `AlephError` enum
- [ ] Add `AlephError::video(msg)` constructor helper
- [ ] Implement `From<VideoError> for AlephError`

**Validation:** `cargo test error`

---

## Phase 2: YouTube Extraction (7 tasks)

### 2.1 Implement YouTube URL Parser

**File:** `Aleph/core/src/video/youtube.rs`

- [ ] Create `parse_video_id(url: &str) -> Result<String>` function
- [ ] Support URL formats:
  - `youtube.com/watch?v=VIDEO_ID`
  - `youtu.be/VIDEO_ID`
  - `youtube.com/embed/VIDEO_ID`
  - `youtube.com/v/VIDEO_ID`
- [ ] Handle URLs with extra parameters (e.g., `&t=152s`)
- [ ] Add comprehensive unit tests

**Validation:** `cargo test youtube::parse`

### 2.2 Implement Video Page Fetcher

**File:** `Aleph/core/src/video/youtube.rs`

- [ ] Create `YouTubeExtractor` struct with `reqwest::Client`
- [ ] Implement `fetch_video_page(video_id: &str) -> Result<String>`
- [ ] Set appropriate User-Agent header
- [ ] Handle HTTP errors and timeouts

**Validation:** Manual test with real YouTube URL

### 2.3 Implement Player Response Extractor

**File:** `Aleph/core/src/video/youtube.rs`

- [ ] Implement `extract_player_response(html: &str) -> Result<serde_json::Value>`
- [ ] Find and parse `ytInitialPlayerResponse` from script tags
- [ ] Handle various page formats (logged in, logged out)
- [ ] Add unit tests with sample HTML fixtures

**Validation:** `cargo test youtube::player_response`

### 2.4 Implement Caption Track Finder

**File:** `Aleph/core/src/video/youtube.rs`

- [ ] Implement `find_caption_url(response: &Value, lang: &str) -> Result<String>`
- [ ] Extract caption tracks from player response
- [ ] Prioritize user's preferred language
- [ ] Fall back to first available track
- [ ] Return appropriate error when no captions exist

**Validation:** `cargo test youtube::caption_url`

### 2.5 Implement Transcript XML Parser

**File:** `Aleph/core/src/video/youtube.rs`

- [ ] Implement `parse_transcript_xml(xml: &str) -> Result<Vec<TranscriptSegment>>`
- [ ] Parse YouTube transcript XML format
- [ ] Decode HTML entities in transcript text
- [ ] Handle malformed XML gracefully
- [ ] Add unit tests with sample transcript XML

**Validation:** `cargo test youtube::transcript_xml`

### 2.6 Implement Full Extraction Pipeline

**File:** `Aleph/core/src/video/youtube.rs`

- [ ] Implement `YouTubeExtractor::extract_transcript(url: &str) -> Result<VideoTranscript>`
- [ ] Chain all extraction steps
- [ ] Implement transcript truncation based on config
- [ ] Add comprehensive error handling
- [ ] Log extraction metrics (duration, segment count)

**Validation:** Integration test with real YouTube video

### 2.7 Add Retry and Timeout Logic

**File:** `Aleph/core/src/video/youtube.rs`

- [ ] Add configurable timeout (default: 10s)
- [ ] Implement retry with exponential backoff (max 3 attempts)
- [ ] Handle transient network errors gracefully

**Validation:** Test with slow network simulation

---

## Phase 3: Capability Integration (5 tasks)

### 3.1 Add Video to CapabilityExecutor

**File:** `Aleph/core/src/capability/mod.rs`

- [ ] Add `video_config: Option<Arc<VideoConfig>>` field
- [ ] Update constructor to accept video config
- [ ] Add `Capability::Video` arm to `execute_capability` match

**Validation:** `cargo build`

### 3.2 Implement execute_video Method

**File:** `Aleph/core/src/capability/mod.rs`

- [ ] Implement `execute_video(&self, payload: AgentPayload) -> Result<AgentPayload>`
- [ ] Check if video capability is configured
- [ ] Extract YouTube URL from user input
- [ ] Call `YouTubeExtractor::extract_transcript`
- [ ] Store transcript in `payload.context.video_transcript`
- [ ] Handle extraction failures gracefully (warn, don't fail)

**Validation:** `cargo test capability::execute_video`

### 3.3 Add YouTube URL Detection to Router

**File:** `Aleph/core/src/router/mod.rs`

- [ ] Add `detect_content_capabilities(input: &str) -> Vec<Capability>` method
- [ ] Implement YouTube URL regex matching
- [ ] Merge detected capabilities with rule-defined capabilities
- [ ] Ensure Video capability is added when URL is present

**Validation:** `cargo test router::url_detection`

### 3.4 Wire Video Config in AlephCore

**File:** `Aleph/core/src/core.rs`

- [ ] Load video config from TOML
- [ ] Pass video config to CapabilityExecutor
- [ ] Add video config to initialization pipeline

**Validation:** Test with config file containing `[video]` section

### 3.5 Add Integration Tests

**File:** `Aleph/core/src/video/mod.rs` (tests module)

- [ ] Test full pipeline with mock HTTP responses
- [ ] Test error scenarios (no captions, invalid URL, timeout)
- [ ] Test transcript truncation
- [ ] Test capability execution with video context

**Validation:** `cargo test video --features integration`

---

## Phase 4: Configuration & Rules (3 tasks)

### 4.1 Add Default /video Rule

**File:** `Aleph/core/src/config/mod.rs` or initialization

- [ ] Add built-in `/video` command rule
- [ ] Set default system prompt for video analysis
- [ ] Ensure rule includes `["video", "memory"]` capabilities

**Validation:** Test `/video URL` command

### 4.2 Update Example Config

**File:** `Aleph/config.example.toml`

- [ ] Add `[video]` section with all options documented
- [ ] Add example `/video` rule in `[[rules]]` section
- [ ] Add comments explaining each option

**Validation:** Manual config file review

### 4.3 Add Configuration Validation

**File:** `Aleph/core/src/config/mod.rs`

- [ ] Validate `preferred_language` is valid ISO 639-1 code
- [ ] Validate `max_transcript_length` is reasonable (1000-100000)
- [ ] Log warning if video enabled but YouTube disabled

**Validation:** `cargo test config::validation`

---

## Phase 5: Testing & Documentation (2 tasks)

### 5.1 Add Manual Testing Scenarios

**File:** `docs/manual-testing-checklist.md`

- [ ] Add video capability test scenarios:
  - Video with manual captions (English)
  - Video with auto-generated captions
  - Video without captions (error handling)
  - Very long video (truncation)
  - Non-English video
  - Invalid/private video URL
  - `/video` command usage
  - Auto-detection with other commands

**Validation:** Execute manual test checklist

### 5.2 Update Architecture Documentation

**File:** `docs/ARCHITECTURE.md`

- [ ] Add Video capability to capability list
- [ ] Document trigger mechanism (auto-detect + /video)
- [ ] Add sequence diagram for video processing flow

**Validation:** Documentation review

---

## Dependency Graph

```
Phase 1 (Infrastructure)
    │
    ├── 1.1 Capability Enum ─────┐
    ├── 1.2 Module Structure ────┤
    ├── 1.3 Data Structures ─────┤
    ├── 1.4 VideoConfig ─────────┤
    ├── 1.5 UniFFI Interface ────┤
    ├── 1.6 AgentContext ────────┤
    ├── 1.7 PromptAssembler ─────┤
    └── 1.8 Error Types ─────────┘
                │
                ▼
Phase 2 (YouTube Extraction)
    │
    ├── 2.1 URL Parser ──────────┐
    ├── 2.2 Page Fetcher ────────┤
    ├── 2.3 Player Response ─────┤
    ├── 2.4 Caption Finder ──────┤
    ├── 2.5 XML Parser ──────────┤
    ├── 2.6 Full Pipeline ───────┤
    └── 2.7 Retry Logic ─────────┘
                │
                ▼
Phase 3 (Capability Integration)
    │
    ├── 3.1 CapabilityExecutor ──┐
    ├── 3.2 execute_video ───────┤
    ├── 3.3 Router Detection ────┤
    ├── 3.4 Core Wiring ─────────┤
    └── 3.5 Integration Tests ───┘
                │
                ▼
Phase 4 (Configuration)
    │
    ├── 4.1 Default Rule ────────┐
    ├── 4.2 Example Config ──────┤
    └── 4.3 Validation ──────────┘
                │
                ▼
Phase 5 (Documentation)
    │
    ├── 5.1 Manual Testing ──────┐
    └── 5.2 Architecture Docs ───┘
```

## Parallelizable Work

The following tasks can be done in parallel:

**Parallel Group A (Phase 1):**
- 1.1, 1.2, 1.3, 1.4 can all start simultaneously

**Parallel Group B (Phase 2):**
- 2.1, 2.2 can start after Phase 1
- 2.3, 2.4, 2.5 depend on 2.2
- 2.6 depends on all above

**Parallel Group C (Phase 4-5):**
- 4.2, 5.1, 5.2 can run in parallel after Phase 3
