# Message Flow Optimization Design

> 参考 OpenClaw 项目，优化 Aether 的对话消息流和 Agent 结果汇总功能

## 1. 背景与目标

### 1.1 现状分析

对比 OpenClaw 项目，Aether 在消息流处理方面存在以下差距：

| 维度 | OpenClaw | Aether 现状 |
|------|----------|-------------|
| 结果汇总 | `buildEmbeddedRunPayloads()` 完整汇总 | `RunSummary` 只有基础字段 |
| 工具格式化 | emoji + 路径分组 + 参数摘要 | 只显示工具名称 |
| 块级回复 | 工具执行前冲刷已有文本 | 整体流式 |
| 消息去重 | 规范化文本比较 | 无去重机制 |
| 序列号 | Per-RunId 独立计数 | 全局原子计数 |

### 1.2 优化目标

1. **增强结果汇总** - Toast + 详情气泡，点击展开完整工具执行列表
2. **智能工具格式化** - emoji 前缀 + 路径分组 + 关键参数摘要
3. **块级回复策略** - 工具执行前冲刷已累积文本
4. **完整消息去重** - 规范化文本追踪，跳过重复发送
5. **Per-RunId 序列** - 每个 run 独立序列号，支持丢包检测

## 2. 核心数据结构

### 2.1 增强的 RunSummary (Rust)

```rust
/// 增强的运行汇总，支持详细的结果收集
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedRunSummary {
    // 基础统计
    pub total_tokens: u64,
    pub tool_calls: u32,
    pub loops: u32,
    pub duration_ms: u64,

    // 汇总文本（所有助手回复合并）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_response: Option<String>,

    // 工具执行摘要列表
    pub tool_summaries: Vec<ToolSummary>,

    // 推理/思考内容（如果有）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,

    // 错误信息（如果有）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ToolError>,
}

/// 单个工具的执行摘要
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSummary {
    pub tool_id: String,
    pub tool_name: String,
    pub emoji: String,           // 🔨 📄 🌐 ✏️
    pub display_meta: String,    // 智能格式化的参数摘要
    pub duration_ms: u64,
    pub success: bool,
}

/// 工具错误信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolError {
    pub tool_name: String,
    pub error: String,
    pub tool_id: String,
}
```

### 2.2 工具显示配置

```rust
/// 工具显示元数据
pub struct ToolDisplay {
    pub emoji: &'static str,
    pub label: &'static str,
}

/// 工具显示映射
pub fn get_tool_display(tool_name: &str) -> ToolDisplay {
    match tool_name {
        "exec" | "shell" | "bash" => ToolDisplay { emoji: "🔨", label: "Exec" },
        "read" | "read_file" => ToolDisplay { emoji: "📄", label: "Read" },
        "write" | "write_file" => ToolDisplay { emoji: "✏️", label: "Write" },
        "edit" | "edit_file" => ToolDisplay { emoji: "📝", label: "Edit" },
        "web_fetch" | "fetch" => ToolDisplay { emoji: "🌐", label: "Fetch" },
        "search" | "grep" => ToolDisplay { emoji: "🔍", label: "Search" },
        "list" | "ls" => ToolDisplay { emoji: "📁", label: "List" },
        _ => ToolDisplay { emoji: "⚙️", label: tool_name },
    }
}
```

## 3. 智能格式化层

### 3.1 路径分组算法

```rust
/// 智能格式化工具参数为显示字符串
pub fn format_tool_meta(tool_name: &str, params: &Value) -> String {
    match tool_name {
        "read" | "read_file" | "write" | "write_file" => format_path_params(params),
        "edit" | "edit_file" => format_edit_params(params),
        "exec" | "shell" | "bash" => format_exec_params(params),
        "web_fetch" | "fetch" => format_url_params(params),
        _ => format_generic_params(params),
    }
}

/// 路径参数分组：/tmp/{file1.txt, file2.txt}
fn format_path_params(params: &Value) -> String {
    let paths: Vec<&str> = extract_paths(params);
    if paths.is_empty() { return String::new(); }
    if paths.len() == 1 { return shorten_path(paths[0]); }

    // 按目录分组
    let mut groups: HashMap<&str, Vec<&str>> = HashMap::new();
    for path in &paths {
        let (dir, file) = split_path(path);
        groups.entry(dir).or_default().push(file);
    }

    // 格式化: dir/{file1, file2}
    groups.iter()
        .map(|(dir, files)| {
            if files.len() == 1 {
                format!("{}/{}", dir, files[0])
            } else {
                format!("{}/{{{}}}", dir, files.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// 编辑参数：src/main.rs:42-56
fn format_edit_params(params: &Value) -> String {
    let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let line = params.get("line").and_then(|v| v.as_u64());
    let end_line = params.get("end_line").and_then(|v| v.as_u64());

    match (line, end_line) {
        (Some(l), Some(e)) => format!("{}:{}-{}", shorten_path(path), l, e),
        (Some(l), None) => format!("{}:{}", shorten_path(path), l),
        _ => shorten_path(path).to_string(),
    }
}

/// 执行参数：elevated · pty · command
fn format_exec_params(params: &Value) -> String {
    let mut parts = Vec::new();
    if params.get("elevated").and_then(|v| v.as_bool()).unwrap_or(false) {
        parts.push("elevated".to_string());
    }
    if params.get("pty").and_then(|v| v.as_bool()).unwrap_or(false) {
        parts.push("pty".to_string());
    }
    if let Some(cmd) = params.get("command").and_then(|v| v.as_str()) {
        parts.push(truncate_command(cmd, 40));
    }
    parts.join(" · ")
}

/// 生成完整的工具摘要字符串
/// 输出: "🔨 Exec: elevated · mkdir -p /tmp/foo"
pub fn format_tool_summary(tool_name: &str, params: &Value) -> String {
    let display = get_tool_display(tool_name);
    let meta = format_tool_meta(tool_name, params);

    if meta.is_empty() {
        format!("{} {}", display.emoji, display.label)
    } else {
        format!("{} {}: {}", display.emoji, display.label, meta)
    }
}
```

