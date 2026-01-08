# skills-settings-ui Specification

## Purpose

定义 Aether Skills 设置界面的 UI 规范，包括布局结构、组件行为、状态管理和交互模式。Skills 设置界面是管理所有 AI 能力扩展（内置 MCP 服务、外部 MCP 服务、提示模板）的统一入口。

## ADDED Requirements

### Requirement: Unified Skills Layout Architecture

Skills 设置界面 SHALL 采用 Filter Sidebar + List + Inspector 的三栏布局结构。

#### Scenario: Layout structure at minimum window size
- **GIVEN** the user opens Settings and navigates to Skills tab
- **WHEN** the window is at minimum size (1200x800)
- **THEN** the view SHALL display three regions:
  - Filter Sidebar: 180px fixed width on the left
  - Skill List: flexible width (fills remaining space minus inspector)
  - Inspector Panel: 400px minimum width on the right (shown when skill selected)
- **AND** the Filter Sidebar SHALL be separated from Skill List by a thin divider
- **AND** the Inspector Panel SHALL slide in from the right with animation

#### Scenario: Layout without selection
- **GIVEN** the Skills tab is active
- **WHEN** no skill is selected
- **THEN** the Inspector Panel SHALL NOT be visible
- **AND** the Skill List SHALL expand to fill the available width
- **AND** an empty state message SHALL be displayed in the list area if no skills exist

---

### Requirement: Skill Filter Sidebar

The Filter Sidebar SHALL provide quick filtering and action buttons for skill management.

#### Scenario: Status filter options
- **GIVEN** the Filter Sidebar is visible
- **WHEN** the user views the status filter section
- **THEN** the following filter options SHALL be available:
  - "全部" (All) - shows all skills
  - "已启用" (Enabled) - shows only enabled skills
  - "已停用" (Disabled) - shows only disabled skills
  - "错误" (Error) - shows only skills with error status
- **AND** exactly one filter option SHALL be selected at a time
- **AND** the "全部" option SHALL be selected by default

#### Scenario: Category filter options
- **GIVEN** the Filter Sidebar is visible
- **WHEN** the user views the category filter section
- **THEN** the following category options SHALL be available:
  - "内置核心" (Builtin) - shows BuiltinMcp skills
  - "外部扩展" (External) - shows ExternalMcp skills
  - "提示模板" (Templates) - shows PromptTemplate skills
- **AND** category filters SHALL be independent of status filters (AND logic)
- **AND** multiple categories MAY be selected simultaneously

#### Scenario: Sidebar action buttons
- **GIVEN** the Filter Sidebar is visible
- **WHEN** the user scrolls to the bottom of the sidebar
- **THEN** two action buttons SHALL be visible:
  - "[+ 添加]" (Add) button - opens Add Skill sheet
  - "[{ } JSON]" button - toggles JSON editor mode
- **AND** the buttons SHALL be positioned at the bottom of the sidebar

---

### Requirement: Skill Card Display

Each skill in the list SHALL be represented as a card with consistent structure.

#### Scenario: Card content layout
- **GIVEN** a skill exists in the list
- **WHEN** the card is rendered
- **THEN** the card SHALL display in a horizontal layout:
  - Left: Skill icon (24x24 SF Symbol) with theme color background
  - Center: VStack containing name (body font) and description (caption font)
  - Right: Status indicator, Toggle switch, and More button (...)
- **AND** the card height SHALL be exactly 72 points
- **AND** the card SHALL have `DesignTokens.Spacing.md` padding

#### Scenario: Card hover state
- **GIVEN** a skill card is rendered
- **WHEN** the user hovers over the card
- **THEN** the card background SHALL change to `.primary.opacity(0.05)`
- **AND** the transition SHALL animate with 150ms duration

#### Scenario: Card selected state
- **GIVEN** a skill card is rendered
- **WHEN** the card is selected
- **THEN** the card SHALL have an accent color border (2px)
- **AND** the Inspector Panel SHALL slide in from the right

---

### Requirement: Skill Status Indicator

Each skill card SHALL display a real-time status indicator.

