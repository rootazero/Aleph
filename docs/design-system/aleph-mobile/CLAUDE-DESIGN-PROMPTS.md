# Claude Design 提示词包 — Aleph iOS MVP 6 屏

> 用法:在 claude.ai/design 的 **"Design System"** 项目里(已含 Aleph 组件库)。
> 先贴【主上下文】定调,再一屏一屏贴。一次只做一屏,Chat 满意后让其余屏沿用同一布局语言。

---

## 0. 主上下文(每次会话开头先贴一次)

```
你正在为 Aleph 设计 iOS 移动端界面。本项目里已有完整的 Aleph 设计系统组件库
(styles/aleph.css 的 token + foundations/components 下的预览),请严格复用,
绝不发明新配色或新样式。输出为完整 iOS 屏的 HTML,复用 aleph.css 的类与 token。

产品:Aleph 是常驻后台的个人 AI 服务;这个 iOS App 是它的移动端 Panel
(瘦客户端,通过 JSON-RPC 连远端 core)。

平台:iOS 手机,竖屏,约 390pt 宽。每屏放进干净的 iPhone mockup 框,焦点是屏内内容。
硬规则:安全区(刘海/Home indicator,env(safe-area-inset-*));点击区 ≥44pt;
主操作放底部拇指可达区;键盘弹起时输入框上浮。

设计系统(必须复用):
- 配色:OKLCH token(--color-surface/-raised/-sunken、--color-text-primary/secondary/tertiary、
  --color-primary 等)。主稿=浅色 + Mauve accent。
- 材质:默认 Liquid 玻璃(visionOS 风,贴合 iOS)。消息气泡用 .msg-glass(AI)/.msg-glass-user(用户)。
- 字体:Inter(正文 13px / line-height 1.5),数字 tabular。圆角走 --radius-*,阴影走 --shadow-*。
- 组件类:.btn/.btn-primary/.btn-secondary/.btn-ghost/.btn-icon、.field、.card、
  .list/.cell/.cell-leading/.cell-chevron、.badge-*、.chip/.chip-active、.tabbar/.tabitem、.sheet、.swatch。
- 图标:线性 SVG,stroke=currentColor,stroke-width 1.8,圆头圆角。

导航(全 App 一致):
- 底部 TabBar 4 项:Chat / Memory / Agents / Settings。
- 二级页 push;次要任务用底部 sheet;顶部 bar 左标题(可带下拉切换)+ 右图标按钮。

每次只设计一屏,所有屏保持同一产品世界(同字体/配色/组件/mockup 尺寸)。
```

---

## 1. Chat(主屏)

```
设计【Chat 主屏】(iOS,默认 tab)。
- 顶部 bar:左 agent 名 "Assistant" + 下拉 chevron(切 agent),右通知 bell 图标按钮。
- 消息流(上→下):
  1) 用户气泡(.msg-glass-user,右对齐):"帮我看下记忆里有没有部署相关的"
  2) AI 气泡(.msg-glass,左对齐):一段中文回答 + 代码块(.glass-inset 包,等宽字体)
     "aleph-server start --port 18790" + 底部 reasoning chip("检索了 3 个来源",带小钟图标)
  3) 再一条 AI 气泡正在流式输出(末尾跳动的打字光标)
- 底部 composer 整行:左附件钮 + 输入框(placeholder "问 Aleph…",支持 / 命令)
  + 语音 orb 圆钮 + 发送圆钮(.btn-icon .btn-primary,上箭头图标)。
- 底部 TabBar:Chat 高亮(primary)。
浅色 + Liquid 材质 + Mauve。放进 iPhone 框。
```

## 2. Voice(语音沉浸态)

```
设计【Voice 沉浸态】(全屏覆盖,从 Chat 语音钮进入)。极简,无 TabBar。
- 背景:surface + 底部向上的 accent 径向光晕。
- 正中:大语音 orb(圆形,内部 accent 渐变流动 + 高光 + 柔和外发光),表现"正在聆听"。
- orb 下方:实时字幕(大号、居中、易读)"帮我把今天的会议纪要整理一下…",
  字幕下一行浅色提示 "松开结束"。
- 底部:结束按钮(圆形,中性/danger,停止图标)。
浅色 + Mauve。放进 iPhone 框。
```

