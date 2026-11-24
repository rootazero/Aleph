# NSH Panel 两栏外壳与个人偏好 实施计划 / Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (native; chosen per user preference "plan written → implement directly"). Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 TSX Panel 的顶部标签栏换成固定两栏外壳，并交付外观、快捷键、命令面板三项本机偏好，规则只在 Rust 存一份。

**Architecture:** 规则放进 `shared_ui_logic::prefs`（不受 `panel` feature 门控，Leptos 默认构建可用）；`interfaces/panel/src/lib.rs` 的 `PanelModel` 以 JSON 进出暴露给 TSX；TSX 外壳只做接线（存储、`setTheme`/`setColorScheme`、`keymap.bind`、弹层）。Desktop 字号经 `panel_surface` 注册的 `aleph.fontScale` 主机函数投递到主线程。

**Tech Stack:** Rust（shared-ui-logic、nsh-core 0.6.0 `keymap::Keystroke`）、NSH TSX（`nsh`、`nsh/base`、`nsh/test`）、Leptos webchat、dispatch2 0.3.1（已在锁文件内，nsh-platform-macos 的传递依赖）。

**Spec:** `docs/superpowers/specs/2026-11-24-nsh-panel-prefs-design.md`

## Global Constraints

- 只在 worktree `.worktrees/nsh-panel-prefs`（branch `nsh-panel-prefs`）工作，不动 main。
- R3：不引入新 crate 到依赖图；dispatch2 只能用锁文件里已有的 `0.3.1`。不引入 TS 状态框架。
- 偏好存储键 `aleph.panel.prefs.v1`，不按端点/subject 分；布局键与格式（`aleph.panel.layout.v1`，`PanelLayout` v1）不变。
- 损坏或未知版本的偏好回默认并报告 `repair`，不抛错。
- 默认快捷键都带修饰键；每个组合键单独 `keymap.bind` 并各自 `try/catch`。
- Web（`platform.os === "web"`）不给 `mod+1…9` 默认绑定。
- 模块命令 id = `<tab key>/<local id>`（如 `aleph:chat/stop`），SDK `dispose()` 或换代即注销。
- 桌面嵌入 QuickJS 没有 `crypto` 全局：TSX 不用 `crypto`。
- 不做：玻璃材质、跨设备同步、descriptor 命令字段、右侧面板、主题导入、“新会话”命令。
- 代码注释用英文；回执与文档中英双语。

## Review Focus

1. 一条保存的系统保留键（`mod+q`）或非法文本：只有这一条失效，`mod+k` 仍能打开面板 → Task 5 测试 `a reserved override disables only itself`。
2. 断线/未配对时打开设置并改主题 → Task 4 测试 `settings open while disconnected`。
3. 模块卸载或重连后执行它注册过的命令 → Task 5 `sdk.test.ts` 两条（dispose、generation）。
4. 两个命令落到同一规范化组合键（`cmd+K` 与 `mod+k`）→ Task 1 `conflict_on_equivalent_keystrokes` + Task 3 `normalize` 测试。
5. 存储里是旧版/损坏的偏好 JSON → Task 1 `restore_*` 测试 + Task 4 打开即修复写回。

---

## File Structure

