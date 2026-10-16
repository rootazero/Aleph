# TUI theme plugin API / TUI 主题插件接口

Amber (琥珀) is the default. Tide (潮汐), Jade (竹影), Iris (鸢尾) and Paper (素笺) are also built in. Themes are local appearance preferences; they do not change Gateway or conversation state. / 默认使用琥珀，另有潮汐、竹影、鸢尾与素笺。主题属于本机外观偏好，不改变 Gateway 或会话状态。

## Use / 使用

- `/theme` or `/theme list`: show the current name, available themes and color depth. / 查看当前名称、可用主题和色深。
- `/theme jade`: switch immediately and remember the choice. / 立即切换并记忆选择。
- `/theme reload`: validate and reload the current file after editing. On failure, the active palette remains in use. / 编辑后校验并重载当前文件；失败时保留现有配色。

Startup precedence is `ALEPH_TUI_THEME`, then `<ALEPH_HOME>/tui-theme`, then `amber`. If a candidate is invalid, startup reports it in the transcript and tries the next candidate. A failed preference write does not undo a successful switch; the notice explains that the change applies to this invocation. / 启动顺序为环境变量、偏好文件、琥珀；无效候选会在正文提示并尝试下一项。保存失败不撤销已完成的切换，提示会说明仅本次生效。

Legacy names `dark`, `light`, and `terminal` remain accepted. `dark` maps to Amber and `light` to Paper; `terminal` leaves colors to the terminal. / 保留旧名称兼容：深色映射琥珀、浅色映射素笺，terminal 交由终端配色。

## Install a community theme / 安装社区主题

Copy a UTF-8 JSON file into `<ALEPH_HOME>/themes/<name>.json`, then run `/theme <name>`. `ALEPH_HOME` defaults to the normal Aleph home directory (usually `~/.aleph`). No server restart or code execution is needed. / 将 UTF-8 JSON 文件复制到上述目录并执行切换命令；未设置时使用默认 Aleph 主目录（通常为 `~/.aleph`），无需重启服务或运行代码。

Names match `[a-z][a-z0-9-]{0,63}`. Built-in and legacy names are reserved and cannot be shadowed; command names `list` and `reload` are reserved too. The document name must match its filename. Each file is limited to 128 KiB. Invalid files are omitted from the list with diagnostics. / 名称遵循上述规则，内置名称和兼容别名不可覆盖，命令名称 `list` 和 `reload` 同样保留，文件名与文档名称须一致，单文件上限 128 KiB；无效文件会被跳过并提示原因。

```json
{
  "schemaVersion": 1,
  "name": "linen",
  "extends": "paper",
  "colors": {
    "accent": "#8b562c",
    "toolOk": "#34714d"
  },
  "chrome": {
    "userBg": "#e8e4d9",
    "borderFocused": "#8b562c"
  }
}
```

The [JSON Schema](../../interfaces/tui/themes/schema.json), [complete built-in files](../../interfaces/tui/themes/) and [community example](../../interfaces/tui/themes/example.json) are the public authoring contract. Themes are data-only plugins; distribute these files through your repository or package of choice. / JSON Schema、完整内置文件与社区示例共同构成公开创作接口；主题为纯数据插件，可通过自己的仓库或包发布。

## Schema version 1 / 版本 1

Required top-level fields are `schemaVersion: 1` and `name`. Optional fields are `extends`, `light`, `colors`, and `chrome`. Unknown fields, duplicate fields, null values, unsupported versions, and invalid colors are rejected. / 顶层必填版本与名称，其余字段可选；未知字段、重复字段、null、未知版本和无效颜色均拒绝加载。

`extends` selects one built-in palette and defaults to `amber`. Community-to-community inheritance is not supported. Omitted roles inherit the base theme. `light` optionally selects light/dark syntax highlighting; otherwise the loader derives it from the background or base theme. / extends 选择内置基底，默认琥珀，不支持社区主题相互继承；缺省角色继承基底，light 可指定语法高亮明暗，否则依据背景或基底推导。

Every color accepts `"#rrggbb"`, `"terminal"` (inherit terminal color), or an integer from 0 through 255 (ANSI index). / 颜色接受六位十六进制、继承终端或 0 至 255 的 ANSI 索引。

| Section / 部分 | Keys / 字段 |
| --- | --- |
| `colors` text / 正文 | `fg`, `dim`, `accent`, `prompt`, `userBar`, `link`, `spinner` |
| `colors` tools / 工具 | `toolPending`, `toolRunning`, `toolOk`, `toolErr`, `toolRail` |
| `colors` diff / 差异 | `diffAdd`, `diffDel`, `diffCtx`, `diffGutter`, `codeBorder` |
| `colors` notices / 提示 | `admonitionNote`, `admonitionTip`, `admonitionImportant`, `admonitionWarning`, `admonitionCaution` |
| `colors` status / 状态 | `contextOk`, `contextWarn`, `contextDanger`, `cost` |
| `chrome` / 界面 | `background`, `surface`, `selectionBg`, `userBg`, `border`, `borderFocused` |

Status foreground derives from `dim` and status background from `background`. Semantic roles are shared with the transcript model; terminal chrome is TUI-specific. Layout, event handlers, executable hooks, network resources, and font selection are outside this interface. / 状态栏前景来自 dim，背景来自 background；语义角色与正文模型共用，终端界面样式单列。布局、事件处理、可执行钩子、网络资源和字体不属于此接口。

## Terminal behavior / 终端行为

The four dark themes inherit the terminal background so transparency remains available. Paper paints an ivory background for readability on dark terminals. Tool and selection surfaces can paint solid cell colors. Per-cell opacity is not portable; use terminal settings for transparency. / 四套深色主题继承终端背景以保留透明度；素笺显式绘制象牙白以兼容深色终端。工具和选择区域可绘制实色，逐单元格透明度不可移植，透明度仍由终端设置。

Windows Terminal (`WT_SESSION`), `COLORTERM=truecolor/24bit` and known direct-color terminal types enable RGB. `ALEPH_TUI_COLOR=truecolor` forces RGB; `ansi` or `256` forces indexed fallback. Indexed fallback preserves distinct palette accents. / 识别 Windows Terminal、COLORTERM 和已知直彩终端；可通过环境变量强制 RGB 或索引降级，索引模式仍保留主题强调色区别。

A theme switch refreshes cached styled transcript lines while preserving conversation content, draft, cursor and viewport identity. The borderless transcript and compact sticky user prompt (maximum 48 columns, `[click to jump]`) retain their existing geometry. / 切换会刷新正文样式缓存，同时保留会话内容、草稿、光标及视口锚点；保持无边框正文和最多 48 列的紧凑吸顶提示及其跳转文字。
