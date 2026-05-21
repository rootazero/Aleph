# Beyond OpenClaw: Aleph 全面能力升级设计

> 日期: 2026-03-09
> 状态: Approved
> 方法: 分阶段递进 (依赖最优排序)

## 背景

对标 OpenClaw 项目，系统性补齐 Aleph 的能力短板，同时保持 Aleph 自身的架构优势（Rust Core、大脑-四肢分离、LanceDB 混合检索、一核多端）。

不是照搬 OpenClaw，而是融合 Aleph 架构思想，在每个领域做到超越。

## Aleph 固有优势（不可丢失）

- Rust Core 性能 + 内存安全
- R1 大脑-四肢分离架构
- Leptos/WASM 统一 UI
- LanceDB 向量+FTS 混合检索（优于 OpenClaw 的 markdown 文件方案）
- ACMA 认知记忆架构（三层三域）
- 一核多端（macOS/Tauri/Web/CLI）

## 阶段总览

| 阶段 | 核心交付 | 依赖 |
|------|---------|------|
| P1 会话编排 | Steering + 队列 + 压缩前持久化 + Prompt 缓存 | 无 |
| P2 浏览器 | BrowserManager + Playwright MCP + Profile 系统 + SSRF + 14 个 tool | P1 |
| P3 媒体 | MediaPipeline + 4 类 Processor + 多 provider fallback + 2 个验证插件 | 部分 P2 |
| P4 插件 | CLI 工具链 + SDK 增强 + 6 个核心插件 + 轻量注册中心 + 文档 | P3 |

---

## P1: 会话编排增强

### 1.1 Steering 模式（中断执行）

**问题**: 当前 agent loop 串行执行 tool call，用户发新消息只能等当前轮次结束。

**设计**:
- 在 `Session` 上增加 `interrupt_tx/rx` channel (`tokio::sync::watch`)
- Tool 执行前检查中断信号，粒度是 tool 间（不在 tool 执行中途强杀）
- 被中断的 tool 结果标记为 `Cancelled`
- LLM 上下文中看到 `[tool was cancelled due to new user input: "..."]`

```
Agent Loop (改造后)
┌─────────────────────────────────────┐
│  await_next_action()                │
│    ├─ LLM 返回 tool_use → 执行前   │
│    │   检查 interrupt_rx.try_recv() │
│    │   ├─ 有中断 → 取消当前 tool,  │
│    │   │   将新消息注入上下文,      │
│    │   │   重新请求 LLM             │
│    │   └─ 无中断 → 正常执行 tool    │
│    └─ LLM 返回 text → 正常结束     │
└─────────────────────────────────────┘
```

改动局限在 Executor/Dispatcher 层，不侵入 Tool 实现（R1 兼容）。

### 1.2 队列模式

三种模式，Session 级可配置：

| 模式 | 行为 | 适用场景 |
|------|------|----------|
| `followup` (默认) | 新消息排队，当前轮次完成后按序处理 | 大多数对话 |
| `steer` | 新消息触发中断，合并处理 | 浏览器操作、改主意 |
| `collect` | 收集 N 秒内所有消息合并为一条再触发 agent | 群聊、连续输入 |

实现为 `SessionQueue` trait + 三个 struct，通过 `AppContext` 注入 Dispatcher。

### 1.3 压缩前记忆持久化

在 `CompactionTask` 执行前插入 silent agent turn：
- System prompt 注入: `"Your context is about to be compacted. Write any important information to memory now using memory_store."`
- Agent 的这轮输出不展示给用户 (`silent: true`)
- 完成后再执行压缩

### 1.4 Prompt 缓存优化

将 system prompt 分为稳定区和动态区：
- **稳定区**（排前面）: 人格设定、工具 schema、技能列表 → 变化频率低，利于缓存
- **动态区**（排后面）: 时间、会话上下文、workspace 信息 → 每次变化
- 对 Claude API 使用 `cache_control` breakpoint 标记稳定区末尾

---

## P2: 浏览器系统

### 2.1 整体架构