| 文件 | 职责 |
|---|---|
| `shared/ui_logic/src/prefs/mod.rs`（新） | `PanelPrefs`、`Binding`、`restore`、编辑函数、`PREFS_STORAGE_KEY` |
| `shared/ui_logic/src/prefs/appearance.rs`（新） | 5 个外观枚举 + 存储值解析、`AccentPalette` 表、`theme_tokens` |
| `shared/ui_logic/src/prefs/commands.rs`（新） | `score`、`rank`、`resolve_bindings` |
| `shared/ui_logic/src/lib.rs` | `pub mod prefs;` |
| `interfaces/webchat/src/appearance.rs` | 改为 re-export 共享枚举；i18n 标签改成 `AppearanceLabel` trait；保留 DOM/localStorage |
| `interfaces/webchat/src/components/command_palette.rs` | 用共享 `score`，删本地实现与测试 |
| `interfaces/webchat/src/appearance_drift.rs`（新，`#[cfg(test)]`） | 强调色表 vs `styles/tailwind.css` 一致性测试 |
| `interfaces/panel/src/lib.rs` | `PanelModel` 新方法 |
| `interfaces/panel/shell/prefs.ts`（新） | 读写偏好、应用主题/明暗/字号 |
| `interfaces/panel/shell/commands.ts`（新） | 命令表（外壳/菜单/模块）、按键绑定 |
| `interfaces/panel/shell/Sidebar.tsx`（新） | 左栏菜单 |
| `interfaces/panel/shell/Settings.tsx`（新） | 设置页：外观 / 快捷键 / 菜单（内嵌 `LayoutEditor`） |
| `interfaces/panel/shell/CommandPalette.tsx`（新） | `mod+k` 弹层 |
| `interfaces/panel/shell/Shell.tsx` | 两栏框架；连接 `If` 只包右栏 |
| `interfaces/panel/shell/sdk.ts` | `commands.register` |
| `interfaces/panel/modules/Chat.tsx` | 注册 `stop` |
| `interfaces/panel/nsh.json`、`web/host.js` | `aleph.fontScale` |
| `desktop/nsh-shell/src/panel_surface.rs`、`Cargo.toml` | `aleph.fontScale` 处理函数 + 主线程交接 |

---

### Task 1: 共享偏好与命令规则 `shared_ui_logic::prefs`

**Files:**
- Create: `shared/ui_logic/src/prefs/{mod.rs,appearance.rs,commands.rs}`
- Modify: `shared/ui_logic/src/lib.rs`

