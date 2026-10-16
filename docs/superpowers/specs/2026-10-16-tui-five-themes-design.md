# Aleph TUI 五套主题设计 / Five-theme design

## Approved outcome / 已批准目标

The user approved all five browser studies and implementation, with Amber as the default. / 用户已认可五套视觉草案并授权实施，琥珀为默认主题。

- Amber / 琥珀: charcoal #24241f, foreground #ebe8df, accent #e7c777.
- Tide / 潮汐: deep blue #171f2b, foreground #dfe8f1, accent #87c5ef.
- Jade / 竹影: ink green #19231f, foreground #e0e9e2, accent #97d2b8.
- Iris / 鸢尾: graphite violet #23212b, foreground #ebe5f0, accent #c6acf0.
- Paper / 素笺: ivory #f3f0e8, foreground #33352f, accent #8b562c.

## Behavior / 行为

Themes change semantic transcript colors, tool surfaces, input rules, sticky prompts and status chrome together. Existing borderless transcript and compact 48-column sticky prompt stay intact. / 语义正文、工具块、输入线、吸顶提示与状态栏同步切换；保持无边框正文和 48 列紧凑吸顶提示。

`/theme` lists themes and current selection; `/theme <name>` validates, switches immediately and remembers the selection. Environment `ALEPH_TUI_THEME` overrides the remembered choice for the invocation. `dark`, `light`, and `terminal` remain compatibility names. / 命令支持列举、即时切换与记忆，环境变量优先，保留旧名称兼容。

A versioned JSON schema and checked-in examples form the public theme plugin API. Community authors distribute data files under `<ALEPH_HOME>/themes/<name>.json`; no executable plugin is required. Colors use semantic roles shared with Panel, with terminal chrome in a separate section. Built-ins use the same validation path. / 公开版本化 JSON Schema 与示例；社区主题通过数据文件接入，共用语义角色并单列终端样式；内置主题走同一校验路径。

Background defaults to terminal inheritance for dark themes to preserve transparency; Paper supplies its light surface so it remains readable on a dark terminal. Each theme can explicitly choose `terminal` or a color. Tool and selection surfaces may use opaque colors; terminal alpha is controlled by the terminal. / 深色主题默认继承终端背景以保留透明度；素笺显式提供浅色背景确保可读。主题可明确选择继承或指定背景，局部面板允许不透明色块。

Color capability detection recognizes modern terminal signals including Windows Terminal, with an explicit `ALEPH_TUI_COLOR` override and ANSI fallback. Invalid themes leave the current selection intact and produce a visible notice; invalid startup preferences fall back to Amber with a notice. / 识别 Windows Terminal 等现代终端，支持显式色深覆盖与 ANSI 降级；无效主题保留当前选择并显示原因，启动失败回退琥珀并提示。

## Implementation decisions / 实施决策

Themes are local display preferences, outside Core business state. No new dependencies, network installers, script execution, layout plugins or theme marketplace are added. Files are bounded and names cannot traverse directories. User themes cannot override built-in names. / 主题属于本机显示偏好；不增加 Core 业务状态、依赖、网络安装器、代码执行、布局插件或市场；限制文件大小、名称与内置名称覆盖。

Acceptance: all five palettes visibly reach real Ratatui buffers; custom theme load/switch/failure/persistence and capability selection are tested with temporary directories, then the rebuilt local executable is exercised. / 验收覆盖五套主题实际渲染、社区主题加载切换失败与记忆、终端色深，测试只使用临时目录，最后验证重建后的本地程序。
