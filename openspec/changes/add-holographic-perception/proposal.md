# Change: Add Holographic Perception (Phase 8.5 - The Eye)
# 变更：全息感知（Phase 8.5 - The Eye）

## Why
EN: Aleph needs system-level perception to reduce context blindness and strengthen POE (Principle–Operation–Evaluation) by providing richer evidence for Principle definition and Evaluation validation.
ZH: Aleph 需要系统级感知来消除上下文盲区，并强化 POE（Principle–Operation–Evaluation），让 Principle 更精确、Evaluation 更有证据。

## What Changes
- EN: Introduce a SnapshotTool that fuses Accessibility (AX) tree capture with optional visual OCR to build a System Shadow DOM.
  ZH: 引入 SnapshotTool，融合 AX 树与可选视觉 OCR，形成 System Shadow DOM。
- EN: Add focus hint inference from input signals (mouse dwell/click) to surface the user's attention locus.
  ZH: 通过输入信号（鼠标停留/点击）推断注意力焦点。
- EN: Extend context capture to attach an optional perception snapshot reference.
  ZH: 扩展上下文捕获，附加可选的感知快照引用。
- EN: Add on-demand Screen Recording permission flow for visual snapshots.
  ZH: 为视觉快照增加按需的屏幕录制权限流程。

## Impact
- Affected specs: `context-capture`, `permission-gating`, `holographic-perception` (new)
- Affected code: Rust core perception module, built-in SnapshotTool, macOS permission handling, Swift Vision/OCR bridge (if required)