**Interfaces — Produces:**
- `appearance.rs`：`ThemeMode{System,Light,Dark}`、`Accent{Mauve,Ocean,Forest,Sunset,Rose}`、`FontScale{Compact,Default,Cozy,Large,Largest}`、`Roundness{Sharp,Slight,Default,Round,Extra}`、`Density{Compact,Cozy,Spacious}`，全部 `#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]`，`#[serde(rename_all = "lowercase")]`。每个都有 `pub const ALL`、`pub fn from_storage(Option<&str>) -> Self`、`pub const fn storage_value(self) -> Option<&'static str>`；`Accent::id`、`Accent::swatch`、`FontScale/Roundness/Density::css_value` 原样从 `interfaces/webchat/src/appearance.rs` 搬来（值与旧值兼容完全一致，含 `"glass"|"translucent"` → `Dark`）。新增 `FontScale::factor(self) -> f32`、`Roundness::factor(self) -> f32`。
- `pub struct AccentPalette { pub primary: &'static str, pub selected: &'static str }`；`impl Accent { pub const fn palette(self, dark: bool) -> AccentPalette }`。值取自 `interfaces/webchat/styles/tailwind.css`：mauve 来自基础 `@theme`（亮 `:26,:44`）与 `.dark`（`:283,:296`）；其余来自 `html[data-accent=…]`（`:362-396`）与 `html.dark[data-accent=…]`（`:400-` ）的 `--color-primary` / `--color-sidebar-active`。
- `pub struct Appearance { pub mode, pub accent, pub font_scale, pub roundness, pub density }`（`Default`，serde camelCase）。
- `pub fn theme_tokens(appearance: &Appearance) -> serde_json::Value`：`{"cssVars":{"theme":{"radius":"<0.625×factor>rem"},"light":{…},"dark":{…}}}`，light/dark 各写 `primary`、`sidebar-primary`、`ring`、`chart-1`（= palette.primary）、`sidebar-accent`（= palette.selected）、`primary-foreground`、`sidebar-primary-foreground`（= `"oklch(0.985 0 0)"`）。Sharp 写 `"0rem"`。
- `mod.rs`：`pub const PREFS_STORAGE_KEY: &str = "aleph.panel.prefs.v1"`；`pub const PREFS_VERSION: u32 = 1`；`#[serde(tag/untagged…)] pub enum Binding { Key(String), Cleared }`（序列化为 `{"key":"…"}` / `"cleared"`）；`pub struct PanelPrefs { pub version: u32, pub appearance: Appearance, pub keybindings: BTreeMap<String, Binding> }`（`Default`、`to_json(&self) -> String`）；`pub enum PrefsRepair { Corrupt, UnknownVersion(u32) }`；`pub struct RestoredPrefs { pub prefs: PanelPrefs, pub repair: Option<PrefsRepair> }`；`pub fn restore(stored: Option<&str>) -> RestoredPrefs`（`None` → 默认且无 repair；未知字段忽略）；`pub fn set_appearance(prefs: &mut PanelPrefs, axis: &str, value: &str) -> bool`（axis ∈ `mode|accent|fontScale|roundness|density`，value 为枚举的 serde 名；非法返回 false 不改）；`pub fn set_binding(prefs: &mut PanelPrefs, command: &str, binding: Option<Binding>)`（`None` = 恢复默认，删除覆盖）。
- `commands.rs`：`pub fn score(query: &str, label: &str, keywords: &[&str]) -> u32`（从 `command_palette.rs:193` 原样搬）；`pub struct RankItem<'a> { pub id: &'a str, pub label: &'a str, pub keywords: Vec<&'a str> }`；`pub fn rank<'a>(query: &str, items: &[RankItem<'a>]) -> Vec<&'a str>`（score>0 的项按分数降序，同分保持输入顺序；空查询保持输入顺序）；`pub struct CommandKey<'a> { pub id: &'a str, pub default_key: Option<&'a str> }`；`pub enum KeySource { Default, User, Cleared }`；`pub enum KeyProblem { Invalid, Conflict(String) }`（`Conflict` 带先占用者 id）；`pub struct ResolvedKey { pub id: String, pub key: Option<String>, pub source: KeySource, pub problem: Option<KeyProblem> }`；`pub fn resolve_bindings(commands: &[CommandKey], overrides: &BTreeMap<String, Binding>, normalize: impl Fn(&str) -> Option<String>) -> Vec<ResolvedKey>`：顺序遍历，用户覆盖优先，`normalize` 返回 `None` → `Invalid` 且 `key: None`；规范化后已被前面命令占用 → `Conflict(先占者)` 且不生效（`key` 仍填规范文本供设置页显示）。

- [ ] **Step 1: 写失败测试**（`#[cfg(test)]` 于各文件）
  - `restore_none_is_default_without_repair`、`restore_corrupt_reports_corrupt`（`"{"`）、`restore_unknown_version_resets`（`{"version":9}` → `UnknownVersion(9)`）、`restore_ignores_unknown_fields`、`roundtrip_keeps_overrides`（`Key("mod+j")` 与 `Cleared` 都存活）。
  - `legacy_mode_values_load_dark`（`"glass"`、`"translucent"` → `Dark`）、`storage_values_roundtrip_for_all`（每个枚举 `from_storage(storage_value())` 回到自身）。
  - `theme_tokens_emit_both_schemes`：`Accent::Ocean` → `light.primary == "oklch(0.55 0.130 250)"`、`dark.primary == "oklch(0.68 0.130 250)"`、`light["sidebar-accent"] == "oklch(0.95 0.022 250)"`；`Roundness::Sharp` → `theme.radius == "0rem"`，`Extra` → `"1.25rem"`。
  - `score_*`：把 `command_palette.rs:461-` 的 5 条测试原样搬来。
  - `rank_orders_by_score_then_input`。
  - `user_override_beats_default`、`cleared_has_no_key`、`invalid_override_is_reported`、`conflict_on_equivalent_keystrokes`（normalize 把 `cmd+k`/`mod+k` 都映射成 `cmd+k`；第二个命令 `problem == Conflict(first)`）。