```
┌─────────────────────────────────────────────┐
│              Aleph Core (Rust)               │
│  ┌─────────────────────────────────────┐     │
│  │       BrowserManager (Rust)         │     │
│  │  - Profile 生命周期管理             │     │
│  │  - 实例注册表 (port/pid/state)      │     │
│  │  - 健康检查 & 自动回收              │     │
│  │  - SSRF 策略执行                    │     │
│  └──────────┬──────────────────────────┘     │
│             │ Trait: BrowserRuntime           │
│  ┌──────────▼──────────────────────────┐     │
│  │   PlaywrightBridge (MCP Client)     │     │
│  │  - 通过 MCP 协议调用 Playwright     │     │
│  │  - 将 MCP 结果映射为 BrowserAction  │     │
│  └──────────┬──────────────────────────┘     │
└─────────────┼───────────────────────────────┘
              │ stdio / WebSocket
┌─────────────▼───────────────────────────────┐
│     Playwright MCP Server (Node.js)          │
│  - 浏览器启动/连接                           │
│  - DOM 操作 (click/type/select/drag)         │
│  - 截图/snapshot (ARIA tree)                 │
│  - JavaScript 执行                           │
│  - 文件上传/下载                             │
│  - 网络拦截                                  │
└──────────────────────────────────────────────┘
```

关键决策:
- `BrowserManager` 在 Core 中定义 trait，实现在 `crates/browser/`（R1）
- Playwright 通过 MCP 协议接入，不在 Core 引入 Node.js 依赖（R3）
- 所有浏览器 tool 复用 P1 的 steering 模式

### 2.2 Profile 管理系统

```
~/.aleph/browser/
├── profiles/
│   ├── default/          # 默认隔离 profile
│   ├── work/             # 工作 profile
│   └── research/         # 调研 profile
└── browser.toml          # profile 配置
```

Profile 生命周期:
- `BrowserManager` 持有 `HashMap<ProfileName, ProfileState>`
- 状态机: `Idle → Starting → Running(pid, port) → Stopping → Idle`
- 闲置超时自动回收（默认 30min）
- 进程崩溃检测 + 自动清理僵尸锁文件

### 2.3 浏览器 Tool 清单

| Tool | 功能 |
|------|------|
| `browser_open` | 打开 URL（指定 profile） |
| `browser_click` | 点击元素（CSS/XPath/ARIA/坐标） |
| `browser_type` | 输入文本 |
| `browser_select` | 下拉选择 |
| `browser_screenshot` | 截图（全页/区域/元素） |
| `browser_snapshot` | ARIA 树 + 页面结构 |
| `browser_evaluate` | 执行 JS 并返回结果 |
| `browser_navigate` | 前进/后退/刷新 |
| `browser_tabs` | 列表/切换/关闭标签页 |
| `browser_upload` | 文件上传 |
| `browser_download` | 下载管理 |
| `browser_fill_form` | 智能表单填充 |
| `browser_network` | 网络请求拦截/监控 |
| `browser_profile` | 管理 profile |

Rust 侧只做参数校验 + SSRF 检查 + MCP 调用转发。

### 2.4 SSRF 防护

```rust
pub trait NetworkPolicy: Send + Sync {
    fn check_url(&self, url: &Url) -> Result<(), PolicyViolation>;
}
```

- browser_open 和 browser_navigate 的 Rust 侧前置检查
- 阻止私有网络访问
- 支持 allow/block 域名列表
- 防御在 Core，不信任外部进程

### 2.5 Chrome Extension Relay（P2+ 延期）

用于操作用户已登录浏览器的场景，标记为延期项。

---

## P3: 媒体理解 Pipeline

### 3.1 整体架构

```
┌────────────────────────────────────────────────┐
│                 Aleph Core                      │
│  ┌──────────────────────────────────────────┐   │
│  │          MediaPipeline (调度器)           │   │
│  │  - 格式检测 → 路由到对应 Processor       │   │
│  │  - 尺寸管控 & 生命周期管理               │   │
│  │  - 多 provider fallback                  │   │
│  └─────┬────────┬────────┬────────┬─────────┘   │
│        │        │        │        │              │
│   Image    Audio    Video    Document            │
│  Processor Processor Processor Processor         │
│        ▲        ▲        ▲        ▲              │
│        │ Trait: MediaProvider                    │
│  ┌─────┴────────┴────────┴────────┴─────────┐   │
│  │        Provider Registry                  │   │
│  └──────────────────────────────────────────┘   │
└────────────────────────────────────────────────┘
```

### 3.2 核心 Trait

```rust
pub enum MediaType {
    Image { format: ImageFormat, width: u32, height: u32 },
    Audio { format: AudioFormat, duration_secs: f64 },
    Video { format: VideoFormat, duration_secs: f64 },
    Document { format: DocFormat, pages: Option<u32> },
    Unknown,
}

pub enum MediaOutput {
    Text(String),
    Description(String),
    Structured(serde_json::Value),
    Chunks(Vec<MediaChunk>),
}

pub trait MediaProvider: Send + Sync {
    fn supported_types(&self) -> &[MediaType];
    fn priority(&self) -> u8;
    async fn process(&self, input: &MediaInput) -> Result<MediaOutput>;
}
```

