# NSH Panel 两栏外壳与个人偏好设计 / Two-column shell and personal preferences

> 状态 / Status: 草案，待用户批准 / Draft, awaiting approval
> 分支 / Branch: `nsh-panel-prefs`（worktree `.worktrees/nsh-panel-prefs`，基于 main `49fdf8090`）
> 上游设计 / Parent design: `docs/superpowers/specs/2026-11-22-nsh-panel-modular-refactor-design.md`（§4 可定制契约）

## 1. 意图 / Intent

NSH Panel 的第五轮迭代。S1–S3 已合入 main，分别交付传输层、可定制外壳和会话纵切。目前 TSX Panel 只有一条顶部标签栏，同一时间只挂载一个模块，用户能做的定制只有排序、隐藏和选中。

用户的构想（2026-11-24 对话）：

- 参考 deepseek-harness，同时参考旧 Tauri/Leptos Panel 的样子。
- **左右两栏的结构固定不动**：左栏是菜单，右栏是选中的内容页。用户和插件都不能改这个框架。
- **插件通过往菜单里注册条目来扩展**，社区的能力在这个框架内发挥。
- **这一轮先做个人偏好**：主题、快捷键、命令面板。
- 偏好存在客户端本地（用户选了方案 A）。

为什么做：让 Panel 从一个能用的标签栏，变成用户能按自己习惯调整的工作台，同时给后续社区模块留好入口（菜单项、命令）。

## 2. 目标 / 非目标 / 反目标 / Goals, non-goals, anti-goals

**目标 / Goals**

1. 两栏外壳：左栏菜单列出快照里的模块，可排序、可隐藏（沿用 `PanelLayout` v1）。左栏底部固定放“设置”入口。右栏挂载选中的模块。两栏框架、主题和快捷键在任何连接状态下都在；断线时右栏显示离线/配对页，设置照样能打开。
2. 外观偏好：明暗（跟随系统/浅/深）、强调色（5 种）、字号、圆角、密度。Desktop（嵌入 AppView）和 Web（wasm 包）用同一份 TSX、同一份 Rust 规则。
3. 快捷键：命令有默认快捷键，用户可以改绑、清除、恢复默认，并能看到冲突。
4. 命令面板：`mod+k` 打开，模糊搜索，分组显示。命令来源有三个：外壳自带、菜单项（每个可用模块一个“打开 X”）、当前挂载模块经 SDK 注册的命令（返回释放函数，模块卸载时自动注销）。
5. 外观、快捷键、命令打分的规则放进 `shared-ui-logic` 的新模块 `prefs`（不受 `panel` feature 门控）。Leptos Panel 改用其中的外观枚举和 `score`，删掉自己那份重复实现。Leptos 没有改绑功能（`state/hotkey.rs` 只有固定键），所以绑定解析只有 TSX 使用。

**非目标 / Non-goals（本轮不做）**

- 跨设备同步偏好（方案 B）。存储读写集中在一处，以后接 `config.get/patch` 只需改这一处。
- 社区 descriptor 声明命令或默认快捷键（`PanelTabDescriptor` 不扩展字段）。插件这一轮通过“菜单项 + 自动生成的打开命令”进入命令面板。
- 可拖拽、可拆分的多窗格布局；右侧检查面板（旧 Leptos `RightPane`）。
- 玻璃材质（`Material`）。NSH 原生渲染没有对应能力，TSX Panel 不提供这一项。Leptos 照旧保留。
- 自定义主题文件、VS Code 主题导入（t3code 有）。
- S4 恢复闸门（断线补读）。照路线图后延。

**反目标 / Anti-goals（这样做就算失败）**

- 用户或插件能移动、拆分、删除两栏框架本身。
- 布局或偏好授予权限、启动被隐藏的后端，或者让 Core 的行为改变。
- 同一条规则在 Rust 和 TSX（或 Leptos 和 shared）各存一份，例如两套命令打分、两套强调色取值且没有一致性检查。
- 插件命令在模块卸载后仍可执行，或在换代（新连接）后执行到旧 SDK。
- 快捷键在文本框里吞掉用户正常输入（例如裸字母快捷键）。

## 3. 约束 / Constraints