- [ ] **Step 2:** `cargo test -p shared-ui-logic prefs` → 编译失败（模块不存在）。
- [ ] **Step 3:** 实现上述接口；`lib.rs` 加 `pub mod prefs;`（无 cfg）。
- [ ] **Step 4:** `cargo test -p shared-ui-logic prefs` PASS；`cargo test -p shared-ui-logic --no-default-features --features panel` PASS（门控外模块两种 feature 组合都编译）。
- [ ] **Step 5:** commit `feat(ui-logic): shared panel preferences, theme tokens and command rules`

### Task 2: Leptos 改用共享规则

**Files:**
- Modify: `interfaces/webchat/src/appearance.rs`、`interfaces/webchat/src/components/command_palette.rs`、`interfaces/webchat/src/lib.rs`（声明 `#[cfg(test)] mod appearance_drift;`），以及编译器指出的 `.label(i18n)` 调用处（`src/platform/phone/settings/{mod.rs,appearance.rs}`、`src/platform/wide/views/chat/composer/appearance_command.rs`、`src/components/command_palette.rs`、`src/platform/wide/views/settings/appearance.rs`）加 `use crate::appearance::AppearanceLabel;`
- Create: `interfaces/webchat/src/appearance_drift.rs`

**Interfaces — Consumes:** Task 1 枚举、`Accent::palette`、`commands::score`。**Produces:** `pub trait AppearanceLabel { fn label(self, i18n: I18nContext<Locale>) -> String; }`，为 5 个共享枚举实现（`Material` 保持本地固有方法）。

- [ ] **Step 1:** 写 `appearance_drift.rs` 测试 `accent_table_matches_tailwind_css`：`include_str!("../styles/tailwind.css")`，对每个 `Accent` × {亮,暗} 取对应块（mauve：`@theme {` 块与 `.dark {` 块；其他：`html[data-accent="<id>"] {` 与 `html.dark[data-accent="<id>"] {`），断言 `--color-primary`、`--color-chart-1`、`--color-ring` 去掉 ` / 0.35` 后都等于 `palette.primary`，`--color-sidebar-active` 等于 `palette.selected`。逐行文本解析，不引入 CSS 解析库。
- [ ] **Step 2:** `appearance.rs`：删除 5 个枚举定义及其 `from_storage`/`storage_value`/`id`/`swatch`/`css_value`，改为 `pub use shared_ui_logic::prefs::appearance::{Accent, Density, FontScale, Roundness, ThemeMode};`；标签移入 `impl AppearanceLabel for …`；`read_*` 调用共享 `from_storage`。存储键与 DOM 应用不变。
- [ ] **Step 3:** `command_palette.rs`：删除 `score` 与其测试模块中的 `score_*` 测试，改 `use shared_ui_logic::prefs::commands::score;`。
- [ ] **Step 4:** 验证：`cargo test -p aleph-panel --lib`（含 drift 测试）PASS；`cargo check -p aleph-panel --lib --target wasm32-unknown-unknown` PASS（默认 feature，即 `just wasm` 的组合）；`cargo clippy -p aleph-panel --lib --target wasm32-unknown-unknown -- -D warnings` 无新增告警。负向检查：临时把 `Accent::Ocean` 亮色 primary 改一位，drift 测试 FAIL，再恢复并 `touch`。
- [ ] **Step 5:** commit `refactor(webchat): read appearance enums and command scoring from shared-ui-logic`

### Task 3: `PanelModel` 绑定

**Files:** Modify `interfaces/panel/src/lib.rs`（实现 + `#[cfg(test)]`）。

