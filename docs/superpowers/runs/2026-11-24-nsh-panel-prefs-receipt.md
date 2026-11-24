# Two-column shell and preferences receipt / 两栏外壳与个人偏好真机回执

## Scope / 范围

Real-device check of the work in `docs/superpowers/specs/2026-11-24-nsh-panel-prefs-design.md`
(plan `docs/superpowers/plans/2026-11-24-nsh-panel-prefs.md`, Task 7) on branch
`nsh-panel-prefs`: the fixed two-column shell, appearance preferences (mode, accent, text
size, corners, density), rebindable shortcuts, and the command palette, on macOS Desktop
(NSH shell, embedded TSX surface over Desktop IPC) and in Chrome (same TSX over Web v1).

在分支 `nsh-panel-prefs` 上对设计文档所述工作做真机验证（计划 Task 7）：固定两栏外壳、外观偏好（明暗、强调色、字号、圆角、密度）、可改绑快捷键与命令面板；macOS Desktop（NSH 外壳内嵌 TSX，经 Desktop IPC）与 Chrome（同一 TSX，经 Web v1）。

## Build under test / 被测构建

- Tested tree: `fd9cbc15f` (main `49fdf8090` untouched). / 被测提交 `fd9cbc15f`，main 未动。
- macOS 27.2.1 arm64, rustc 1.99.1, node 26.13.0, nsh 0.6.0 (5541637), Playwright Chromium.
- Input source during the run: `com.apple.inputmethod.SCIM.Shuangpin` (Chinese, Shuangpin). / 运行时输入法：简体双拼。
- `interfaces/panel` tree `b590b319f`; `bundle.js` sha256 `b74ca458…`; `capabilities.json` sha256 `765d5380…`.
- `aleph-server` sha256 `185316c6…`; `aleph-nsh-shell` (`--features tsx-panel`, debug) sha256 `36b350a0…`; Web `index.html` sha256 `a46dfb9f…`.

## Real-machine run / 真机运行

`python3 qa/nsh_prefs/run.py …` (see `qa/nsh_prefs/README.md`): one isolated Gateway (temporary
HOME/ALEPH_HOME, random port, self-signed TLS), all checks PASS, Gateway exit 0. Evidence:
`qa/nsh_prefs/evidence/` (report files and downscaled screenshots).

一个隔离 Gateway，全部通过，Gateway 退出码 0；证据在 `qa/nsh_prefs/evidence/`。

### Desktop / 桌面

Driven with System Events keystrokes and posted mouse clicks (`qa/nsh_prefs/click.swift`). Stored
preferences are asserted from the shell's storage file; the rest was checked on the screenshots.

| Check / 检查项 | Result / 结果 | Evidence / 证据 |
|---|---|---|
| mod+k opens the palette from launch, no click first / 启动后不点击即可 mod+k | PASS | `desktop-edit-palette.jpg` |
| Palette keyboard selection runs "Open Capabilities" / 键盘选择执行“打开 Capabilities” | PASS | `desktop-edit-palette-ran.jpg` |
| mod+, opens Settings; clicks on Dark, Ocean, Round, Large are applied and stored / 设置页点击生效并保存 | PASS (storage asserted) | `desktop-edit.jpg` |
| Relaunch restores dark, ocean, large text, round corners, cozy density / 重启后恢复 | PASS | `desktop-restored.jpg` |
| Palette rebound to mod+j: mod+k does nothing, mod+j opens it / 改绑生效、旧键失效 | PASS | `desktop-restored-mod-k.jpg`, `desktop-restored-mod-j.jpg` |
| mod+q (macOS reserves it) disables only that command; the row says "Reserved by the system" / 保留键只使该条失效 | PASS | `desktop-restored.jpg` |
| mod+2 on "Toggle light and dark" takes the key; "Open menu entry 2" row says "Conflicts with Toggle light and dark" / 冲突可见 | PASS | `desktop-restored.jpg`, `desktop-restored-mod-j.jpg` |
| Gateway stopped: mod+, opens Settings, preferences still applied / 断线时可打开设置 | PASS | `desktop-offline.jpg` |
| Letters typed into the Chat composer compose under Shuangpin and open nothing / 双拼下在输入框打字不触发快捷键 | PASS | `desktop-chat-typed.jpg` |
| mod+k from the focused composer opens the palette, with Chat's "Stop reply" / 输入框聚焦时 mod+k 打开面板，含 Chat 注册的命令 | PASS | `desktop-chat.jpg` |

### Web / Web（Chrome）

`qa/nsh_prefs/web-prefs.cjs`, all steps asserted in the driver. / 全部由驱动断言。