- R2：复杂业务 UI 只有一份共享呈现源；Core 是唯一业务事实与授权权威。
- R3：不引入新的重依赖。不引入状态框架（Zustand 一类）。
- 上游设计 §4.4：偏好只保存稳定 ID 与纯数据；损坏或旧版数据只重置表现层。
- 桌面嵌入 QuickJS 没有 `crypto` 全局（S3 真机发现），`nsh test` 运行时不等于桌面运行时，必须做真机验证。
- NSH API（0.6.0，已核对 `.nsh/types` 与 crates 源码）：
  - `setTheme(theme)`：运行中替换主题 token。读不懂的值抛 `TypeError`，当前主题不变。
  - `setColorScheme("light"|"dark"|"system")`。
  - `keymap.bind(keystroke | record, handler)` 返回解绑函数，组件卸载时自动解绑。非法或系统保留的组合键（macOS：⌘Q、⌘H、⌥⌘H、⌘M、⌃⌘F；其他平台：Ctrl+Q、Alt+F4、F11）**只在 `bind` 时**抛错（`nsh-core-0.6.0/src/keymap.rs:429-451`）。`keymap.label(keystroke)` 返回平台显示文本，只在语法非法时抛错，查不出系统保留键。
  - 输入框有焦点时，默认不触发该输入框自己要用的按键（`KeyBindingOptions.when`，`Keystroke::edits_text`）。方向键、回车、Esc 都算文本键。
  - Web 端 `reserved()` 返回 `None`，因为浏览器自己留下的组合键页面收不到（`keymap.rs:449-450`）。例如 Chrome 里 `mod+1…9` 切换浏览器标签页。
  - `setTheme` 接受 `oklch()`（`nsh-core-0.6.0/src/color.rs:32,233`；默认主题就用 oklch）。`Theme.cssVars` 同时含 `light` 和 `dark` 两段，由 NSH 按当前明暗选用。
  - `AppView::set_font_scale` 只放大文字，长度不变（`nsh-platform-macos-0.6.0/src/app.rs:562-568`）。宿主 host 函数是 `Send + Sync`，在引擎线程运行（`nsh-engine-0.6.0/src/host_channel.rs:162-172`），而 `AppView` 是只能在主线程碰的 `Rc`。
- 仓库约束：`shared/ui_logic/src/lib.rs:3-4` 中 `pub mod panel` 只在 `panel` feature 下编译；webchat 只有可选的 `panel-modular` 打开它，`just wasm` 的发布构建不打开。TSX 只能通过 `aleph/panel` 绑定（`interfaces/panel/src/lib.rs` 的 `PanelModel`，JSON 进出）调用 Rust。`host.call` 只允许 `nsh.json` 的 `capabilities.host` 里列出的名字。
  - `fontScale` 在 TSX 中只读。宿主侧有 `nsh_platform_macos::AppView::set_font_scale(Option<f32>)`（`nsh-platform-macos-0.6.0/src/app.rs:569`）。
  - 没有原始按键事件 API，无法像 deepseek-harness 那样直接“录制”按键。

## 4. 方案 / Approaches

**偏好存储（用户已选 A）**

| 方案 | 做法 | 代价 |
|---|---|---|
| **A. 客户端本地（已选）** | 规则进 `shared-ui-logic`，经 NSH `Storage` 存在本机 | 换设备不跟随；Desktop 与 Web 各存一份 |
| B. Core 配置 | 新增 `ui` 配置段，经 `config.get/patch` 读写 | 要改 Core 配置结构、给 v1 开放 `config.patch` 并审权限；范围约翻倍 |

选 A 的理由：沿用 S2 已验证的布局存储方式；存储读写集中在一处，日后切 B 不返工。

**两栏框架**：用户已定（固定左右两栏、插件注册进菜单），不再比较。

**快捷键改绑交互**（我的决定）：NSH 没有原始按键事件，所以不做“按下即录制”。改为在输入框里写组合键文本（如 `mod+shift+p`），用 `keymap.label` 校验并实时显示平台写法（`⇧⌘P`）。代价：比录制麻烦一些。日后 NSH 提供按键事件再换。