**Interfaces — Produces（TSX 侧通过 `aleph/panel` 调用，JSON 进出；驼峰名由 NSH 生成）：**
- `prefs(stored: Option<String>) -> JsonValue` → `{ appearance: {mode, accent, fontScale, roundness, density}, keybindings: {id: {key}|"cleared"}, stored: String, repair: "corrupt"|"unknown_version"|null, theme: <theme_tokens>, fontFactor: f32, density: "compact"|"cozy"|"spacious" }`。
- `set_appearance(stored: String, axis: String, value: String) -> Option<String>`
- `set_binding(stored: String, command: String, binding: JsonValue) -> Option<String>`（`null` = 恢复默认，`"cleared"`，`{"key": "<text>"}`）
- `normalize_keystroke(text: String, apple: bool) -> Option<String>`：`nsh_core::keymap::Keystroke::parse_with(&text, apple).ok().map(|k| k.to_string())`。
- `resolve_bindings(commands: JsonValue /* [{id, defaultKey}] */, stored: String, apple: bool) -> JsonValue` → `[{ id, key: String|null, source: "default"|"user"|"cleared", problem: null|"invalid"|{conflict: id} }]`。
- `rank_commands(query: String, items: JsonValue /* [{id,label,keywords}] */) -> Vec<String>`。

- [ ] **Step 1:** 失败测试：`prefs_of_nothing_is_default`、`set_appearance_rejects_unknown_axis`、`set_binding_null_restores_default`、`normalize_treats_mod_as_cmd_on_apple`（`normalize_keystroke("mod+K", true) == normalize_keystroke("cmd+k", true)`，`("mod+k", false)` 不等）、`resolve_reports_conflict_and_invalid`、`rank_commands_filters`。
- [ ] **Step 2:** `cargo test -p aleph-panel-ui` → FAIL。
- [ ] **Step 3:** 实现（每个方法只做 JSON 转换并调用 Task 1 函数）。
- [ ] **Step 4:** `cargo test -p aleph-panel-ui` PASS；`nsh check interfaces/panel` 生成 `.nsh/types/aleph/*.d.ts` 并通过。
- [ ] **Step 5:** commit `feat(panel): expose preference and command rules to the TSX shell`

### Task 4: 两栏外壳、外观偏好、设置页

**Files:**
- Create: `interfaces/panel/shell/{prefs.ts,Sidebar.tsx,Settings.tsx}`
- Modify: `interfaces/panel/shell/Shell.tsx`、`interfaces/panel/App.test.tsx`
- Delete from Shell: 顶部标签栏、`Customize` 按钮、`editing` 信号

**Interfaces:**
- `prefs.ts`：`type Prefs = ReturnType`-shaped 对象（`PanelModel.prefs` 的 JSON）；`async function loadPrefs(): Promise<Prefs>`（`Storage.getItem(PREFS_KEY)` → `PanelModel.prefs` → 若 `stored` 不同则写回 → `applyPrefs`）；`function applyPrefs(prefs: Prefs): { fontScale: "applied" | "unsupported" }`：`setTheme(prefs.theme)`、`setColorScheme(prefs.appearance.mode)`、`host.call("aleph.fontScale", prefs.fontFactor)`（拒绝 → `"unsupported"`）；`async function savePrefs(stored: string): Promise<Prefs>`；`const PREFS_KEY = "aleph.panel.prefs.v1"`；`const DENSITY_CLASSES: Record<Density, { row: string; gap: string }>`（compact `px-2 py-1`/`gap-1`，cozy `px-3 py-1.5`/`gap-1.5`，spacious `px-3 py-2`/`gap-2`）。
- `Sidebar.tsx`：`Sidebar(props: { view: PanelView | null; connection: () => ConnectionState; active: () => string | null; settingsOpen: () => boolean; compact: () => boolean; density: () => Density; onOpen(key: string): void; onSettings(): void })`。菜单项 `accessibilityLabel` 沿用 `Open <label>`（现有测试依赖）；状态点 `accessibilityLabel` 为 `Connection: <state>`；设置项 `accessibilityLabel="Settings"`；compact 时只显示图标首字母。
- `Settings.tsx`：`Settings(props: { prefs: () => Prefs; onAppearance(axis: string, value: string): void; view: PanelView | null; onMove; onHidden; shortcuts: JSX.Element })`；三个分区标题 `Appearance` / `Shortcuts` / `Menu`；外观每个选项是按钮，`accessibilityLabel` 形如 `Accent ocean`、`Mode dark`、`Text size large`、`Corners sharp`、`Density cozy`；字号不支持时显示 `Not supported here`。
- Shell：`settingsOpen` 信号；右栏 = settingsOpen ? Settings : (connected ? ModuleSlot : Offline)；`horizontalSizeClass() === "compact"` 传给 Sidebar；偏好读完之前只渲染 `<View class="flex-1 bg-background" />`。

