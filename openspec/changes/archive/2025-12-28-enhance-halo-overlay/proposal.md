# Proposal: Enhance Halo Overlay

## Change Metadata
- **Change ID**: `enhance-halo-overlay`
- **Title**: Enhance Halo Overlay with Advanced Features and Polish
- **Status**: Proposed
- **Created**: 2025-12-23
- **Phase**: Phase 3

## Why

Phase 2 delivered a functional Halo overlay foundation, but user feedback and design review identified critical gaps that prevent Aleph from achieving its "Ghost" aesthetic vision and production-ready quality standards.

## Motivation

Phase 2 successfully delivered a working Halo overlay with basic animations and state transitions. However, user testing revealed several areas for improvement:

1. **Limited Visual Feedback**: Current animations are functional but lack the "Ghost" aesthetic polish that defines Aleph's brand
2. **Missing Progress Indicators**: Users can't see streaming AI responses or processing progress
3. **Inflexible Theming**: Provider colors are hardcoded; no support for custom themes (cyberpunk, zen, jarvis)
4. **No Audio Feedback**: Silent operation makes it less engaging for users who prefer multi-sensory feedback
5. **Basic Error UX**: Error states show generic red X; no actionable information or retry options
6. **Performance Gaps**: Animations may not consistently hit 60fps on older hardware
7. **Limited Accessibility**: No VoiceOver support or accessibility considerations

## Goals

### Primary Goals
1. **Visual Polish**: Implement theme system (cyberpunk, zen, jarvis) with smooth transitions
2. **Streaming Response Display**: Show AI response character-by-character in Halo overlay
3. **Enhanced Error Handling**: Display actionable error messages with retry options
4. **Audio Feedback**: Optional sound effects for state transitions (subtle, non-intrusive)
5. **Performance Optimization**: Guarantee 60fps animations on macOS 13+ hardware

### Secondary Goals
6. **Accessibility**: VoiceOver support for Halo state announcements
7. **Customization**: User-configurable Halo size, opacity, and animation speed
8. **Multi-Halo Support**: Handle rapid consecutive hotkey presses gracefully

## Non-Goals
- Changing core Halo architecture (NSWindow-based overlay remains)
- AI provider integration (deferred to Phase 4)
- Settings persistence beyond themes (Phase 5)
- Windows/Linux support (macOS-only for Phase 3)

## Proposed Solution

### 1. Theme System (`halo-theming`)

Implement three pre-defined themes with distinct visual languages:

**Cyberpunk Theme:**
- Neon colors (cyan, magenta, yellow)
- Glitch effects during transitions
- Hexagonal Halo shape
- Scanline overlay texture
- RGB split effect on errors

**Zen Theme:**
- Soft pastels (white, light gray, sage green)
- Smooth, organic animations
- Circular Halo with breathing effect
- Ink-wash gradient fills
- Gentle fade transitions

