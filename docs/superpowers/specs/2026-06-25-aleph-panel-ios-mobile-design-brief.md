# Aleph Panel — iOS / 移动端响应式设计简报

> 用途:指导「Claude 生成视觉稿 → 移动端响应式 Panel」工作流。
> 目标形态:让现有 Leptos/WASM panel(`interfaces/webchat/`,crate `aleph-panel`)在手机上自适应——**不是**重写原生 App。
> 防漂移原则:**移植设计系统(token),适配布局(iOS 范式)**。功能一致 + 视觉身份一致 ≠ 布局一致。

---

## 0. 核心约束

- 视觉系统已是现成 CSS(Tailwind v4 + OKLCH token),**不重写**;移动化工作量只在布局 / 导航 / 手机特性。
- 玻璃材质 `Liquid`(visionOS 风)天然贴合 iOS,建议手机默认 `Liquid` 或 `Luxe`。
- 连接形态:纯壳/远程 panel 通过 `location.host` + token 连 `aleph-server`(JSON-RPC over WS)。
- 红线:R2(UI 唯一源在 Leptos)、R4(Interface 纯 I/O)、R6(一核多端)。移动 panel 仍是纯 I/O 渲染层。

---

## 1. 分期(Phasing)

### Phase 1 — MVP「随身的 Aleph」
高频场景 = 对话 + 语音 + 主动到达。

| 模块 | 范围 | RPC 关键能力 |
|---|---|---|
| **Chat** | 消息流 / composer / slash / 附件 / 流式 token / session 切换 | `chat.send(stream)` `chat.history` `chat.abort` `sessions.*` |
| **Voice** | 语音输入 orb + 全屏沉浸态 | `voice.transcribe(delta)` `voice.synthesize` |
| **Memory 浏览** | Vault 列表/卡片 + 搜索(星系画布降 P2) | `memory.search` `memory.stats` |
| **Agents 切换** | 列表 + 设默认 + 只读 overview | `agents.list` `agents.get` `agents.set_default` |
| **关键设置** | 连接 / Providers 看与切 / Appearance | `config.*` `generation_providers.list` network/connection |
| **通知 + 审批** | 通知中心 + 审批弹层 | 订阅 `approval.**` `alerts.**` |

### Phase 2 — 管理下沉手机
- Dashboard(Home 活动流 / Logs / Usage / Agent Trace,只读监控)
- Cron(查看 + 触发 + 开关)
- 完整 Settings AI 组(embedding/rerank/generation/route/search/memory)+ Channels(Telegram/Discord/WhatsApp/iMessage)
- Extensions Hub(浏览 + 安装 flow)
- Memory 星系 WebGL 画布(移动优化版,触控手势,降节点数)

### Phase 3 — 桌面重场景重构
- Teams(Kanban 拖拽→触控 / Plan DAG / Replay / Workers)
- Agents 完整编辑(files / skills / channels / teams 成员)
- Advanced Settings(policies / security / execution / browser / ACP / routing rules)
- Split 工作区 → 底部可上拉 sheet

---

## 2. 信息架构 → 移动导航映射

桌面:底部 7 模式切换器 + 左 256px 侧栏 + 可选 Split 双栏。
移动:**底部 TabBar(4)+ 导航栈 push + sheet**。

| 桌面 | 移动 |
|---|---|
| 7 模式切换器 | 底部 TabBar:**Chat / Memory / Agents / Settings**(Dashboard 进 P2,可作第 5 tab 或 Settings 内入口) |
| 侧栏 agent/session 列表 | Chat 顶部标题下拉(切 agent)+ 左滑/顶栏按钮唤出 session sheet |
| Settings 左栏 6 组 27 页 | iOS Grouped List,点组 → push 子页 |
| ⌘K 命令面板 | 顶部搜索 + 语音入口 |
| Split 工作区(工具活动/任务条/时间线) | 底部可上拉 sheet 或从消息 push 详情页 |
| 通知 bell + popover | 顶栏 bell → 全屏/半屏通知页 |

---

## 3. 设计系统 / Design Tokens(精确值,源:`interfaces/webchat/styles/tailwind.css` + `src/appearance.rs`)

### 3.1 颜色(OKLCH)
**亮色**
```
surface           oklch(0.96 0.005 220)   surface-raised  oklch(1.00 0 0)
surface-sunken    oklch(0.905 0.010 220)  surface-overlay oklch(0.985 0.004 220)
sidebar           oklch(0.99 0.003 220)   sidebar-active  oklch(0.95 0.015 310)
text-primary      oklch(0.20 0.015 310)   text-secondary  oklch(0.40 0.010 220)
text-tertiary     oklch(0.48 0.008 220)   text-inverse    oklch(0.97 0.005 220)
border            oklch(0.86 0.009 220)   border-subtle   oklch(0.91 0.006 220)
primary(mauve)    oklch(0.55 0.120 310)   primary-hover   oklch(0.50 0.110 310)
success           oklch(0.55 0.120 130)   warning         oklch(0.60 0.080 70)
danger            oklch(0.55 0.150 25)    info            oklch(0.50 0.030 220)
```
**暗色**
```
surface           oklch(0.15 0.020 310)   surface-raised  oklch(0.225 0.022 310)
sidebar           oklch(0.13 0.025 310)   sidebar-active  oklch(0.22 0.035 310)
text-primary      oklch(0.97 0.005 220)   text-secondary  oklch(0.68 0.008 220)
border            oklch(0.31 0.020 310)
primary(mauve)    oklch(0.65 0.120 310)   primary-hover   oklch(0.70 0.110 310)
```