#### Scenario: Running status display
- **GIVEN** a skill is in Running state
- **WHEN** the status indicator renders
- **THEN** a green circle (8px) SHALL be displayed
- **AND** the text "Running" SHALL be shown next to the circle
- **AND** the circle SHALL have a subtle pulsing animation

#### Scenario: Stopped status display
- **GIVEN** a skill is in Stopped state
- **WHEN** the status indicator renders
- **THEN** a gray circle (8px) SHALL be displayed
- **AND** the text "Stopped" SHALL be shown next to the circle

#### Scenario: Starting status display
- **GIVEN** a skill is in Starting state
- **WHEN** the status indicator renders
- **THEN** a yellow circle (8px) SHALL be displayed
- **AND** the text "Connecting..." SHALL be shown next to the circle

#### Scenario: Error status display
- **GIVEN** a skill is in Error state
- **WHEN** the status indicator renders
- **THEN** a red circle (8px) SHALL be displayed
- **AND** the text "Error" SHALL be shown next to the circle
- **AND** hovering over the indicator SHALL show a tooltip with error details

---

### Requirement: Skill Inspector Panel

The Inspector Panel SHALL provide detailed configuration for the selected skill.

#### Scenario: Inspector panel header
- **GIVEN** a skill is selected
- **WHEN** the Inspector Panel renders
- **THEN** the header section SHALL display:
  - Skill icon (48x48) on the left
  - Skill name (heading font) and type label (caption) in the center
  - Status indicator and Toggle switch on the right
- **AND** the header SHALL be visually separated from the content below

#### Scenario: Connection section for external MCP
- **GIVEN** an ExternalMcp skill is selected
- **WHEN** the Inspector Panel renders
- **THEN** a "Connection" section SHALL be displayed containing:
  - Transport picker (Stdio / SSE dropdown)
  - Command field with Browse button
  - Arguments editor (dynamic list)
  - Working Directory field with Browse button
- **AND** the section SHALL NOT be displayed for BuiltinMcp or PromptTemplate skills

#### Scenario: Environment variables section
- **GIVEN** any skill is selected
- **WHEN** the Inspector Panel renders
- **THEN** an "Environment Variables" section SHALL be displayed
- **AND** each variable SHALL show as a row with:
  - Key field (text input)
  - Value field (SecureField with masked text)
  - Eye button to toggle visibility
  - Delete button
- **AND** an "Add Variable" button SHALL be at the bottom

#### Scenario: Permissions section
- **GIVEN** any skill is selected
- **WHEN** the Inspector Panel renders
- **THEN** a "Permissions" section SHALL be displayed containing:
  - "工具调用前需确认" (Requires confirmation) toggle
  - Allowed Paths list with Add/Remove capability
  - Allowed Commands list (only for shell-type skills)

#### Scenario: Tools section (read-only)
- **GIVEN** a skill with tools is selected
- **WHEN** the Inspector Panel renders
- **THEN** a "Tools" section SHALL display all available tools
- **AND** each tool SHALL be shown as a tag/chip
- **AND** the section SHALL be read-only (no editing allowed)

#### Scenario: Inspector action bar
- **GIVEN** the Inspector Panel is visible
- **WHEN** the user views the bottom of the panel
- **THEN** an action bar SHALL be displayed containing:
  - "View Logs" button on the left
  - "Cancel" button on the right
  - "Save" button on the far right
- **AND** the action bar SHALL be fixed at the bottom of the panel

---

### Requirement: Environment Variable Security

Environment variable values SHALL be protected from casual observation.

#### Scenario: Default masked display
- **GIVEN** an environment variable exists
- **WHEN** the value field is rendered
- **THEN** the value SHALL be displayed as masked characters (••••)
- **AND** the field SHALL use SecureField type

#### Scenario: Toggle value visibility
- **GIVEN** an environment variable is displayed with masked value
- **WHEN** the user clicks the eye button next to the value
- **THEN** the value SHALL be shown in plain text
- **AND** clicking the eye button again SHALL mask the value
- **AND** the visibility state SHALL NOT persist after leaving the panel