**Jarvis Theme (Iron Man-inspired):**
- Arc reactor blue (#00d4ff)
- Geometric HUD elements
- Hexagonal segments that assemble/disassemble
- Pulsing energy core center
- Tech readout style text

**Implementation:**
- Add `Theme` enum in Swift (Cyberpunk, Zen, Jarvis)
- Store theme preference in UserDefaults
- Create separate SwiftUI view components per theme
- Theme switching triggers re-render with crossfade transition

### 2. Streaming Response Display (`halo-streaming-text`)

Show AI responses as they arrive, character-by-character:

**Design:**
- Halo expands vertically to accommodate text (max 3 lines)
- Text appears in scrolling marquee style
- Monospace font for code, sans-serif for prose
- Auto-collapse back to spinner after 2s of no new text
- Text color matches theme palette

**Implementation:**
- Add `text: String?` to `HaloState.processing` case
- Rust core streams response chunks via callback: `on_response_chunk(text: String)`
- SwiftUI `Text` view with typewriter animation
- Use `GeometryReader` for dynamic sizing

### 3. Enhanced Error Handling (`halo-error-feedback`)

Replace generic red X with actionable error information:

**Error Types:**
- `NetworkError`: Show "Network error" + Retry button
- `PermissionError`: Show "Permission denied" + Open Settings button
- `QuotaError`: Show "API quota exceeded" + fallback provider option
- `TimeoutError`: Show "Request timed out" + Retry button

**Implementation:**
- Define `ErrorType` enum in UniFFI
- Callback: `on_error(error_type: ErrorType, message: String)`
- Error overlay shows icon + message + action button
- Button triggers Swift callback → Rust retry logic

### 4. Audio Feedback System (`halo-audio-feedback`)

Optional sound effects for key state transitions:

**Sounds:**
- `listening.aiff`: Soft "whoosh" (100ms)
- `processing.aiff`: Gentle hum loop
- `success.aiff`: Satisfying "ding" (200ms)
- `error.aiff`: Subtle "thud" (150ms)

**Implementation:**
- Use `AVFoundation` to play system sounds
- Toggle via `soundEnabled` in UserDefaults
- Volume: 30% of system volume (non-intrusive)
- Pre-load all sounds on app launch

### 5. Performance Optimization (`halo-performance`)

Ensure smooth 60fps animations:

**Optimizations:**
- Use `CADisplayLink` for frame-perfect timing
- Minimize SwiftUI view hierarchy depth
- Pre-render complex shapes with `CGPath`
- Use `Metal` shaders for theme effects (cyberpunk glitch)
- Profile with Instruments: < 16ms per frame

**Fallback Strategy:**
- Detect device GPU capabilities on launch
- Disable expensive effects on Intel HD 3000/4000
- Degrade gracefully: disable glitch effects, use solid colors

### 6. Accessibility Support (`halo-accessibility`)

VoiceOver announcements for state changes:

**Announcements:**
- Listening: "Aleph listening"
- Processing: "Processing with OpenAI" (provider name)
- Success: "Complete"
- Error: "Error: [message]"

**Implementation:**
- Set `accessibilityLabel` on HaloWindow
- Post `NSAccessibility.Notification.announcementRequested` on state change
- Use `.accessibilityElement(children: .combine)` for VoiceOver focus

### 7. User Customization (`halo-customization`)

Add preferences for Halo appearance:

**Settings:**
- Size: Small (80px) / Medium (120px) / Large (160px)
- Opacity: 50% / 75% / 100%
- Animation speed: Slow (1.5x) / Normal (1x) / Fast (0.7x)

**Implementation:**
- Store in `UserDefaults` under `HaloPreferences` struct
- Pass to HaloWindow on initialization
- Apply via SwiftUI modifiers: `.frame()`, `.opacity()`, `.animation()`

### 8. Multi-Halo Handling (`halo-concurrency`)

Handle rapid hotkey presses without visual conflicts:

**Strategy:**
- Queue hotkey events (FIFO)
- If Halo is visible, ignore new hotkey until current operation completes
- Show subtle "busy" indicator (pulsing border) if hotkey pressed during processing
- Max queue depth: 3 operations

**Implementation:**
- Add `isProcessing` flag in EventHandler
- Guard hotkey callback with `if !isProcessing { ... }`
- Display queued count badge on Halo (e.g., "2 pending")

## Implementation Plan

### Stage 1: Core Theme System
1. Create `Theme` enum and assets
2. Implement theme-specific SwiftUI views
3. Add theme selector to Settings (stub)
4. Theme switching logic with transitions

### Stage 2: Streaming & Error UX
5. Add streaming text display to HaloView
6. Implement error type classification
7. Create error action buttons (Retry, Open Settings)
8. Test error scenarios manually

### Stage 3: Audio & Performance
9. Add audio assets to bundle
10. Implement sound playback with AVFoundation
11. Run performance profiling
12. Optimize frame rate bottlenecks

### Stage 4: Accessibility & Customization
13. Add VoiceOver support
14. Implement customization settings
15. Test with macOS accessibility tools

### Stage 5: Polish & Testing
16. Handle multi-Halo edge cases
17. Comprehensive manual testing
18. Update documentation

## Risks and Mitigations

### Risk: Theme complexity increases maintenance burden
**Mitigation**: Use protocol-based theme system; each theme is self-contained module

### Risk: Streaming text causes Halo to flicker/resize jankily
**Mitigation**: Pre-allocate max height; use smooth spring animations for size changes

### Risk: Audio feedback annoys users
**Mitigation**: Default to OFF; make easily toggleable in menu bar (right-click → Mute Sounds)

### Risk: Performance regressions on older Macs
**Mitigation**: Implement capability detection; disable Metal shaders on Intel GPUs

### Risk: Accessibility announcements interrupt user workflow
**Mitigation**: Use low-priority announcements; only trigger on explicit user-initiated actions

## Testing Strategy

### Manual Testing
- Test all 3 themes on macOS 13, 14, 15
- Test streaming text with varying response lengths
- Test error states with real network failures
- Test audio feedback at different system volumes
- Profile with Instruments on 2018 MacBook Pro
- VoiceOver testing with macOS accessibility inspector

### Automated Testing
- Unit tests for Theme enum and view selection logic
- Snapshot tests for each theme's visual appearance
- Performance tests: measure frame rate under load
- Mock audio playback for CI/CD (no actual sound)

## Acceptance Criteria

1. ✅ All 3 themes implemented and selectable
2. ✅ Streaming text displays smoothly (< 100ms latency per chunk)
3. ✅ Error messages are actionable (buttons work)
4. ✅ Sound effects play correctly (when enabled)
5. ✅ 60fps animations on target hardware (2018+ Macs)
6. ✅ VoiceOver announces all state changes
7. ✅ Customization settings persist across app restarts
8. ✅ Multi-Halo queue works without UI glitches

## Documentation Updates

- Update CLAUDE.md with Phase 3 completion status
- Add theme system architecture to README
- Document customization options in user guide
- Update TESTING_GUIDE.md with new test scenarios

## Dependencies

- **Blocks**: None (Phase 2 complete)
- **Blocked By**: None
- **Related**:
  - `add-macos-client-and-halo-overlay` (Phase 2) - builds upon this foundation

## Open Questions

1. **Theme assets**: Should we hire a designer for professional theme assets, or use procedurally generated graphics?
   - **Recommendation**: Start with procedural graphics (SwiftUI shapes); defer custom assets to Phase 6 polish

2. **Streaming latency**: What's acceptable character display latency for streaming text?
   - **Recommendation**: Target < 50ms; measured from Rust callback to screen render

3. **Error retry limits**: How many auto-retries before giving up?
   - **Recommendation**: Max 2 auto-retries; then show manual Retry button

4. **Theme persistence**: Use UserDefaults or write to config.toml?
   - **Recommendation**: UserDefaults for Phase 3; migrate to config.toml in Phase 5

## Timeline Estimate

**NOT INCLUDED** (per project conventions - no time estimates in planning)

## Stakeholders

- **Implementers**: Swift developer (UI), Rust developer (callbacks)
- **Reviewers**: UX designer (theme aesthetics), accessibility specialist
- **Approvers**: Project lead

## References

- Phase 2 completion status: `openspec/changes/add-macos-client-and-halo-overlay/`
- CLAUDE.md sections: "Development Phases", "UI Constraints", "Key Design Constraints"
- Apple Human Interface Guidelines: [Accessibility](https://developer.apple.com/design/human-interface-guidelines/accessibility)