### 3.2 Accent 调色板(5 套,只重染 primary/focus/sidebar)
```
Mauve(默认)  oklch(0.60 0.13 310)
Ocean         oklch(0.58 0.13 250)
Forest        oklch(0.55 0.12 150)
Sunset        oklch(0.66 0.135 60)
Rose          oklch(0.62 0.15 15)
```

### 3.3 玻璃材质(3 套,`data-material`,移动建议 Liquid 默认)
- **Luxe**(默认):内敛磨砂,blur 20px / saturate 1.6
- **Liquid**(visionOS 风,推荐手机):blur 34px / saturate 2.0,明亮高光边
- **Aurora**:厚奶白霜,色彩透出,blur 26px / saturate 1.5

### 3.4 字体
```
sans  "Inter","Inter Variable",-apple-system,BlinkMacSystemFont,"SF Pro Text",system-ui,…
mono  "JetBrains Mono",ui-monospace,SFMono-Regular,"SF Mono",Menlo,…
serif "Fraunces","Noto Sans SC",Georgia,serif
body  font-size 0.8125rem(13px) / line-height 1.5 / letter-spacing -0.005em
root  font-size calc(16px * --control-ui-text-scale)
标题   letter-spacing -0.02em / text-wrap balance
```

### 3.5 间距 / 圆角 / 阴影 / 动效
```
间距基准  --spacing = calc(0.22rem * --control-ui-density)  ≈ 3.52px(比 TW 默认紧 12%)
圆角     xs 3 / sm 6 / md 8 / lg 12 / xl 16 / 2xl 20 / 3xl 28 / full 9999  (× --control-ui-radius-scale)
阴影     mauve-tinted 多层柔阴影(xs→xl);focus-ring = 2px surface + 4px accent
动效     timing cubic-bezier(0.32,0.72,0,1) / 180ms;ease-out cubic-bezier(0.16,1,0.3,1)
```

### 3.6 六根可调外观轴(全部保留,`localStorage` 持久)
```
1 主题   System(默认)/ Light / Dark
2 Accent Mauve(默认)/ Ocean / Forest / Sunset / Rose
3 材质   Luxe(默认)/ Liquid / Aurora
4 字号   Compact .9 / Default 1 / Cozy 1.1 / Large 1.25 / Largest 1.4
5 圆角   Sharp 0 / Slight .5 / Default 1 / Round 1.5 / Extra 2
6 密度   Compact 1 / Cozy 1.13 / Spacious 1.25
```

### 3.7 图标
内联 SVG,`stroke="currentColor"`,stroke-width 1.8,round cap/join,继承文字色。

---

## 4. 移动端专属规则

- **安全区**:适配刘海 / Home indicator(`env(safe-area-inset-*)`);TabBar 贴底安全区。
- **拇指可达**:主操作(发送 / 语音 / 新建)放底部;次要操作放顶部。
- **点击区** ≥ 44pt。
- **键盘避让**:composer 随键盘上浮,消息流自动滚到底。
- **手势**:左滑返回;Chat 左滑唤 session sheet;长按消息 → 操作菜单。
- **降级**:WebGL 星系画布(MVP 用列表);Kanban 拖拽(P3 改触控);Split 双栏(改 sheet)。
- **响应式断点**:`< 640px` 单列 + TabBar;`≥ 768px` 渐进恢复侧栏(平板/横屏)。

---

## 5. MVP 各屏规格(给视觉稿)

1. **Chat(主屏)**:顶栏(agent 名下拉 + 通知 bell)/ 消息气泡流(user 用 primary 玻璃气泡,AI 用中性玻璃,含代码块/工具调用/reasoning 折叠)/ 底部 composer(输入框 + slash 触发 + 附件 + 语音 orb + 发送)。流式打字指示。
2. **Voice 沉浸态**:全屏,中心语音 orb(morph/hue 动效),实时字幕,底部结束按钮。
3. **Memory(Vault)**:搜索栏 + 结果卡片列表(每卡:标题 / 摘要 / 类型徽章 / 时间),点 → 详情 sheet。顶部 facet 筛选 chip。
4. **Agents**:agent 列表(头像/名/默认徽章),点 → 只读 overview(身份 + soul),右上「设默认」。
5. **Settings(分组列表)**:Grouped List(Basic / AI / Channels / …),点 → 子页。MVP 子页:连接、Providers(列表 + active 切换)、Appearance(主题/accent/材质/字号/圆角/密度 swatch)。
6. **通知中心**:列表(成功/警告/危险/信息徽章)+ 审批请求卡(批准/拒绝按钮)。

---

## 6. 明确不做 / 延后(防 MVP 膨胀)

- 星系 WebGL 画布、Teams 全套、Agents 编辑(files/skills/channels)、Advanced Settings、Cron 编辑、Extensions 安装 → P2/P3。
- Split 双栏工作区在手机不存在。

---

## 7. 视觉稿生成方向(给 imagegen-frontend-mobile)

- 统一调色板:**亮色 Mauve accent + Liquid 材质**为主稿(可附暗色变体)。
- 每屏放进 iPhone mockup 边框,内容为主。
- 字体观感:Inter / SF;数字用 tabular。
- 保持桌面版的克制、玻璃质感、柔阴影、紧凑密度。
- 一屏一图:Chat / Voice / Memory / Agents / Settings / 通知,共 6 张 MVP 主稿。
