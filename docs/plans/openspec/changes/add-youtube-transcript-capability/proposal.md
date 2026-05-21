# Proposal: Add YouTube Transcript Capability

**Change ID:** `add-youtube-transcript-capability`
**Status:** Draft
**Author:** Claude
**Created:** 2025-01-06

## Summary

Enable Aleph to analyze YouTube video content by automatically extracting video transcripts (subtitles/captions) and injecting them into the AI context. This allows users to use commands like `/search analyze this video: https://youtube.com/watch?v=...` to get AI-powered video content analysis.

## Motivation

### User Pain Point

Users want to:
1. Summarize long YouTube videos quickly
2. Extract key points from educational content
3. Translate video content to another language
4. Ask questions about video content without watching the entire video

Currently, Aleph can only process text and images. Video content analysis requires manual transcript copying, which breaks the "frictionless" user experience.

### Why YouTube First

- YouTube is the largest video platform with ~800M videos
- Most YouTube videos have auto-generated captions (90%+ coverage)
- YouTube transcript extraction is well-documented and lightweight
- No video download required - only text extraction

## Approach

### Technical Strategy: Transcript Extraction (MVP)

Extract video subtitles/captions via YouTube's publicly available transcript API. This is the **lightest and fastest** approach:

| Aspect | Details |
|--------|---------|
| Data Source | YouTube auto-generated or manual captions |
| Processing | HTTP request → Parse XML/JSON → Text extraction |
| Dependencies | Pure Rust (reqwest + quick-xml/serde_json) |
| Latency | ~500ms-2s (single HTTP request) |
| Cost | Free (no API key required for public videos) |

### Alternatives Considered (Rejected for MVP)

| Approach | Pros | Cons | Decision |
|----------|------|------|----------|
| **A. Transcript Extraction** | Fast, free, lightweight | Requires captions exist | **Selected for MVP** |
| B. Audio Transcription (Whisper) | Works without captions | Slow (~10s/min), requires download | Future enhancement |
| C. Multimodal API (Gemini) | Full video understanding | Expensive, API limits | Future enhancement |

## Scope

### In Scope (MVP)

1. **YouTube URL Detection**: Regex pattern matching for `youtube.com/watch?v=` and `youtu.be/` formats
2. **Transcript Extraction**: Fetch auto-generated or manual captions in user's preferred language
3. **New Capability Type**: `Video` capability alongside existing Memory/Search/MCP
4. **Context Injection**: Format transcript with timestamps for AI consumption
5. **Error Handling**: Graceful fallback when captions unavailable

### Out of Scope (Future)

- Other video platforms (Vimeo, Bilibili, TikTok)
- Audio-only transcription (Whisper)
- Video frame analysis
- Live stream support
- Download or caching of video content

## Integration Points

### Capability System Extension

```
Capability Enum:
  Memory = 0    (existing)
  Search = 1    (existing)
  Mcp = 2       (existing)
  Video = 3     (NEW)
```

### Processing Flow

```
User Input: "/search analyze: https://youtube.com/watch?v=xyz"
                ↓
Router: Detect YouTube URL → Add Video capability
                ↓
CapabilityExecutor: Execute Video capability
                ↓
VideoCapability: Extract transcript from YouTube
                ↓
PromptAssembler: Inject transcript into context
                ↓
Provider: Send to AI with transcript context
```

### Configuration

```toml
[video]
enabled = true
youtube_transcript = true
preferred_language = "en"     # Fallback language for transcripts
max_transcript_length = 50000 # Character limit

[[rules]]
regex = "^/analyze"
provider = "claude"
system_prompt = "Analyze the following video transcript."
capabilities = ["video", "memory"]
```

## Success Criteria

1. User can analyze YouTube video content with a single hotkey press
2. Transcript extraction completes within 3 seconds
3. AI receives properly formatted transcript with timestamps
4. Graceful error message when video has no captions
5. Works with both `youtube.com` and `youtu.be` URL formats

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| YouTube API changes | Medium | High | Use multiple extraction methods, version pinning |
| No captions available | Low | Medium | Clear error message, suggest alternative |
| Rate limiting | Low | Low | Implement retry with backoff |
| Large transcript size | Medium | Medium | Truncate with warning, configurable limit |

## Dependencies

### New Rust Crates (Estimated)

- `regex`: URL pattern matching (existing)
- `reqwest`: HTTP client (existing)
- `quick-xml` or `serde_json`: Parse transcript format
- No new Swift dependencies

### System Requirements

- Network access to YouTube
- No additional permissions required

## Timeline Estimate

This is a **medium-sized change** affecting:
- Rust core: ~400-600 lines (new `video/` module)
- Config: ~20 lines (new section)
- UniFFI: ~30 lines (new capability enum value)
- Tests: ~200 lines

## Related Changes

- `add-multimodal-content-support` (completed): Established media handling patterns
- `add-search-capability-integration` (in progress): Similar capability extension pattern
