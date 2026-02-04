## Context
EN: Phase 8.5 (The Eye) adds system-level perception so Aleph can build a System Shadow DOM by fusing AX semantics with visual OCR, reducing context blindness and improving POE evaluation.
ZH: Phase 8.5（The Eye）为 Aleph 增加系统级感知，通过 AX 语义与视觉 OCR 融合构建 System Shadow DOM，降低上下文盲区并增强 POE 评估。

## Goals / Non-Goals
- Goals (EN):
  - Provide a SnapshotTool that returns a structured PerceptionSnapshot (AX tree + optional vision blocks + focus hint).
  - Keep capture on-demand and low-latency; avoid focus stealing.
  - Ensure permission flows are explicit and user-controlled.
- Goals (ZH):
  - 提供 SnapshotTool，返回结构化的 PerceptionSnapshot（AX 树 + 可选视觉块 + 焦点提示）。
  - 捕获按需触发、低延迟，不打断焦点。
  - 权限流程清晰、可控。
- Non-Goals (EN):
  - Continuous full-screen recording.
  - Cloud-based OCR or remote screenshot upload.
  - Hardware eye tracking integration in Phase 8.5.
- Non-Goals (ZH):
  - 持续全屏录制。
  - 云端 OCR 或上传截图。
  - Phase 8.5 内不集成硬件眼动追踪。

## Decisions
- EN: Implement AX tree capture in Rust via `accessibility-sys` with depth/size limits to avoid runaway traversal.
  ZH: 使用 `accessibility-sys` 在 Rust 侧实现 AX 树捕获，并设置深度/大小上限。
- EN: Use CoreGraphics for window/region snapshots and perform OCR via Apple Vision (Swift bridge or native binding).
  ZH: 使用 CoreGraphics 捕获窗口/区域图像，OCR 由 Apple Vision（Swift bridge 或原生绑定）完成。
- EN: SnapshotTool returns structured data + image references (not raw image bytes by default).
  ZH: SnapshotTool 返回结构化数据与图像引用（默认不直接返回原始图像字节）。
- EN: Screen Recording permission is requested on-demand for vision capture, not at app launch.
  ZH: 屏幕录制权限按需申请，不纳入启动强制门禁。
- EN: Tool name is `snapshot_capture` and all bounding boxes use `screen_points_top_left` coordinate space.
  ZH: 工具名称为 `snapshot_capture`，所有边界框统一使用 `screen_points_top_left` 坐标系。
- EN: Default latency budgets are 250ms (AX-only) and 800ms (vision/image); return partial results on timeout.
  ZH: 默认时延预算为 250ms（AX-only）与 800ms（视觉/图像），超时返回部分结果。
- EN: Screen Recording guidance is non-blocking with deep link and a 10-minute prompt cooldown.
  ZH: 屏幕录制引导为非阻塞，提供深链，且 10 分钟提示冷却。

## Risks / Trade-offs
- EN: AX trees can be large or incomplete; merge logic must be conservative to avoid mismatched nodes.
  ZH: AX 树可能巨大或不完整，融合逻辑需保守以避免错误匹配。
- EN: Screen Recording permission adds friction; mitigate with clear UX and AX-only fallback.
  ZH: 屏幕录制权限增加摩擦，可通过 AX-only 回退和清晰提示缓解。
- EN: OCR latency may exceed budgets on older Macs; need time budgets and partial results.
  ZH: OCR 在旧机型上可能超时，需要时间预算与部分结果返回。

## Migration Plan
EN: Introduce new tool and optional fields without breaking existing context capture; existing flows continue when SnapshotTool is unused.
ZH: 新工具与可选字段不破坏现有上下文捕获；未调用 SnapshotTool 时流程不变。

## Open Questions
- EN: What threshold defines “AX coverage is insufficient” for vision fallback?
  ZH: 触发视觉回退的 AX 覆盖率阈值如何定义？
- EN: Where should snapshot image references be stored and how long retained?
  ZH: 快照图像引用存放位置与保留时长如何定义？