**外观 token 来源**（我的决定）：强调色等取值以 Rust 数据表为准（`shared_ui_logic::prefs`），TSX 通过 `setTheme` 应用。Leptos 仍用 `styles/tailwind.css` 里的 CSS 变量，因为它的 Tailwind 管线依赖 CSS。两份取值之间加一个单元测试防止漂移（Shape #1）。测试放在拥有 CSS 的 webchat crate（`include_str!`），不让 `shared-ui-logic` 去读别的 crate 的文件。不做 CSS 代码生成：那要改 Leptos 构建管线，收益不值。

两边的变量不是一一对应，所以按下表比对（mauve 的取值在基础 `@theme` 块，`tailwind.css:44,296`；其余在 `html[data-accent=…]` 块，`:362-396`）：

| Rust 表字段 | NSH token | Leptos 变量 |
|---|---|---|
| `primary` | `primary`、`sidebar-primary`、`ring`、`chart-1` | `--color-primary`、`--color-chart-1`、`--color-ring`（Leptos 另加 `/ 0.35` 透明度，比对时去掉） |
| `selected` | `sidebar-accent` | `--color-sidebar-active` |
| 固定 `oklch(0.985 0 0)` | `primary-foreground`、`sidebar-primary-foreground` | —（Leptos 无此变量，不比对） |
| —（只在 Leptos） | — | `--color-primary-hover`、`--color-primary-subtle`、`--color-sidebar-accent`：不进 Rust 表，不比对 |

名字交叉：Leptos 的 `--color-sidebar-active` 是选中项的浅色底，对应 shadcn 的 `sidebar-accent`（悬停/选中底色）；Leptos 的 `--color-sidebar-accent` 是强调色本身，与 `primary` 接近但个别档位不同，不映射。前景色固定为近白，因为强调色都是中等明度，NSH 默认主题在深色下的 `primary-foreground` 是近黑。（2026-11-24 计划阶段核对 `tailwind.css` 后修正。）

## 5. 设计 / Design

### 5.1 两栏外壳 / Shell

```
┌────────────┬────────────────────────────────────┐
│ Aleph  ●   │                                    │
│            │                                    │
│ ▸ Overview │                                    │
│   Sessions │         选中模块的内容页             │
│   Chat     │         (ModuleSlot)               │
│   Capab.   │                                    │
│   Notes ⚠  │   ← 社区模块；不可用时灰显并说明     │
│            │                                    │
│ ─────────  │                                    │
│ ⚙ 设置     │                                    │
└────────────┴────────────────────────────────────┘
```

- 左栏菜单项 = `PanelModel.view().slots`，顺序与隐藏沿用 `PanelLayout` v1，不改存储格式。
- 顶部是连接状态点，替代现在标签栏里的状态文字。
- 框架在所有连接状态下都挂载。现在 `Shell.tsx:124` 用 `<If when={connected} else={<Offline/>}>` 包住整个界面，改为只包右栏：断线时右栏显示 `Offline`，左栏只显示状态点和“设置”（菜单项来自快照，没有连接就没有菜单项）。
- “设置”是外壳自带的固定入口，不属于 tab registry，因此不能被隐藏或被社区模块替换（对应 deepseek-harness 的 `sidebar.settings` 位置）。设置页有三个分区：外观、快捷键、菜单（即现在的 `LayoutEditor`，从顶部挪进来）。
- 窄窗口（`horizontalSizeClass() === "compact"`）时左栏收成只显示图标的窄条。这是同一框架的紧凑形态，不是用户可拖动的布局。
- 右栏的挂载规则不变：仍按 `(tab key, renderer, generation)` 挂载与释放模块。

### 5.2 偏好模型 / Preferences model（`shared_ui_logic::prefs`）

新模块不受 `panel` feature 门控，不依赖 NSH 或 web-sys，因此 Leptos 的默认构建（`just wasm`）也能用。

```rust
pub struct PanelPrefs {            // stored JSON, version 1
    pub version: u32,
    pub appearance: Appearance,    // mode, accent, font_scale, roundness, density
    pub keybindings: BTreeMap<CommandId, Binding>, // user overrides only
}
pub enum Binding { Key(String), Cleared }   // absent = default
```

