# Halo Streaming Text Specification

## ADDED Requirements

### Requirement: Display Streaming AI Response Text
The Halo overlay SHALL display AI response text as it streams from the provider, character-by-character.

#### Scenario: Streaming text appears during OpenAI response

**Given** user triggers hotkey with selected text
**And** request routes to OpenAI (streaming enabled)
**When** OpenAI sends first chunk "Hello"
**Then** Rust calls on_response_chunk("Hello")
**And** Swift updates HaloState.processing with text
**And** StreamingTextView renders "Hello" with typewriter animation
**And** Halo frame expands vertically to accommodate text (max 200px height)
**And** expansion animation uses spring curve (0.4s response, 0.6 damping)

---

#### Scenario: Text accumulates across multiple streaming chunks

**Given** Halo is displaying "Hello" from first chunk
**When** OpenAI sends second chunk " World"
**Then** Rust calls on_response_chunk("Hello World")
**And** Swift appends new text to existing
**And** typewriter animation continues from character 5
**And** no flicker or text reset occurs

---

### Requirement: Typewriter Animation for Text Reveal
Streaming text SHALL use typewriter animation revealing characters at 50ms intervals.

#### Scenario: Typewriter animation timing verification

**Given** StreamingTextView receives text "Test"
**When** view appears on screen
**Then** "T" appears at t=0ms
**And** "e" appears at t=50ms
**And** "s" appears at t=100ms
**And** "t" appears at t=150ms
**And** total animation duration is 200ms

---

### Requirement: Maximum Text Lines with Wrapping Support
Streaming text display SHALL support maximum 3 lines with automatic text wrapping.

#### Scenario: Text overflow triggers horizontal marquee scroll

**Given** streaming text exceeds 3 lines (> 120 characters)
**When** line 3 wraps to line 4
**Then** text display switches to marquee scroll mode
**And** text scrolls horizontally at 30 pixels/second
**And** scrolling loops seamlessly (infinite scroll)

---

### Requirement: Auto-Collapse Halo Frame After Streaming Completes
Halo frame SHALL auto-collapse back to spinner-only view after 2 seconds of no new text chunks.

#### Scenario: Auto-collapse after streaming completes

**Given** Halo is displaying expanded view with streaming text "Response text"
**And** last chunk received at t=0s
**When** 2 seconds elapse with no new chunks
**Then** Halo frame animates back to 120x120 size (spinner only)
**And** text fades out with 0.3s duration
**And** collapse animation uses easeInOut curve

---

### Requirement: UniFFI Callback for Streaming Text Delivery
UniFFI callback SHALL provide on_response_chunk(text: String) for streaming text delivery.

#### Scenario: Rust streams response chunks to Swift

**Given** AetherCore is processing AI request
**When** AI provider returns streaming response
**Then** Rust accumulates chunks in buffer
**And** calls on_response_chunk(full_text) every 100ms
**And** Swift EventHandler receives callback on background thread
**And** EventHandler dispatches to main queue for UI update
**And** latency from Rust callback to screen render < 50ms

---

### Requirement: Font Selection Based on Content Type
Streaming text SHALL use monospace font for code blocks and sans-serif for prose.

#### Scenario: Font selection based on content type

**Given** StreamingTextView receives text "function main() { }"
**When** text contains keywords: "function", "class", "def", etc.
**Then** font switches to SF Mono (monospace)
**And** syntax highlighting disabled (plain text)

**Given** StreamingTextView receives text "Hello world"
**When** text is plain prose (no code keywords)
**Then** font uses SF Pro (sans-serif)
**And** line height optimized for readability (1.4x)

---

## Cross-References

- **Related Specs**: `event-handler` (callback implementation), `uniffi-bridge` (FFI)
- **Depends On**: Phase 2 HaloView, UniFFI bindings
- **Blocks**: None
