# Voice Immersive UI — Siri 级语音交互界面设计

**Date**: 2026-06-13
**Status**: Approved by user (brainstorming session with visual companion)
**Scope**: Aleph Panel (Leptos/WASM) + 共享至 Tauri desktop shell webview

## 1. 背景与目标

语音将是 AI 人机交互的主要方式，Aleph 提前布局；炫酷的语音 UI 是吸引用户使用语音的关键杠杆。
本设计在 Panel 内打造沉浸式语音交互体验，对标 Siri / ChatGPT 高级语音模式的视觉与交互水准。

前置依赖：语音后端已转向 BYO OpenAI 兼容端点（见 plan `2026-06-12-aleph-voice-local-sidecar.md`
Amendment 1）。Panel 已有完整 batch 语音闭环（`voice.transcribe` / `voice.synthesize` /
MediaRecorder 录音 / `<audio>` 播放），本设计是其上的体验层，**零 gateway 改动**。

## 2. 已确认的产品决策

| 决策点 | 选择 |
|--------|------|
| 形态 | A 全屏沉浸模式为主 + B 聊天 composer 增强共存；退出 A 回落到 B |
| 对话回路 | 伪流式 + 可打断（客户端 VAD 自动判句；TTS 播放中开口即打断）；真全双工流式留二期 |
| 视觉主角 | 有机流体球（fluid blob）+ 主题色流光：accent 主调 + 放宽的邻近色（±60° 量级）内部流动 + 极慢全局色相漂移 |
| 屏幕布局 | 纯净剧场：球居中独占舞台 + 单行实时字幕（你的话/AI 的话切换）+ 退出提示 |
| 唤醒入口 | composer 流光球按钮（现麦克风按钮换皮）点击进入 + Panel 内快捷键（macOS ⌘⇧V；Win/Linux 取 Ctrl+Alt+V 避开纯文本粘贴冲突） |
| 渲染技术 | 纯 CSS 分层渐变 + 音频驱动 CSS 变量；球封装为可换内核组件（二期可升级 Canvas/shader） |

## 3. 状态机

```
Idle ──球按钮/⌘⇧V──▶ Listening ──VAD判句──▶ Processing ──首句TTS就绪──▶ Speaking
                        ▲   ▲                  (转写+Agent思考)            │
                        │   └────── 整段回答播完 ◀─────────────────────────┘
                        │   └────── 打断：Speaking 中检测到人声 → 停TTS+清队列 → Listening
                       esc/✕ ──▶ 退出 → 聊天流（B 形态 composer）
```

- **Listening**: 麦克常开；AnalyserNode 能量阈值 VAD——持续 ≥~300ms 人声开始捕获，
  静音 ≥~700ms 判句结束自动送转写（免按键）。球随 `--voice-level` 呼吸缩放。
- **Processing**: 球慢速旋流。转写文本作为普通聊天消息发进当前 session。
- **Speaking**: 句级流水线 TTS（见 §5）；球随播放能量脉动；麦克保持开启
  （`echoCancellation: true` 系统 AEC 防自激）供打断检测。
- **打断语义**: 停播 + 清 TTS 队列 + **不取消 Agent run**（文字继续生成入库，只是不再念）。
  用户接着说的话作为新消息发送，由 core 既有 busy-queue/steering 处理。
- **字幕**: Listening 判句后闪现用户的话；Speaking 按音频时长渐进显示 AI 当前句
  （不做逐字精确对齐，一期用时长估算节奏）。

## 4. 组件架构（全部在 Panel）

```
interfaces/webchat/src/views/voice/
├── mod.rs        ImmersiveVoiceView — 全屏覆盖层，状态机宿主，esc/快捷键处理
├── orb.rs        VoiceOrb — props = (state, level 信号)；渲染内核 = 分层 div + CSS 类，
│                 内核可替换（二期 Canvas/shader 不动调用方）
├── captions.rs   单行字幕组件（说话方切换 + 渐进显示）
└── audio.rs      音频管线：MicCapture（getUserMedia+AnalyserNode+MediaRecorder）
                  / VAD（纯函数状态机）/ TtsQueue（句队列 + 播放 + 打断清理）
```