---

### Requirement: JSON Editor Mode

The view SHALL support a JSON editor mode for advanced configuration.

#### Scenario: Toggle JSON mode
- **GIVEN** the Skills settings view is active
- **WHEN** the user clicks the "{ } JSON" button in the sidebar
- **THEN** the view SHALL switch to JSON editor mode
- **AND** the full skills configuration SHALL be displayed as formatted JSON
- **AND** clicking the button again SHALL return to GUI mode

#### Scenario: JSON editing and validation
- **GIVEN** the JSON editor mode is active
- **WHEN** the user edits the JSON content
- **THEN** syntax errors SHALL be highlighted inline
- **AND** the Save button SHALL be disabled if JSON is invalid
- **AND** saving valid JSON SHALL update the GUI view accordingly

---

### Requirement: Add Skill Sheet

A sheet SHALL be provided for adding new skills.

#### Scenario: Add skill sheet content
- **GIVEN** the user clicks the "Add" button
- **WHEN** the Add Skill sheet opens
- **THEN** the sheet SHALL present three options:
  - "External MCP Server" - configure a new external MCP process
  - "Import from URL" - install skill from GitHub URL
  - "Import from ZIP" - install skill from local ZIP file

#### Scenario: External MCP form
- **GIVEN** the user selects "External MCP Server"
- **WHEN** the form is displayed
- **THEN** the following fields SHALL be required:
  - Name (text field)
  - Command (text field with Browse)
- **AND** the following fields SHALL be optional:
  - Description
  - Arguments (list)
  - Environment Variables (key-value pairs)
  - Icon (SF Symbol picker)
  - Theme Color (color picker)

---

### Requirement: Skill Logs Viewer

A logs viewer SHALL be accessible for each skill.

#### Scenario: Open logs viewer
- **GIVEN** a skill is selected
- **WHEN** the user clicks "View Logs" button
- **THEN** a sheet SHALL open displaying the skill's logs
- **AND** the logs SHALL be displayed in reverse chronological order
- **AND** the sheet SHALL have a "Refresh" button to reload logs
- **AND** the maximum displayed lines SHALL be configurable (default: 100)

---

### Requirement: Skill Toggle Behavior

The enable/disable toggle SHALL control the skill's operational state.

#### Scenario: Enable a stopped skill
- **GIVEN** a skill is in Stopped state with toggle OFF
- **WHEN** the user turns the toggle ON
- **THEN** the skill status SHALL change to Starting
- **AND** the system SHALL attempt to start the skill
- **AND** on success, the status SHALL change to Running
- **AND** on failure, the status SHALL change to Error

#### Scenario: Disable a running skill
- **GIVEN** a skill is in Running state with toggle ON
- **WHEN** the user turns the toggle OFF
- **THEN** the skill SHALL be gracefully stopped
- **AND** the status SHALL change to Stopped
- **AND** the toggle state SHALL be persisted to configuration

---

### Requirement: Responsive Layout Behavior

The layout SHALL adapt gracefully to window size changes.

#### Scenario: Window resize with inspector open
- **GIVEN** the Inspector Panel is open
- **WHEN** the user resizes the window narrower
- **THEN** the Skill List SHALL shrink proportionally
- **AND** the Inspector Panel SHALL maintain its minimum width (400px)
- **AND** the Filter Sidebar SHALL maintain its fixed width (180px)

#### Scenario: Minimum window constraint
- **GIVEN** the Skills tab is active with Inspector open
- **WHEN** the window would be resized below the minimum required width
- **THEN** the window resize SHALL be constrained
- **AND** the minimum width SHALL be: 180 + 200 + 400 = 780px

---

## MODIFIED Requirements

(None - this is a new spec)

## REMOVED Requirements

(None - this is a new spec)

## Related Capabilities

- `settings-ui-layout` - Parent spec for overall settings UI design patterns
- `implement-mcp-capability` - MCP service implementation (data source)
- `add-skills-capability` - Skills/Prompt template implementation (data source)
