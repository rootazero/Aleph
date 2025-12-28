# Halo Accessibility Specification

## ADDED Requirements

### Requirement: VoiceOver Announcements for State Changes
VoiceOver SHALL announce Halo state changes with descriptive, user-friendly messages.

#### Scenario: VoiceOver announces listening state

**Given** VoiceOver is enabled on macOS
**And** user presses hotkey (Cmd+~)
**When** Halo transitions to .listening state
**Then** VoiceOver speaks "Aether listening"
**And** announcement uses low priority (non-intrusive)
**And** announcement does not interrupt current VoiceOver context

---

#### Scenario: VoiceOver announces processing with provider name

**Given** VoiceOver is enabled
**When** Halo transitions to .processing(providerColor: green)
**Then** provider color mapped to provider name (green = OpenAI)
**And** VoiceOver speaks "Processing with OpenAI"
**And** announcement timing: immediately after state change (< 100ms)

---

#### Scenario: VoiceOver announces error with message

**Given** VoiceOver is enabled
**When** Halo transitions to .error(type: .network, message: "Network unavailable")
**Then** VoiceOver speaks "Error: Network unavailable"
**And** error type provides context (network vs permission vs quota)

---

### Requirement: HaloWindow Accessibility Labels
HaloWindow SHALL provide accessibility labels for all states visible to VoiceOver.

#### Scenario: Accessibility Inspector shows correct labels

**Given** Accessibility Inspector is open
**When** Halo is in .processing state
**Then** HaloWindow.accessibilityLabel is "Processing with OpenAI"
**And** accessibilityRole is .window
**And** window is marked as accessible (not ignored)

---

### Requirement: Low Priority Accessibility Announcements
Accessibility announcements SHALL use NSAccessibility.post() with low priority to avoid interrupting user workflow.

#### Scenario: Announcement priority validation

**Given** VoiceOver is reading long text in Safari
**When** Halo state changes to .success
**Then** NSAccessibility.post() called with announcement "Complete"
**And** userInfo includes priority: .low
**And** VoiceOver queues announcement (does not interrupt current text)
**And** announcement plays after current speech completes

---

### Requirement: Accessible Streaming Text with Live Region Updates
Streaming text SHALL be accessible via VoiceOver with live region updates.

#### Scenario: VoiceOver reads streaming response text

**Given** VoiceOver is enabled
**When** streaming text updates from "Hello" to "Hello World"
**Then** accessibilityValue updates to "Hello World"
**And** VoiceOver announces new text incrementally
**And** announcement rate respects VoiceOver speed setting

---

## Cross-References

- **Related Specs**: `macos-client` (NSWindow accessibility), `event-handler` (state change triggers)
- **Depends On**: macOS Accessibility APIs (NSAccessibility)
- **Blocks**: None