- **样式**: `styles/tailwind.css` 新增 `--voice-orb-*` token（从 accent token 派生，
  五色板 Mauve/Ocean/Forest/Sunset/Rose 自动跟随）+ 形变/流光 keyframes。
  `prefers-reduced-motion` → 静态渐变 + 透明度脉动；`prefers-reduced-transparency` → 实心球。
- **音量驱动**: rAF 每帧把 AnalyserNode 能量写入 CSS 变量 `--voice-level`，
  球的缩放/流速/光晕统一消费该变量（JS 只写一个数，动效全在 CSS）。
- **composer（B 形态）**: 现有 `VoiceInputButton` 换皮为迷你流光球（同套 CSS 类缩放）；
  点击 = 进沉浸模式；长按 = 保留原"录音→转文字进输入框"。
- **VAD 纯函数**: `fn vad_step(state, energy_frame) -> (state, Option<Event>)`——
  host 可测，不碰 web_sys（项目测试红线）。

## 5. 数据流（一轮对话）

```
麦克风 → AnalyserNode → VAD 判句 → MediaRecorder blob (webm/opus) → base64
  → voice.transcribe RPC → 字幕闪现 + chat.send（进当前 session）
  → Panel 既有流式订阅（TextDelta/RunComplete）→ 句切分器（。！？.!? 边界 + 最小句长）
  → 逐句 voice.synthesize RPC → TtsQueue 顺序播放（首句即响，压伪流式延迟）
```

- **会话一致性**: 沉浸模式是当前 session 的另一个渲染器，不建新会话；消息/工具调用照常，
  退出后聊天流可见完整记录。
- **端点无关**: `voice.synthesize`/`voice.transcribe` 走 core provider 解析
  （BYO 本地端点或云端，本地失败 core 自动降云端），Panel 不感知。

## 6. 错误处理与降级

| 故障 | 表现 | 恢复 |
|------|------|------|
| 麦克风权限被拒 | 球变灰 + 字幕给系统设置指引 | 不进入/自动退出沉浸模式 |
| 转写失败 | 字幕“没听清，再说一次？” + 球短促红晕 | 自动回 Listening；连续 3 次建议退出 |
| TTS 单句失败 | 该句静默跳过，字幕仍显示文字 | 继续下一句；全败 = 纯字幕模式不中断对话 |
| Agent run 出错 | 复用聊天流既有错误文案，TTS 念简短错误 | 回 Listening |
| 切后台/锁屏 | 暂停 Listening（不偷录） | 回前台自动恢复 |
| reduced-motion/transparency | 静态渐变球 / 实心球 | CSS 媒体查询，无 JS 分支 |

原则：任何单点故障不锁死循环，最差降级 = 纯字幕文字对话（P7）。

## 7. 测试

1. VAD 状态机纯函数单测（静音/短噪声/正常句/拖尾/边界能量）
2. 句切分器单测（中英标点、省略号、代码块跳过、最小句长合并）
3. 状态机转换表单测（含非法转换拒绝）
4. `cargo build --target wasm32-unknown-unknown`（panel 改动必跑 wasm 验证）+ clippy
5. 视觉验收：standalone HTML + chrome-devtools，三材质 × 五色板截图对比
6. 人工 E2E：真麦克风完整对话 + 打断 + esc 退出 + 权限拒绝路径

## 8. 一期范围外（YAGNI，二期候选）

- WebSocket + PCM/Opus 真全双工流式通道（用户既有决策：全双工仅自研客户端）
- 唤醒词（用户既有决策：点击唤醒 + 动态释放）
- 全局桌面浮窗（形态 C，可复用本期全部视觉资产）
- Canvas / WebGL shader 渲染内核升级
- TTS 字幕逐字精确对齐
