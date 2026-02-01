# holographic-perception Specification

## Purpose
EN: Provide a System Shadow DOM by fusing Accessibility (AX) semantics with optional vision/OCR snapshots to reduce context blindness.
ZH: 通过融合 AX 语义与可选视觉/OCR 快照构建 System Shadow DOM，降低上下文盲区。

## ADDED Requirements
### Requirement: SnapshotTool Interface
The system SHALL provide a built-in tool named `snapshot_capture` (SnapshotTool) that returns a structured PerceptionSnapshot.
ZH: 系统必须提供名为 `snapshot_capture` 的内置工具（SnapshotTool），返回结构化的 PerceptionSnapshot。

PerceptionSnapshot MUST include:
- schema_version (int, default 1)
- snapshot_id (string, prefix "ps_")
- captured_at (UTC ISO-8601)
- target (frontmost_window | region)
- coordinate_space ("screen_points_top_left")
- partial (bool)
- ax_tree (optional)
- vision_blocks (optional)
- shadow_dom (optional)
- focus_hint (optional)
- image_ref (optional)
- errors (optional list)

SnapshotTool input MUST support:
- target (frontmost_window | region, default frontmost_window)
- region (x, y, width, height) when target=region
- include_ax (bool, default true)
- include_vision (bool, default false)
- include_image (bool, default false)
- image_format ("png" | "jpeg", default "png")
- max_latency_ms (int, default 250 for ax-only; 800 when include_vision or include_image is true; min 50, max 2000)
- focus_window_ms (int, default 1200; min 200, max 5000)
- ax_limits (optional: max_depth default 12, max_nodes default 1500, max_value_bytes default 256)
- vision_limits (optional: max_blocks default 200, min_confidence default 0.30)
- merge_strategy (optional: "iou"; iou_threshold default 0.60)

#### Scenario: Defaults applied
- **WHEN** SnapshotTool is called with empty args
- **THEN** target is frontmost_window
- **AND** include_ax=true, include_vision=false, include_image=false
- **AND** max_latency_ms=250
- **AND** coordinate_space="screen_points_top_left"

#### Scenario: AX-only snapshot
- **WHEN** SnapshotTool is called with include_ax=true and include_vision=false
- **THEN** PerceptionSnapshot includes ax_tree
- **AND** vision_blocks is empty or null
- **AND** image_ref is null
- **AND** completes within max_latency_ms

#### Scenario: Vision snapshot with image reference
- **WHEN** SnapshotTool is called with include_vision=true and include_image=true
- **THEN** PerceptionSnapshot includes vision_blocks and image_ref
- **AND** shadow_dom is present when merge succeeds
- **AND** completes within max_latency_ms or returns partial results with errors

---

### Requirement: Coordinate Space Convention
The system SHALL represent all bounding boxes in screen points with origin at the top-left of the primary display, with Y increasing downward.
ZH: 系统必须使用屏幕坐标（点）表示所有边界框，原点在主显示器左上角，Y 轴向下。

#### Scenario: Coordinate space published
- **WHEN** SnapshotTool returns PerceptionSnapshot
- **THEN** coordinate_space is "screen_points_top_left"
- **AND** all ax_tree, vision_blocks, shadow_dom, and focus_hint bounding boxes use this space

---

### Requirement: AX Tree Capture
The system SHALL capture an Accessibility (AX) tree for the frontmost window when include_ax=true.
ZH: 当 include_ax=true 时，系统必须捕获前台窗口的 AX 树。

AX nodes MUST include:
- node_id (string, unique within snapshot)
- role (string)
- title (optional)
- value (optional)
- frame (x, y, width, height)
- children (list of node_id)

The capture MUST enforce depth and node-count limits to avoid runaway traversal.
If limits are reached, the snapshot MUST set partial=true and include error code "AX_LIMIT_REACHED".

#### Scenario: Capture AX tree with permission
- **GIVEN** Accessibility permission is granted
- **WHEN** SnapshotTool captures the frontmost window
- **THEN** ax_tree is populated with nodes and bounding frames
- **AND** node count is within configured limits

#### Scenario: AX permission missing
- **WHEN** Accessibility permission is not granted
- **THEN** ax_tree is null or empty
- **AND** errors includes "AX_PERMISSION_REQUIRED"

---

### Requirement: Vision OCR Capture
The system SHALL perform OCR on the captured window/region image when include_vision=true.
ZH: 当 include_vision=true 时，系统必须对窗口/区域图像执行 OCR。

vision_blocks MUST include:
- block_id (string, unique within snapshot)
- text (string)
- bbox (x, y, width, height)
- confidence (0.0 - 1.0)
- language (optional)

Results MUST respect vision_limits (min_confidence, max_blocks) and be ordered top-to-bottom, left-to-right.

#### Scenario: OCR success
- **GIVEN** Screen Recording permission is granted
- **WHEN** SnapshotTool captures a vision snapshot
- **THEN** vision_blocks includes at least one text block when text is present
- **AND** each block includes text and bounding box

#### Scenario: Confidence threshold
- **WHEN** OCR returns blocks below min_confidence
- **THEN** those blocks are omitted from vision_blocks

#### Scenario: Screen Recording permission missing
- **WHEN** Screen Recording permission is not granted
- **THEN** vision_blocks is null or empty
- **AND** errors includes "SCREEN_RECORDING_REQUIRED"

---

### Requirement: Shadow DOM Merge
The system SHALL attempt to merge AX nodes and vision blocks into a shadow_dom when both are present.
ZH: 当 AX 与视觉结果同时存在时，系统必须尝试合并生成 shadow_dom。

shadow_dom nodes MUST include:
- node_id (string)
- bbox (x, y, width, height)
- text (optional)
- role (optional)
- sources (list of { ax_node_id?, vision_block_id? })

#### Scenario: Merge success
- **GIVEN** both ax_tree and vision_blocks are present
- **WHEN** merge_strategy is "iou"
- **THEN** shadow_dom includes nodes with merged sources
- **AND** each merged node references its AX and/or vision source IDs

#### Scenario: Merge failure
- **WHEN** merge cannot be completed within max_latency_ms
- **THEN** shadow_dom is null
- **AND** errors includes "MERGE_FAILED"

---

### Requirement: Focus Hint Inference
The system SHALL infer a focus_hint from recent input signals (mouse dwell/click, keyboard focus) and include it in PerceptionSnapshot.
ZH: 系统必须根据近期输入信号（鼠标停留/点击、键盘焦点）推断 focus_hint 并写入 PerceptionSnapshot。

focus_hint MUST include:
- bbox (x, y, width, height)
- source (mouse_dwell | mouse_click | keyboard_focus)
- confidence (0.0 - 1.0)
- last_event_at (UTC ISO-8601)

#### Scenario: Focus hint from mouse dwell
- **GIVEN** mouse dwell exceeds focus_window_ms over a region
- **WHEN** SnapshotTool is executed
- **THEN** focus_hint includes a bounding box around the dwell region
- **AND** focus_hint includes a confidence score

#### Scenario: No recent input signals
- **WHEN** there is no recent mouse/keyboard signal within focus_window_ms
- **THEN** focus_hint is null

---

### Requirement: Latency Budget and Partial Results
The system SHALL honor max_latency_ms and return partial results when the budget is exceeded.
ZH: 系统必须遵守 max_latency_ms，在超时情况下返回部分结果。

#### Scenario: Time budget exceeded
- **WHEN** SnapshotTool exceeds max_latency_ms
- **THEN** PerceptionSnapshot sets partial=true
- **AND** errors includes "TIME_BUDGET_EXCEEDED"
- **AND** any completed sub-results (AX or vision) are still returned
