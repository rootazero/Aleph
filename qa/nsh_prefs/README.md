# Two-column shell and preferences receipt / 两栏外壳与个人偏好回执

Real-device check of the two-column TSX Panel and its personal preferences
(appearance, shortcuts, command palette) on macOS Desktop (NSH shell over
Desktop IPC) and in Chrome (Web v1), against one isolated Gateway.

在 macOS 真机上验证两栏 TSX Panel 与个人偏好（外观、快捷键、命令面板）：Desktop（NSH 外壳经 Desktop IPC）与 Chrome（Web v1）共用一个隔离 Gateway。

- `run.py`: starts the Gateway (S2 fixture: temporary HOME/ALEPH_HOME, random
  port, self-signed TLS), runs the Desktop phases, then the Chrome driver.
  Writes `report.json`, logs and screenshots into `--out`.
  - `edit`: mod+k palette, Down×16 + Return runs "Open Capabilities"; mod+,
    opens Settings; clicks Mode Dark, Accent Ocean, Corners Round, Text size
    Large. Asserts the stored preferences.
  - `restored`: with density Cozy and overrides written while the shell is
    down (palette mod+j, scheme mod+2 which steals menu entry 2, reconnect
    mod+q which macOS reserves): mod+k does nothing, mod+j opens the palette,
    Settings shows the conflict and the reserved key.
  - `offline`: Gateway stopped; mod+, still opens Settings.
  - `chat`: Chat composer focused; letters typed under the input method stay
    in the field, mod+k still opens the palette (with Chat's "Stop reply").
- `web-prefs.cjs`: pairs; edits all five appearance axes and checks the root
  font size (20px for Large); reload restores them; palette filter + Return;
  rebinding, conflict, invalid text, reset; Settings with the Gateway stopped.
- `click.swift`: posts a mouse click at a screen point. System Events'
  `click at` goes through accessibility and does not reach NSH views.
- `evidence/`: the report files and downscaled screenshots of one run.

Desktop outcomes other than the stored preferences are checked by reading the
screenshots: NSH views expose no accessibility tree the script can query.
`SETTINGS_CLICKS` holds point offsets measured at the default text size; a
layout change in Settings moves them, and the storage assertion then fails.

Desktop 上除存储偏好外的结果靠截图核对（脚本无法查询 NSH 视图的无障碍树）。`SETTINGS_CLICKS` 是默认字号下量得的点坐标；设置页布局一变，存储断言就会失败。

```sh
just panel-tsx-bundle panel-tsx-web
cargo build --locked --bin aleph-server
cargo build --locked -p aleph-nsh-shell --features tsx-panel
swiftc -O -o /tmp/winlist qa/nsh_s2_shell/winlist.swift
swiftc -O -o /tmp/click qa/nsh_prefs/click.swift
python3 qa/nsh_prefs/run.py --server target/debug/aleph-server --shell target/debug/aleph-nsh-shell \
  --web "interfaces/panel/out-web/Aleph Panel" --winlist /tmp/winlist --click /tmp/click \
  --node-modules "$(git rev-parse --show-toplevel)/node_modules" --out /tmp/prefs-run
```

The terminal running it needs Accessibility (keystrokes, clicks) and Screen
Recording (window captures) permission. Do not touch the keyboard or mouse
while it runs.

运行它的终端需要“辅助功能”（按键、点击）与“屏幕录制”（窗口截图）权限；运行期间不要操作键鼠。