| Check / 检查项 | Result / 结果 | Evidence / 证据 |
|---|---|---|
| Pair; fresh browser has no stored preferences, root font size untouched / 首次无存储偏好 | PASS | `web-report.json` |
| All five axes edited from Settings; stored; root font size 20px for Large / 五个维度均可编辑，Large 根字号 20px | PASS | `web-appearance-edited.jpg` |
| Reload restores and re-applies them / 刷新后恢复 | PASS | `web-appearance-restored.jpg` |
| mod+k, type "capab", Return runs "Open Capabilities"; palette closes / 面板过滤并执行 | PASS | `web-palette-filtered.jpg` |
| Rebind palette to mod+j: stored as `{"key":"mod+j"}`, mod+k no longer opens it, mod+j does / 改绑 | PASS | `web-report.json` |
| Settings also on mod+j: its row says "Conflicts with Command palette" / 冲突提示 | PASS | `web-shortcut-conflict.jpg` |
| `mod+` refused as "Invalid shortcut", nothing stored / 非法键被拒 | PASS | `web-report.json` |
| Reset both: no overrides left, mod+k opens the palette again / 重置恢复默认 | PASS | `web-report.json` |
| Gateway stopped: Settings opens and still edits appearance (Mode light stored) / 断线时设置仍可用 | PASS | `web-offline-settings.jpg` |

### Earlier receipts replayed / 旧回执重放

- `qa/nsh_s2_shell/run.py`: PASS on Desktop and Web. Its Chrome driver now arranges the menu from
  Settings → Menu instead of the removed top-bar "Customize" button. / S2 重放通过；Chrome 驱动改为从设置页 Menu 调整菜单。
- `qa/nsh_s3_chat/run.py`: PASS unchanged. / S3 重放通过，无需修改。

## Found on device and fixed / 真机发现并修复

1. **Shortcuts were dead at launch on Desktop** (`f7acf56b8`). NSH routes a surface's shortcuts only
   while the window's focus is inside that surface, and the shell window gives its own root the
   focus at launch. A `focus()` at first render does nothing because the Panel is not in the
   window yet. The Panel root now takes the focus on its first sized layout.
   Desktop 启动时快捷键无效：NSH 只在焦点位于该 surface 内时分派快捷键，外壳窗口启动时焦点在自身根视图。Panel 根视图现在在首次有尺寸的布局时取得焦点。
2. **Shortcut fields showed `mod+k` as placeholder on every row** (`f7acf56b8`), which read as the
   current key. Now "New shortcut". / 每行快捷键输入框占位符都是 `mod+k`，易误读，改为 “New shortcut”。
3. **Clear, Reset, the menu editor arrows and Show/Hide drew near-black on the dark theme**
   (`fd9cbc15f`): their text had no colour class. / 深色主题下这些按钮文字几乎不可见，已补前景色。

## Observations, not fixed / 观察到但未修改

- A conflict goes to the later command in list order, not to the default. Rebinding "Toggle light
  and dark" (listed earlier) to mod+2 takes the key away from "Open menu entry 2", and that row
  says so. This is the rule as designed (`resolve_bindings`). The palette shows no key for the
  losing command, but Settings' key column still shows ⌘2 on its row above the conflict text.
  冲突按列表顺序归后者：把排在前面的命令改绑为 mod+2 会使“Open menu entry 2”失效，该行有提示。符合设计规则。
- After a disconnect, Web keeps the last menu and Settings → Menu stays editable (the layout is
  local); a Desktop launch with no Gateway shows an empty menu. / 断线后 Web 保留上次菜单；无 Gateway 启动的 Desktop 菜单为空。
- The pre-pairing `/v1/ws` attempt logs "HTTP Authentication failed" in the browser console, as in
  the S2 and S3 runs. / 配对前的 WebSocket 认证失败日志与 S2/S3 相同，属已有行为。
- Web has no default keys for "Open menu entry 1…9" (Chrome keeps mod+1…9); their rows show
  "None". / Web 上 mod+1…9 无默认键（Chrome 自用）。

## Not verified / 未验证

- Linux and Windows hosts; release builds and bundle size. / Linux、Windows 与发布体积。
- Headed Chrome: the run uses Playwright's headless Chromium, whose key events never reach browser
  menus, so it does not prove that a desktop Chrome window leaves mod+k and mod+, to the page.
  / 无头 Chromium 不经浏览器菜单，未证明真实 Chrome 窗口会把 mod+k、mod+, 交给页面。
- Typing a new shortcut into the Settings field on Desktop under the input method; Desktop
  overrides were written to storage, the field itself was exercised on Web. / Desktop 上未在输入法下手输新快捷键（改为写存储），输入框只在 Web 上验证。
- Text size on Desktop beyond Large, and the "Not supported here" path on a host without
  `aleph.fontScale` (covered by the TSX tests only). / 其他字号档与不支持字号的宿主仅有 TSX 测试覆盖。
- Community (DOM-rendered) module entries in the palette: the fixture has only the four built-in
  modules. / 夹具只有四个内置模块，未验证社区模块条目。
