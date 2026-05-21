# Capability: Halo Toast

## Summary

Toast notification system integrated with Halo overlay, replacing system NSAlert dialogs for a consistent "Ghost" aesthetic experience.

## ADDED Requirements

### Requirement: Toast State Support

The HaloState enum MUST support a toast case for displaying notifications.

#### Scenario: Toast state is added to HaloState

**Given** the HaloState enum
**When** a toast notification is triggered
**Then** the state transitions to `.toast(type:title:message:autoDismiss:onDismiss:)`
**And** the HaloView renders the HaloToastView component

### Requirement: Toast Type Classification

The system MUST support three toast types with distinct visual styling.

#### Scenario: Info toast displays blue accent

**Given** a toast with type `.info`
**When** the toast is rendered
**Then** the icon is "info.circle.fill"
**And** the accent color is blue (#007AFF)

#### Scenario: Warning toast displays orange accent

**Given** a toast with type `.warning`
**When** the toast is rendered
**Then** the icon is "exclamationmark.triangle.fill"
**And** the accent color is orange (#FF9500)

#### Scenario: Error toast displays red accent

**Given** a toast with type `.error`
**When** the toast is rendered
**Then** the icon is "xmark.circle.fill"
**And** the accent color is red (#FF3B30)

### Requirement: Light Background with Semi-Transparency

Toast MUST use a light, semi-transparent background for readability.

#### Scenario: Toast background is visible against any content

**Given** a toast is displayed
**When** the background is rendered
**Then** the background color is white at 85-90% opacity
**And** a subtle blur effect (ultraThinMaterial) is applied
**And** a soft drop shadow is visible
**And** the corner radius is 12px

### Requirement: Dynamic Sizing Based on Content

Toast dimensions MUST adapt to content length.

#### Scenario: Short message fits minimum width

**Given** a toast with title "Success" and message "Done."
**When** the toast size is calculated
**Then** the width is at least 200px
**And** the height accommodates title and single-line message

#### Scenario: Long message wraps and expands height

**Given** a toast with a message exceeding 400px in width
**When** the toast size is calculated
**Then** the width is capped at 400px
**And** the message wraps to multiple lines
**And** the height expands to fit up to 5 lines of message text

#### Scenario: Very long message is truncated

**Given** a toast with message exceeding 5 lines when wrapped at 400px
**When** the toast is rendered
**Then** the message is truncated with ellipsis at line 5

### Requirement: Close Button Design

Toast MUST have a small, elegant close button.

#### Scenario: Close button is visible and positioned correctly

**Given** a toast is displayed
**When** the user views the toast
**Then** a close button (xmark icon) is visible in the top-right corner
**And** the button size is 16x16 pixels
**And** the button has hover effect (increased opacity and scale)

#### Scenario: Close button dismisses toast

**Given** a toast is displayed
**When** the user clicks the close button
**Then** the toast is dismissed with fade-out animation
**And** the HaloState returns to idle

### Requirement: Auto-Dismiss for Info Toasts

Info toasts MUST auto-dismiss after a timeout when autoDismiss is enabled.

#### Scenario: Info toast auto-dismisses after 3 seconds

**Given** a toast with type `.info` and autoDismiss enabled
**When** 3 seconds have elapsed without user interaction
**Then** the toast automatically dismisses
**And** the HaloState returns to idle

#### Scenario: Warning and error toasts require manual dismissal

**Given** a toast with type `.warning` or `.error`
**When** the toast is displayed
**Then** the toast does NOT auto-dismiss
**And** the user must click the close button to dismiss

### Requirement: Focus Preservation

Toast window MUST NOT steal focus from the active application.

#### Scenario: Toast appears without stealing focus

**Given** the user is typing in another application
**When** a toast is displayed
**Then** keyboard focus remains in the original application
**And** the toast window is floating but not key window

#### Scenario: Close button is clickable without stealing focus

**Given** a toast is displayed while user is in another app
**When** the user clicks the close button
**Then** the toast dismisses
**And** focus remains in the original application

### Requirement: Screen Center Positioning

Toast MUST appear at the center of the screen.

#### Scenario: Toast appears at screen center

**Given** a toast is triggered
**When** the toast window position is calculated
**Then** the toast is horizontally and vertically centered on the screen

### Requirement: Animation

Toast appearance and dismissal MUST be animated.

#### Scenario: Toast fade-in animation on appear

**Given** a toast is triggered
**When** the toast appears
**Then** the toast scales from 0.9 to 1.0
**And** the opacity increases from 0 to 1
**And** the animation duration is approximately 0.3 seconds

#### Scenario: Toast fade-out animation on dismiss

**Given** a toast is displayed
**When** the toast is dismissed
**Then** the opacity decreases from 1 to 0
**And** the animation duration is approximately 0.2 seconds

### Requirement: EventHandler Toast Methods

The EventHandler class MUST provide methods for showing and dismissing toast notifications.

#### Scenario: showToast method triggers toast display

**Given** an EventHandler implementation
**When** `showToast(type:title:message:autoDismiss:)` is called
**Then** the HaloState MUST transition to toast state
**And** the HaloWindow MUST display the toast view

#### Scenario: dismissToast method hides toast

**Given** an EventHandler implementation with active toast
**When** `dismissToast()` is called
**Then** the HaloState MUST transition to idle
**And** the toast view MUST be dismissed with animation

### Requirement: HaloTheme Toast View Support

The HaloTheme protocol MUST include a method for rendering toast views.

#### Scenario: Theme provides customized toast view

**Given** a theme implementing HaloTheme protocol
**When** toast state is active
**Then** the theme's `toastView(type:title:message:onDismiss:)` method MUST be called
**And** the returned view MUST be rendered in HaloView

## Dependencies

- **HaloState**: Toast case added to existing state enum
- **HaloWindow**: Window sizing and mouse event handling for toast
- **HaloView**: Switch case for toast state rendering
- **EventHandler**: Protocol extension for toast methods
- **HaloTheme**: Protocol extension for toast view customization