- 枚举（`ThemeMode`、`Accent`、`FontScale`、`Roundness`、`Density`）从 `interfaces/webchat/src/appearance.rs` 下移，连同它们的存储值解析（含旧值兼容，如 `"glass"` → `Dark`）。i18n 标签留在各端。
- `restore(raw) -> RestoredPrefs { prefs, repair }`：与 `layout::restore` 同形，损坏或未知版本时回默认并报告 `repair`。
- `theme_tokens(&Appearance) -> Theme`：一次生成 `cssVars.light` 和 `cssVars.dark` 两段，交给 `setTheme`；明暗由 `setColorScheme` 选。不把明暗作为参数，否则“跟随系统”每次系统切换都要重新 `setTheme`，会闪。强调色按 §4 的表映射；圆角 = NSH 默认 `0.625rem` × Leptos 同档倍数，Sharp 为 0。
- 存储键（我的决定）：`aleph.panel.prefs.v1`，不按端点或 subject 分。Desktop 上等于按设备存：切换 Gateway 不换主题。Web 上 NSH 用 `localStorage`，按源隔离，每个 Gateway 从自己的源提供 `/v1/panel/`，所以实际是按 Gateway 存。接受这个差别，不另做跨源存储。布局仍按端点与 subject 分，因为它依赖该 Gateway 提供哪些模块。
- TSX 经 `PanelModel` 新方法调用这些规则（JSON 进出）：`prefs_restore`、`theme_tokens`、`resolve_bindings`、`score`、`normalize_keystroke`。改完重新生成 `.nsh/types/aleph/*.d.ts`（本 worktree 里还没有这个目录，要先生成）。
- Leptos 继续用自己原有的 localStorage 键（`aleph-theme` 等），只把枚举和解析换成 shared 里的版本。两端不共享存储（来源不同，Web TSX 在 `/v1/panel/`）。

### 5.3 外观应用 / Applying appearance

| 维度 | Desktop（AppView） | Web（wasm） |
|---|---|---|
| 明暗 | `setColorScheme` | `setColorScheme` |
| 强调色、圆角 | `setTheme(theme_tokens(...))` | 同左 |
| 字号 | `host.call("aleph.fontScale", n)` → `panel_surface` 注册的处理函数 → 主线程 → `AppView::set_font_scale` | `web/host.js` 处理同名调用，设置根元素字号（NSH Web 播放器由它推出 `fontScale`，`nsh.js:2111-2121`） |
| 密度 | Panel 自己的间距 token；目前只作用于外壳（侧栏与设置页），内置模块尚未使用 | 同左 |

Desktop 字号这条路要改四处：

1. `interfaces/panel/nsh.json` 的 `capabilities.host` 加 `aleph.fontScale`（否则 `host.call` 返回 `ERR_ACCESS_DENIED`）。
2. `desktop/nsh-shell/src/panel_surface.rs` 在同一个 `HostChannel` 上注册这个处理函数，表面释放时移除。不放进 `panel_host.rs` 的 `FUNCTIONS`：那里管 Gateway 连接，而字号属于持有 `AppView` 的表面（计划阶段修正）。
3. `interfaces/panel/web/host.js` 的 `switch`（`:231-241`）加同名分支。
4. 主线程交接：处理函数在引擎线程运行，`panel_host::Handle` 又在 `AppView` 创建之前启动（`panel_surface.rs:85-97`）。因此 `panel_surface` 在 `AppView::new` 之后把 `Weak<AppView>` 放进主线程的 thread-local，处理函数把字号投递到主线程，由主线程取 `Weak` 调用 `set_font_scale`。

字号的含义在两种渲染器里不同：NSH 只放大文字，间距不变；Leptos 的 `FontScale` 放大所有 `rem`，间距也跟着变（`appearance.rs:245`）。同一档位在两端看起来不一样，设置页不做额外补偿。

偏好是异步读取的。为了不先用默认样式画一帧再跳变，外壳读完偏好、应用主题和字号之后才渲染框架，读取期间只显示背景色。

宿主不支持某一项时，设置页把这一项标成“此端不支持”，不假装已生效。

### 5.4 命令与快捷键 / Commands and keybindings