- [ ] **Step 1:** 改/写 `App.test.tsx`：
  - 修改 `layout round-trips through storage`：`press("label:Settings")` 替代 `press("Customize")`，其余断言不变。
  - `settings open while disconnected`：`aleph.connection` 返回 `{state:"disconnected",…}` → `waitFor("label:Settings")` → press → `press("label:Accent ocean")` → `JSON.parse(storage()["aleph.panel.prefs.v1"]).appearance.accent === "ocean"`。
  - `appearance survives a restart`：存储预置 `{version:1, appearance:{mode:"dark",accent:"rose",…}}` → 打开后设置页 `label:Accent rose` 呈选中态（`accessibilityState`/文本 `✓`，实现时二选一并在测试里断言同一个）。
  - `corrupt prefs are repaired on open`：预置 `"{"` → 打开后存储值等于默认 `PanelPrefs` JSON。
  - `font size reaches the host`：handlers 加 `"aleph.fontScale": (n) => null`；press `label:Text size large` → `host.calls()` 末条 `aleph.fontScale` 的 data 为 `1.25`。
- [ ] **Step 2:** `nsh test interfaces/panel` → 新测试 FAIL。
- [ ] **Step 3:** 实现 `prefs.ts`、`Sidebar.tsx`、`Settings.tsx`、改 `Shell.tsx`（`LayoutEditor` 移进 Settings 的 Menu 分区，组件本身不改）；`nsh.json` `capabilities.host` 加 `"aleph.fontScale"`（测试 host 需要它）。
- [ ] **Step 4:** `nsh test interfaces/panel` 全部 PASS（含原有 11 条）；`nsh check interfaces/panel` PASS。
- [ ] **Step 5:** commit `feat(panel): fixed two-column shell with appearance settings`

### Task 5: 命令、快捷键、命令面板

**Files:**
- Create: `interfaces/panel/shell/{commands.ts,CommandPalette.tsx}`
- Modify: `interfaces/panel/shell/{sdk.ts,sdk.test.ts,Shell.tsx,Settings.tsx}`、`interfaces/panel/modules/Chat.tsx`、`interfaces/panel/App.test.tsx`

