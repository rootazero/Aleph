# Five-theme TUI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Deliver five switchable palettes with Amber default and a documented community JSON theme interface. / 提供五套可切换主题、默认琥珀与公开社区 JSON 主题接口。

**Architecture:** Reuse shared transcript semantic roles. Resolve built-in and user JSON through a single validated palette layer; keep local display preferences in Aleph home. Widgets consume resolved palette/chrome colors. / 复用共享语义角色，内置与社区主题统一校验解析，显示偏好本地保存，组件只消费已解析样式。

**Tech Stack:** Rust, existing serde_json, Ratatui/crossterm, existing tempfile tests; no new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-16-tui-five-themes-design.md`

## Global Constraints

- Isolated `feat/tui-streaming-refactor` worktree only; no main merge.
- All Cargo calls serial through `py -B qa/terminal/memory_guard.py -- cargo ...`, `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0`.
- No real-home preference writes from tests; owned temporary directories only.
- Preserve terminal transparency, compact sticky width 48, borderless transcript and chat reliability fixes.
- Comments English; reference documentation bilingual. Versioned pure JSON; no executable community plugin.

## Review Focus

- Malformed, oversized or path-traversing theme files must fail visibly without replacing active colors.
- Custom Paper-like palettes must select light code highlighting and readable chrome.
- Windows Terminal without COLORTERM must receive RGB; explicit ANSI override must suppress RGB.
- Switching themes must invalidate markdown and highlighted-code caches as well as widget colors.
- Remembered community names and legacy names must resolve on restart without writing user preferences during tests.

### Task 1: Palette contract and resolution

**Files:** `interfaces/tui/src/tui/theme.rs`, new `theme/` modules and `interfaces/tui/themes/*.json`; `highlight.rs` palette adaptations.
**Interfaces:** Produce `palette() -> Palette`, `theme() -> Theme`, `select_theme(&str) -> Result<(), String>`, `current_theme_name() -> String`, `available_themes() -> (Vec<String>, Vec<String>)`, `remember_theme(&str) -> io::Result<PathBuf>`, and startup notices. Preserve existing semantic rendering consumers.

- [x] Pin malformed/version/name/color rejection, five built-ins, default Amber, capability signals, temporary preference round-trip and community loading in tests.
- [x] Implement versioned strict JSON validation, bounded loading, pure capability selection, local catalog and current palette state.
- [x] Publish schema/example and ensure all shipped themes pass the production loader.
- [x] Run scoped theme tests under memory guard and inspect outcomes.

### Task 2: Theme application in visible surfaces

**Files:** `render.rs`, `widgets/input_area.rs`, `widgets/tool_row.rs`, `widgets/sticky_user.rs`, `widgets/status_bar.rs`, related renderer/color consumers.
**Interfaces:** Consume Theme colors including `background`, `surface`, `border`, `border_focused`, `status_bg/fg`; keep semantic color mapping shared.

- [x] Add renderer assertions for active RGB foreground/background, rule and tool colors, with isolated palettes and no global test races.
- [x] Apply theme base foreground/background, tool surfaces and input/sticky/status styles while preserving geometry and hit-testing.
- [x] Ensure syntax highlighting uses palette lightness and clears on theme change.

### Task 3: Commands and public author interface

**Files:** `commands.rs`, `slash.rs`, `mod.rs`, `README.md`, `docs/reference/TUI_THEMES.md`, `FEATURE_LOCATOR.md`.
**Interfaces:** Use Task 1 public selection/catalog API; expose public theme module through TUI library.

- [x] Wire `/theme`, `/theme list`, `/theme <name>` and `/theme reload`; invalidate rendered lines after successful selection.
- [x] Display current selection, capability and load/remember failures honestly; surface startup fallback warnings.
- [x] Document exact schema, role mapping, paths, packaging, precedence, compatibility and terminal background limitations; provide author example.

### Task 4: Verification and delivery

**Files:** local ignored verification/deployment helpers; plan/spec acceptance records.

- [x] Audit selected tests for home/vault/keyring side effects, then run TUI lib suite and strict scoped Clippy serially.
- [x] Build TUI binary, review complete diff independently and address material findings.
- [x] Exercise built executable with owned console; verify theme switching and persistence in temporary ALEPH_HOME without touching credentials.
- [ ] Commit code/docs, deploy TUI to existing local acceptance directory and open updated acceptance terminal.
- [x] Record exact commands, results and remaining visual acceptance limits.

## Verification record / 验证记录

- `cargo test -p aleph-tui --lib --bins -- --test-threads=1`: library **553 passed, 2 ignored**; binary **8 passed**. All calls used the physical-memory guard, one Cargo job and disabled dev debug information. / 库测试 553 通过、2 忽略，程序测试 8 通过；均经物理内存守卫串行执行。
- `cargo clippy -p aleph-tui --all-targets -- -D warnings` and `cargo build -p aleph-tui --bin aleph-tui`: passed. Targeted rustfmt and `git diff --check`: passed. / 严格静态检查、构建、限定文件格式与差异检查通过。
- Real Ratatui buffer assertions cover all five palettes, transcript/tool/composer colors, Paper overlay background after clearing, dark cursor contrast and clipped-line surface retention. / 实际单元格断言覆盖五套主题正文、工具与输入配色、浅色浮层清除后的底色、深色光标与裁剪行背景。
- Owned native Windows console drove the built executable with a temporary `ALEPH_HOME`: Amber default, six consecutive built-in/community switches, invalid reload retaining the active palette, community preference restoration after restart, no console diagnostics, and process cleanup all passed. No credential or real-home preference file was modified by the probe. / 自有原生控制台驱动重建程序，临时目录验证默认琥珀、六次连续切换、无效重载保留配色、重启恢复社区主题、无日志污染及进程清理；探针未修改真实偏好或凭据。
- Read-only independent review identified overlay background, clipped line style, cursor contrast and command-name conflicts; all four findings were addressed with regression coverage. / 独立只读审查提出浮层底色、裁剪样式、光标对比度与命令名称冲突，四项均已修复并覆盖回归。
- Native console automation verifies behavior and cell styles; exact font rendering and terminal transparency remain dependent on Windows Terminal settings and manual visual acceptance. / 自动化验证行为与单元格样式，实际字形与透明效果仍由 Windows Terminal 设置及人工视觉验收决定。
