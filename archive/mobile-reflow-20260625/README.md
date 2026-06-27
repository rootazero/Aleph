# Archive · mobile-reflow-20260625

归档的「移动端响应式 reflow」面板方案（已被取代，仅作参考留存）。
Archived snapshot of the superseded "mobile-responsive reflow" panel approach (reference only).

## 这是什么 / What this is

`interfaces/webchat` 面板早期的 iPhone 适配尝试：通过 Tailwind `max-sm:` 断点 + 专用
`MobileTabBar` / `MobileTopBar` / `mobile_landing` 设置页，把现有桌面布局**响应式地折叠**到
手机宽度（Phase 0.5 → Phase 1 → Phase 2）。

An early iPhone-adaptation effort for the `interfaces/webchat` panel: reflowing the
existing desktop layout down to phone widths via Tailwind `max-sm:` breakpoints plus
dedicated `MobileTabBar` / `MobileTopBar` / `mobile_landing` settings screens
(Phase 0.5 → Phase 1 → Phase 2).

## 为何归档 / Why archived

此方案已被 **原生 phone 屏方案** 取代——见活跃源码 `interfaces/webchat/src/platform/phone/`
（FEATURE 3 Chat / FEATURE 4 Memory 等独立手机视图）。reflow 路线的核心文件
（`mobile_tab_bar.rs` / `mobile_top_bar.rs` / `mobile_landing.rs`）已不在 main 活体源码中。

Superseded by the **native phone-screen approach** — see the live source at
`interfaces/webchat/src/platform/phone/`. The reflow approach's core files are no longer
present in main's active tree.

## 出处 / Provenance

- 原分支 / Source branch: `archive/mobile-reflow-20260625`（tip `a7b07af25`，仅本地、未推任何远端）
- 与 main 的合并基 / Merge-base with main: `9a56c3c94` (`style: cargo fmt workspace`)
- 归档日期 / Archived: 2026-06-27
- 内容 / Content: 32 次提交的净改动中的 **69 个源文件最终状态**快照
- 已排除 / Excluded: `dist/` 构建产物（含 15.1 MB WASM）、逐提交历史

> 注：这是改动文件**最终状态的扁平快照**，非完整可编译子树，亦不含逐提交历史；
> 原本地分支在归档后已删除。
> Note: a flat snapshot of the changed files' final state — not a buildable subtree,
> and without per-commit history; the original local branch was deleted after archiving.