## 3. Memory(Vault 列表)

```
设计【Memory · Vault 列表】(iOS,Memory tab)。
- 顶部 bar:标题 "Memory" + 右搜索图标;其下一行 segmented control(Graph / Vault),Vault 选中。
- 搜索框(.field,placeholder "搜索记忆…")。
- facet chips 一行:All(.chip-active)/ Facts / Notes / Corrections。
- 结果列表(.list + 多个 .cell):每条 = 标题 + 副标题(类型 · 日期)+ 右侧类型 .badge。
  给 5-6 条不同内容(偏好/事实/纠正等)。
- 底部 TabBar:Memory 高亮。
浅色 + Liquid + Mauve。iPhone 框。
```

## 4. Agents

```
设计【Agents 列表】(iOS,Agents tab)。
- 顶部 bar:标题 "Agents" + 右 + 添加图标按钮。
- Agent 列表(.list/.cell):每条 = 左圆形字母徽标(.cell-leading)+ 名称
  + 副标题(人格档 · 模型,如 "Expert · Opus 4.8")+ 默认项右侧 .badge-primary "default" + chevron。
  给 4 个:Assistant / Expert / Companion / Maker。
- 底部一张当前 agent 只读概览卡(.card):名称 + 一句 soul 摘要 + "Set default" 次要按钮。
- 底部 TabBar:Agents 高亮。
浅色 + Mauve。iPhone 框。
```

## 5. Settings(分组列表)

```
设计【Settings 分组列表】(iOS,Settings tab)。
- 顶部 bar:标题 "Settings"。
- iOS 风分组列表(每组 .list-header + .list/.cell;cell 含 .cell-leading 图标 + 标题 + 右值 + chevron):
  · 组「连接」:Connection(值 "remote · 10.10.10.4")
  · 组「AI」:Providers(值 "Anthropic")、Embeddings(值 "text-embedding-3")、Model route(值 "Opus 4.8")
  · 组「外观」:Theme(值 "System")、Accent(右侧 5 个 .swatch 色点,Mauve 选中)、Material(值 "Liquid")
- 底部 TabBar:Settings 高亮。
浅色 + Mauve。iPhone 框。
```

## 6. Notifications / Approvals(通知中心)

```
设计【通知中心】(iOS,从顶部 bell 进入;做成全屏页或底部大 sheet 皆可)。
- 顶部 bar:标题 "Notifications" + 右 "Clear"。
- 列表(.list/.cell),混合审批 + 通知:
  1) 审批请求卡(突出,.card):"Agent 想执行 shell 命令" + 命令行(.glass-inset 等宽)"rm -rf build/"
     + 底部两钮:"拒绝"(.btn-secondary)、"批准"(.btn-primary)。
  2) 普通通知若干(.cell + 左状态 .badge):成功 "Memory 重嵌入完成"、
     警告 "与 core 短暂断开已恢复"、info "Cron:早报已投递"。每条带时间。
浅色 + Mauve。iPhone 框(若做 sheet:在 Chat 屏上盖 .scrim + .sheet)。
```

---

## 用法技巧(让 claude.ai/design 不跑偏)

1. **一次一屏**。先做 Chat 定调,满意后对其余屏说:"沿用 Chat 屏的布局语言、字体、配色、mockup 尺寸"。
2. 每次强调:"**复用本项目的 aleph.css token 和组件类,不要新造样式或配色**"。
3. 要变体就让它出**浅色 + 暗色**,或 **Liquid vs Luxe** 材质对比。
4. 不满意只指**具体项**(字太小 / 间距 / 导航假 / 设备框不均),让它**重画那一屏**,别从头来。
5. 设备框**统一尺寸、四周留白均匀**,焦点始终在屏内内容(不是炫设备)。
6. 想要交互流,就让它把两屏并排(如 Chat → Voice、Agents 列表 → Agent 概览),保持同一设计系统。
```