Trait 定义在 Core，provider 实现在 `crates/media/`（R1/R4）。

### 3.3 四类 Processor

**ImageProcessor**: PNG/JPEG/WebP/GIF/SVG/HEIC → OCR、场景描述、图表数据提取。超 20MB 自动压缩。Provider fallback: Claude → OpenAI → Gemini Vision。与现有 `vision_tool` 整合。

**AudioProcessor**: MP3/WAV/OGG/FLAC/M4A/WebM → 语音转文字、说话人分离、语言检测。超 25MB 分段（ffmpeg 静音切割）。Provider: Whisper → Deepgram → Gemini Audio。

**VideoProcessor**: MP4/WebM/MOV/AVI → 本地 ffmpeg 提取关键帧 + 音轨 → ImageProcessor + AudioProcessor → 合并摘要。不上传整个视频。限制最大 30min。

**DocumentProcessor**: PDF/DOCX/XLSX/PPTX/TXT/MD/HTML → 文本提取、表格结构化。PDF 用 Rust crate，Office 通过 Node.js 插件。

### 3.4 尺寸管控 & 生命周期

```rust
pub struct MediaPolicy {
    pub max_image_bytes: u64,      // 20MB
    pub max_audio_bytes: u64,      // 100MB
    pub max_video_duration: u64,   // 1800s
    pub max_document_pages: u32,   // 200
    pub temp_dir: PathBuf,
    pub temp_ttl: Duration,        // 1h
}
```

后台 `TempCleaner` 定时清理过期临时文件。超限返回错误 + 建议。

### 3.5 Tool 层

| Tool | 功能 |
|------|------|
| `media_understand` | 统一入口：自动检测类型 → 返回理解结果 |
| `audio_transcribe` | 专用音频转录（流式、说话人分离） |
| `vision_tool` (增强) | OCR 模式、图表数据提取模式 |
| `document_extract` | 文档文本/表格提取 |

### 3.6 作为插件的验证

Office 格式和 ffmpeg 视频处理实现为 Node.js 插件 (`media-video`, `media-office`)，直接验证 P4 的插件开发体验。

---

## P4: 插件生态补全

### 4.1 开发体验基建

**CLI 脚手架**:
```bash
aleph plugin init my-plugin --type nodejs   # Node.js 插件模板
aleph plugin init my-plugin --type wasm     # WASM 插件模板
```

**开发调试**:
```bash
aleph plugin dev ./plugins/my-plugin
# 文件监听 + 自动重载 + 实时日志 + 工具调用模拟器
```

**验证 & 发布**:
```bash
aleph plugin validate ./plugins/my-plugin   # manifest + schema + 安全扫描
aleph plugin pack ./plugins/my-plugin       # 打包 .aleph-plugin
```

### 4.2 插件 SDK 增强

**Node.js SDK** (`@aleph/plugin-sdk`):
```typescript
import { definePlugin, defineTool, defineHook } from '@aleph/plugin-sdk';
export default definePlugin({
  tools: [defineTool({ name, description, schema, execute })],
  hooks: [defineHook('before_tool_call', handler)],
  services: [{ name, start, stop }]
});
```

**WASM SDK** (`aleph-plugin-sdk` Rust crate):
```rust
#[tool(name = "my_tool", description = "...")]
async fn my_tool(args: MyArgs) -> PluginResult<MyOutput> { ... }
```

### 4.3 核心插件移植

| 插件 | 类型 | 优先级 |
|------|------|--------|
| `media-video` | Node.js (ffmpeg) | P3 已完成 |
| `media-office` | Node.js | P3 已完成 |
| `voice-call` | Node.js (WebRTC/SIP) | 高 |
| `diagnostics` | Node.js (OpenTelemetry) | 高 |
| `memory-analytics` | WASM (记忆统计) | 中 |
| `llm-task` | Node.js (批量 LLM) | 中 |
| `diff-viewer` | WASM (代码 diff) | 中 |
| `phone-control` | Node.js (ADB) | 低 |

不移植的（Aleph 已有更好方案）: memory-lancedb, channel 扩展, copilot-proxy

### 4.4 插件注册中心（轻量方案）

阶段 1: GitHub Release 分发
```bash
aleph plugin install github:rootazero/Aleph-plugins/voice-call
```

阶段 2: 索引文件 (plugins-index.json)
```bash
aleph plugin search voice
aleph plugin install voice-call
```

### 4.5 文档

- `docs/guides/plugin-development.md` — 从零写插件教程
- `docs/guides/plugin-sdk-reference.md` — SDK API 参考
- `docs/guides/plugin-examples.md` — 常见模式示例