**Interfaces:**
- `sdk.ts`：`export type CommandSpec = { id: string; label: string; keywords?: string[]; defaultKey?: string; run(): void }`；`ShellAccess.registerCommand(spec: CommandSpec): () => void`；`PanelSdk.commands.register(spec: Omit<CommandSpec,"id"> & { id: string }): () => void` —— 以 `${key}/${spec.id}` 注册、经 `hold` 释放，`run` 只在 `current()` 时执行。
- `commands.ts`：`export type Command = CommandSpec & { group: "shell" | "menu" | "module" }`；`export function shellCommands(actions: { palette(): void; settings(): void; toggleScheme(): void; reconnect(): void; openSlot(index: number): void }, web: boolean): Command[]`（ids：`shell.palette` mod+k、`shell.settings` mod+,、`shell.scheme`、`shell.reconnect`、`shell.slot.1`…`shell.slot.9` 默认 `mod+1…9`，`web` 时无默认）；`export function menuCommands(view: PanelView | null, open: (key: string) => void): Command[]`（每个 live slot 一条 `open:<key>`，label `Open <label>`）；`export function bindCommands(commands: Command[], resolved: ResolvedKey[]): { unbind(): void; failures: Map<string, string> }`：对每个有 `key` 且无 `problem` 的项单独 `keymap.bind(key, run)`，`try/catch` 把错误消息记进 `failures`。`apple` 取 `keymap.label("mod+k").includes("⌘")`（NSH 文档保证 Apple 平台及其上的浏览器以 ⌘ 显示），只在 `commands.ts` 算一次。
- `CommandPalette.tsx`：`CommandPalette(props: { commands: () => Command[]; labelOf(id): string | null; onClose(): void })`；输入框 `placeholder="Type a command"`；结果顺序 = `PanelModel.rankCommands`；`up`/`down`/`enter`/`escape` 用 `keymap.bind(..., { when: "always" })`；每行显示快捷键 `keymap.label`。
- Settings Shortcuts 分区：每行命令 label + 当前键 `keymap.label` + 输入框（`accessibilityLabel="Shortcut for <label>"`，提交时 `PanelModel.normalizeKeystroke` 校验，非法显示 `Invalid shortcut`）+ `Clear` + `Reset`；问题显示 `Conflicts with <label>` / `Reserved by the system: <message>`。
- `Chat.tsx`：挂载时 `sdk.commands.register({ id: "stop", label: "Stop reply", run: stop })`。

- [ ] **Step 1:** 测试：
  - `sdk.test.ts`：`module commands unregister on dispose`、`module commands from an older generation do not run`（`run` 不被调用）、`module command ids carry the tab key`。
  - `App.test.tsx`：`mod+k opens the palette and opens a menu entry`（`app.keyboard.press("mod+k")` → `waitFor("Type a command")` → `type` "sess" → `keyboard.press("enter")` → `waitFor` Sessions 模块内容）；`a rebound shortcut fires`（设置页把 `shell.palette` 改成 `mod+j` → `mod+j` 打开面板，`mod+k` 不再打开）；`a reserved override disables only itself`（预置偏好 `keybindings: {"shell.settings": {key:"mod+q"}}`，`platform: "macos"` → 设置页显示 `Reserved by the system`，`mod+k` 仍打开面板）；`conflicting shortcuts are shown`（`shell.settings` 改成 `mod+k` → 显示 `Conflicts with`）；`typing in chat does not trigger shortcuts`（Chat 输入框 `type("k")` 后面板未打开）；`chat stop is in the palette only while chat is mounted`。
- [ ] **Step 2:** `nsh test interfaces/panel` → FAIL。
- [ ] **Step 3:** 实现；Shell 汇总三类命令 → `PanelModel.resolveBindings` → `bindCommands`，命令表或偏好变化时先 `unbind()` 再重绑。
- [ ] **Step 4:** `nsh test interfaces/panel` + `nsh check interfaces/panel` PASS。
- [ ] **Step 5:** commit `feat(panel): command palette and rebindable shortcuts`

### Task 6: 字号主机函数（Desktop + Web）

**Files:**
- Modify: `desktop/nsh-shell/src/panel_surface.rs`、`desktop/nsh-shell/Cargo.toml`（macOS 段加 `dispatch2 = "0.3.1"`）、`interfaces/panel/web/host.js`、`interfaces/panel/web/host.test.mjs`

**Interfaces:** `aleph.fontScale(data: number) -> null`；非有限数或 ≤0 → reject `"font scale must be a positive number"`。