## 4. 流式处理增强

### 4.1 Per-RunId 序列管理

```rust
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Per-RunId 序列计数器管理
pub struct RunSequenceManager {
    sequences: DashMap<String, AtomicU64>,
}

impl RunSequenceManager {
    pub fn new() -> Self {
        Self { sequences: DashMap::new() }
    }

    /// 获取下一个序列号（per-runId）
    pub fn next_seq(&self, run_id: &str) -> u64 {
        self.sequences
            .entry(run_id.to_string())
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::SeqCst)
    }

    /// 清理已完成的 run
    pub fn cleanup(&self, run_id: &str) {
        self.sequences.remove(run_id);
    }
}
```

### 4.2 块级回复缓冲

```rust
/// 流式文本缓冲管理
pub struct StreamBuffer {
    text: String,
    flushed_at: usize,
    in_tool_execution: bool,
}

impl StreamBuffer {
    pub fn new() -> Self {
        Self { text: String::new(), flushed_at: 0, in_tool_execution: false }
    }

    pub fn append(&mut self, content: &str) {
        self.text.push_str(content);
    }

    /// 工具开始前冲刷已累积文本
    pub fn flush_before_tool(&mut self) -> Option<String> {
        if self.flushed_at < self.text.len() {
            let unflushed = self.text[self.flushed_at..].to_string();
            self.flushed_at = self.text.len();
            self.in_tool_execution = true;
            if !unflushed.trim().is_empty() {
                return Some(unflushed);
            }
        }
        self.in_tool_execution = true;
        None
    }

    pub fn tool_ended(&mut self) {
        self.in_tool_execution = false;
    }

    pub fn full_text(&self) -> &str {
        &self.text
    }

    pub fn reset(&mut self) {
        self.text.clear();
        self.flushed_at = 0;
        self.in_tool_execution = false;
    }
}
```

## 5. 消息去重机制

### 5.1 文本规范化

```rust
/// 规范化文本用于去重比较
pub fn normalize_text_for_comparison(text: &str) -> String {
    text.trim()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .replace(['。', '，', '！', '？'], "")
        .replace(['.', ',', '!', '?'], "")
}

/// 检查两段文本是否实质相同
pub fn is_text_duplicate(a: &str, b: &str) -> bool {
    normalize_text_for_comparison(a) == normalize_text_for_comparison(b)
}
```

### 5.2 消息发送追踪器

```rust
use std::collections::HashSet;
use std::time::Instant;

/// 追踪已发送的消息，用于去重
pub struct SentMessageTracker {
    sent_texts: Vec<String>,
    sent_normalized: HashSet<String>,
    sent_targets: Vec<SentTarget>,
}

#[derive(Clone)]
pub struct SentTarget {
    pub channel: String,
    pub user_id: Option<String>,
    pub text: String,
    pub sent_at: Instant,
}

impl SentMessageTracker {
    pub fn new() -> Self {
        Self {
            sent_texts: Vec::new(),
            sent_normalized: HashSet::new(),
            sent_targets: Vec::new(),
        }
    }

    pub fn is_duplicate(&self, text: &str) -> bool {
        let normalized = normalize_text_for_comparison(text);
        self.sent_normalized.contains(&normalized)
    }

    pub fn record_sent(&mut self, text: &str, channel: &str, user_id: Option<&str>) {
        let normalized = normalize_text_for_comparison(text);
        self.sent_texts.push(text.to_string());
        self.sent_normalized.insert(normalized);
        self.sent_targets.push(SentTarget {
            channel: channel.to_string(),
            user_id: user_id.map(|s| s.to_string()),
            text: text.to_string(),
            sent_at: Instant::now(),
        });
    }

    /// 检查并记录（原子操作），返回 true 表示是新消息
    pub fn check_and_record(&mut self, text: &str, channel: &str, user_id: Option<&str>) -> bool {
        if self.is_duplicate(text) { return false; }
        self.record_sent(text, channel, user_id);
        true
    }

    pub fn reset(&mut self) {
        self.sent_texts.clear();
        self.sent_normalized.clear();
        self.sent_targets.clear();
    }
}
```

