## ADDED Requirements
### Requirement: On-Demand Screen Recording Permission for Visual Snapshots
The system SHALL request Screen Recording permission on-demand when SnapshotTool requires image capture.
ZH: 当 SnapshotTool 需要图像捕获时，系统必须按需请求屏幕录制权限。

#### Scenario: AX-only snapshot without Screen Recording permission
- **WHEN** SnapshotTool is called with include_vision=false and include_image=false
- **THEN** the system does NOT request Screen Recording permission
- **AND** proceeds with AX-only capture

#### Scenario: Image capture without permission
- **WHEN** SnapshotTool is called with include_image=true and Screen Recording permission is not granted
- **THEN** the tool returns a permission error with guidance
- **AND** the macOS prompt is triggered via `CGRequestScreenCaptureAccess()`
- **AND** the system logs "Screen Recording permission required for vision snapshots"

#### Scenario: Vision snapshot without permission
- **WHEN** SnapshotTool is called with include_vision=true and Screen Recording permission is not granted
- **THEN** the tool returns a permission error with guidance
- **AND** the macOS prompt is triggered via `CGRequestScreenCaptureAccess()`
- **AND** the system logs "Screen Recording permission required for vision snapshots"

#### Scenario: Vision snapshot after permission granted
- **GIVEN** Screen Recording permission is granted
- **WHEN** SnapshotTool is called with include_vision=true
- **THEN** the system captures the image and runs OCR
- **AND** returns vision_blocks in PerceptionSnapshot

---

### Requirement: Non-Blocking Permission UX for Screen Recording
The system SHALL present Screen Recording permission guidance without blocking core app functionality.
ZH: 系统必须以非阻塞方式引导屏幕录制权限，不阻断核心功能。

#### Scenario: Non-blocking guidance
- **WHEN** SnapshotTool returns SCREEN_RECORDING_REQUIRED
- **THEN** the system shows a non-blocking Halo toast or inline banner
- **AND** the UI includes an "Open System Settings" action
- **AND** the app does NOT show PermissionGateView or block other features

#### Scenario: Deep link to Screen Recording pane
- **WHEN** user clicks "Open System Settings" from the guidance UI
- **THEN** the system opens macOS System Settings to:
  - `x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture`

#### Scenario: Prompt rate limiting
- **WHEN** Screen Recording permission is denied
- **THEN** the system does NOT re-trigger the prompt more than once per 10 minutes per session