- 命令 = `{ id, label, group, keywords, defaultKey?, run }`。规则在 `shared_ui_logic::prefs`（组合键规范化例外，见下）：
  - `score(query, label, keywords)`：从 `components/command_palette.rs` 下移，Leptos 改为调用它。
  - `resolve_bindings(commands, overrides) -> { bindings, conflicts }`：用户覆盖优先于默认；两个命令落到同一组合键时报告冲突，后注册者不生效并在设置页标出。
  - `normalize_keystroke`：放在 `interfaces/panel` 的 `PanelModel` 里，用 `nsh_core::keymap::Keystroke` 解析成规范文本，与 NSH 的解析保持一致（`cmd+K` 与 `mod+k` 在 macOS 上是同一个键）。`shared-ui-logic` 不依赖 NSH，`resolve_bindings` 只比较规范化之后的文本。
- 命令来源：
  1. 外壳：打开命令面板（`mod+k`）、打开设置（`mod+,`）、切换明暗、重新连接、菜单第 1–9 项（`mod+1…9`）。
  2. 菜单：每个可用模块一个“打开 X”命令，由快照自动生成。社区模块因此自动进入命令面板，无需改协议。
  3. 模块：`PanelSdk.commands.register(spec)` 返回释放函数；id 自动加上模块 tab key 前缀（如 `aleph:chat/abort`），防止冒名。SDK `dispose()` 或换代时全部注销，与现有监听释放同一机制（`createSdk` 的 `hold`）。本轮内置的 Chat 注册“停止生成”（包装现有的 Stop，`Chat.tsx:160`）作为首个消费者。不做“新会话”：Chat 目前没有这个动作，做它就是新功能。
- 绑定：**每个组合键单独 `keymap.bind`**，各自 `try/catch`。某个键被系统保留或非法时，只有这一条失效，设置页标成“系统保留”或“无效”，其余快捷键照常工作（一次性绑定整张表时，一条 `mod+q` 就会让 `mod+k` 也失效）。绑定表变化时先全部解绑再重绑。默认快捷键都带修饰键；输入框里不吞字由 NSH 的 `when` 默认值保证。
- 默认快捷键按端区分：浏览器自己留下的组合键（如 Chrome 的 `mod+1…9`）在 Web 端没有默认绑定，用户可以自己改绑。具体哪些被浏览器留下，以真机验证为准。
- 命令面板：用 NSH 弹层呈现，上下键选择，回车执行，Esc 关闭。搜索框有焦点时这些键算文本键，所以面板的导航绑定用 `when: "always"`。

### 5.5 清理 / Cleanup

- `interfaces/webchat/src/appearance.rs`：删除下移到 shared 的枚举与解析，保留 DOM 应用和 i18n 标签。
- `interfaces/webchat/src/components/command_palette.rs`：删除 `score` 及其测试（测试随实现迁到 shared）。
- `interfaces/panel/shell/Shell.tsx`：顶层的连接状态 `If` 只包右栏。
- `interfaces/panel/shell/Shell.tsx`：删除顶部标签栏与 “Customize” 按钮。`LayoutEditor` 挪进设置页。

## 6. 验证 / Acceptance

- Rust 单元测试：prefs 恢复（损坏、旧版、未知字段）；`theme_tokens`；强调色表与 `tailwind.css` 一致；绑定解析与冲突；打分（原测试迁移）。
- TSX 测试（`App.test.tsx`、`sdk.test.ts`）：模块命令随 dispose/换代注销；菜单命令随快照变化；设置页改动持久化。
- 真机（macOS Desktop + Chrome Web，同一 Gateway）：切换明暗/强调色/圆角/字号后重启仍保留；断线时设置仍可打开；改绑快捷键生效、冲突可见；改绑成系统保留键时只有这一条失效；Web 端逐个核对默认快捷键是否能收到；`mod+k` 打开面板并执行社区模块的“打开”命令；文本框中输入不被快捷键吞掉；中文输入法下 `mod+k` 不误触。证据落 `docs/superpowers/runs/` 回执。
- 回归：`just wasm`（Leptos）、`cargo test -p shared-ui-logic`、Panel lib 测试、S2/S3 QA 装置重放。

## 7. 留给实现者 / Left to the builder

- 第一步 spike：Web 端设置根元素字号是否只影响 Panel、没有别的副作用；`keymap.bind` 在嵌入式 AppView 中能否收到 `mod+k`、`mod+,`（宿主窗口菜单可能先截获）。
- 左栏宽度、图标、间距等视觉细节；密度的具体档位数值。
- 命令面板在窄窗口中的尺寸。