## 6. Swift UI 增强

### 6.1 增强的结果摘要模型

```swift
/// 增强的运行摘要（匹配 Rust 侧）
struct EnhancedRunSummary: Codable, Equatable {
    let totalTokens: UInt64
    let toolCalls: UInt32
    let loops: UInt32
    let durationMs: UInt64
    let finalResponse: String?
    let toolSummaries: [ToolSummaryItem]
    let reasoning: String?
    let errors: [ToolErrorItem]
}

struct ToolSummaryItem: Codable, Equatable, Identifiable {
    let id: String           // tool_id
    let toolName: String
    let emoji: String
    let displayMeta: String
    let durationMs: UInt64
    let success: Bool

    var formatted: String {
        displayMeta.isEmpty ? "\(emoji) \(toolName)" : "\(emoji) \(toolName): \(displayMeta)"
    }
}

struct ToolErrorItem: Codable, Equatable {
    let toolName: String
    let error: String
    let toolId: String
}
```

### 6.2 详情气泡视图

详见 `HaloResultDetailPopover.swift`，支持：
- 头部：状态图标 + 统计信息
- 工具列表：emoji + 格式化参数 + 耗时
- 错误列表：工具名 + 错误信息
- 推理摘要：截断显示
- 底部：复制 + 关闭按钮

### 6.3 Toast 触发 Popover

```swift
struct HaloResultViewV2: View {
    let context: ResultContext
    let enhancedSummary: EnhancedRunSummary?
    @State private var showingDetail = false

    var body: some View {
        HaloResultView(context: context, ...)
            .onTapGesture { if enhancedSummary != nil { showingDetail = true } }
            .popover(isPresented: $showingDetail) {
                HaloResultDetailPopover(summary: enhancedSummary!, ...)
            }
    }
}
```

## 7. 文件变更清单

### 7.1 Rust Core 新建文件

| 文件 | 说明 |
|------|------|
| `core/src/gateway/tool_display.rs` | 工具 emoji 映射、智能格式化 |
| `core/src/gateway/message_dedup.rs` | 文本规范化、去重追踪器 |
| `core/src/gateway/stream_buffer.rs` | 块级回复缓冲管理 |
| `core/src/gateway/run_context.rs` | Agent 运行上下文 |

### 7.2 Rust Core 修改文件

| 文件 | 变更 |
|------|------|
| `core/src/gateway/event_emitter.rs` | Per-RunId 序列、增强 RunSummary |
| `core/src/gateway/mod.rs` | 导出新模块 |

### 7.3 Swift macOS 新建文件

| 文件 | 说明 |
|------|------|
| `Components/HaloResultDetailPopover.swift` | 详情气泡视图 |

### 7.4 Swift macOS 修改文件

| 文件 | 变更 |
|------|------|
| `Gateway/ProtocolModels.swift` | EnhancedRunSummary、ToolSummaryItem |
| `Gateway/GatewayStreamAdapter.swift` | 处理增强事件、工具摘要累积 |
| `Components/HaloResultView.swift` | 添加 Popover 触发 |
| `Components/HaloStreamingView.swift` | 使用格式化的工具摘要 |

### 7.5 需要清理的旧代码

| 位置 | 说明 |
|------|------|
| `event_emitter.rs` 中的全局 `seq_counter` | 替换为 `RunSequenceManager` |
| 简单的 `RunSummary` 结构 | 替换为 `EnhancedRunSummary` |

## 8. 实施优先级

1. **P0 - 核心基础设施**
   - Per-RunId 序列管理
   - 增强 RunSummary 结构
   - 工具格式化模块

2. **P1 - 流式处理**
   - 块级回复缓冲
   - 工具执行前冲刷

3. **P2 - UI 增强**
   - Swift 协议模型更新
   - 详情气泡视图
   - Toast + Popover 集成

4. **P3 - 去重机制**
   - 文本规范化
   - 消息追踪器
   - 渠道集成

## 9. 兼容性考虑

- `EnhancedRunSummary` 保持与旧 `RunSummary` 的字段兼容
- 新字段使用 `#[serde(skip_serializing_if)]` 避免破坏旧客户端
- Swift 侧使用可选解码，兼容旧版 Gateway