- [ ] **Step 1:** `host.test.mjs` 加 `font_scale_sets_the_root_size_and_reports`：stub `document.documentElement.style` 与 `player.environmentChanged`，`call("aleph.fontScale", 1.25)` 后 `fontSize === "20px"` 且 `environmentChanged` 被调用一次；`call("aleph.fontScale", -1)` reject。
- [ ] **Step 2:** `node --test interfaces/panel/web/host.test.mjs` → FAIL。
- [ ] **Step 3:** `host.js` switch 加分支：设 `document.documentElement.style.fontSize = \`${16 * scale}px\``，调用 `player.environmentChanged()`。`panel_surface.rs`：`thread_local! { static SURFACE_VIEW: RefCell<Weak<AppView>> }`，`AppView::new` 后写入；在 `view.load` 之前 `channel.handle("aleph.fontScale", …)`：解析数值后 `dispatch2::DispatchQueue::main().exec_async(move || SURFACE_VIEW.with(|v| if let Some(view) = v.borrow().upgrade() { view.set_font_scale(Some(scale)) }))`，`call.resolve(Value::Null)`；`PanelSurface` 持有 `HostChannel` 克隆，`Drop` 里 `remove_handler("aleph.fontScale")` 并清空 thread-local。
- [ ] **Step 4:** `node --test …host.test.mjs` PASS；`just panel-tsx-bundle` 后 `cargo check -p aleph-nsh-shell --features tsx-panel` 与 `cargo clippy -p aleph-nsh-shell --features tsx-panel -- -D warnings` PASS。
- [ ] **Step 5:** commit `feat(nsh-shell): apply the Panel's text size on Desktop and Web`

### Task 7: 真机验证与回执

**Files:** Create `qa/nsh_prefs/{run.py,README.md,evidence/}`（沿用 `qa/nsh_s3_chat/run.py` 的 Gateway 启动与截图方式）、`docs/superpowers/runs/2026-11-24-nsh-panel-prefs-receipt.{md,json}`

- [ ] **Step 1 (spike, 先做):** macOS Desktop：`mod+k`、`mod+,` 能否到达嵌入 AppView（窗口菜单是否截获）；Chrome：设置根字号是否只影响 Panel；`mod+1…9` 与 `mod+k`、`mod+,` 在 Chrome 是否到达页面。结论若与 Task 5 默认冲突，改默认表并补测试后再继续。
- [ ] **Step 2:** 同一隔离 Gateway：Desktop + Chrome 各走一遍——切换明暗/强调色/圆角/字号/密度并重启保留；断线时打开设置；改绑生效、冲突可见、保留键只失效一条；`mod+k` 执行社区模块“打开 X”；Chat 输入框打字不触发快捷键；中文输入法（双拼）下 `mod+k` 不误触。每项截图入 `qa/nsh_prefs/evidence/`。
- [ ] **Step 3:** 回执 md/json 写明通过项、未验证项（Linux/Windows、发布体积）。重放 `qa/nsh_s2_shell/run.py`、`qa/nsh_s3_chat/run.py`，按新两栏结构修正其选择器。
- [ ] **Step 4:** commit `qa(panel): real-device receipt for two-column shell and preferences`

### Task 8: 文档与收尾

**Files:** `docs/reference/DESKTOP_SHELL.md`、`docs/reference/FEATURE_LOCATOR.md`、`docs/reference/SUBSYSTEM_ROUTING.md`、`docs/superpowers/specs/2026-11-22-nsh-panel-modular-refactor-design.md`（路线图记一笔）

- [ ] **Step 1:** 更新上述文档：两栏框架、`shared_ui_logic::prefs`、`PanelModel` 新方法、`aleph.fontScale`、存储键、命令 id 规则。
- [ ] **Step 2:** 全量回归：`cargo test -p shared-ui-logic`、`cargo test -p aleph-panel --lib`、`cargo test -p aleph-panel-ui`、`just panel-tsx-test`、`python3 scripts/doc_wiring_audit.py`（或仓库现有等价命令）。
- [ ] **Step 3:** 独立整支审阅（`gstack_pi_review`，不同模型），修复后 commit `docs(panel): two-column shell and preferences`。
- [ ] **Step 4:** 继续开放目标：校对 `FEATURE_LOCATOR.md` 与 reference 文档（m00001 的第二部分）。
