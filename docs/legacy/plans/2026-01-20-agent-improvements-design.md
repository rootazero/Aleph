# AI Agent Improvements Design

Date: 2026-01-20

## Overview

This document outlines fixes for four issues identified in the Aether AI agent:

1. CPU 100% with slow output (Typewriter O(n²) problem)
2. nanobanana drawing model not recognized
3. Task decomposition not working
4. Intermediate progress not displayed in multi-turn mode

## Problem Analysis

### Problem 1: Typewriter O(n²) Performance

**Root Cause**: Each character triggers a full UI re-render.

```swift
// MultiTurnCoordinator.swift:259-267
for char in response {  // 13021 iterations
    currentText.append(char)
    viewModel.updateStreamingText(currentText)  // Full string passed each time
}
```

**Impact**: 13021 characters = 13021 re-renders, each more expensive than the last.

### Problem 2: generate_image Tool Not Registered

**Root Cause**: The tool is never added to the agent's toolset.

```rust
// core/src/agent/manager.rs:71
const BUILTIN_TOOLS: &[&str] = &["search", "web_fetch", "youtube", "file_ops"];
// Missing: "generate_image"
```

### Problem 3: Task Decomposition Depends on Tool Awareness

**Root Cause**: UnifiedPlanner doesn't know about image generation capabilities, so it can't plan tasks that use them.

### Problem 4: Multi-turn Mode Skips Progress Updates

**Root Cause**: EventHandler explicitly skips UI updates in multi-turn mode.

```swift
guard !slf.isInMultiTurnMode else { return }
```

---

## Solution Design

### Part 1: Batch Typewriter Updates

**Files to modify**:
- `platforms/macos/Aether/Sources/MultiTurn/MultiTurnCoordinator.swift`

**Changes**:

```swift
// Constants
private let BATCH_SIZE = 50      // Update every 50 characters
private let THROTTLE_MS = 50     // Minimum 50ms between updates

private func startTypewriterOutput(response: String, topic: Topic, userInput: String, isFirstMessage: Bool, speed: Int) {
    typewriterTask?.cancel()

    guard unifiedWindow.viewModel.startStreamingMessage() != nil else {
        print("[MultiTurnCoordinator] Failed to start streaming message")
        return
    }

    let charDelay = 1.0 / Double(max(speed, 1))

    typewriterTask = Task { @MainActor in
        var currentText = ""
        var lastUpdateTime = Date()
        let responseChars = Array(response)

        for (index, char) in responseChars.enumerated() {
            if Task.isCancelled {
                print("[MultiTurnCoordinator] Typewriter cancelled")
                break
            }

            currentText.append(char)

            // Batch update: every BATCH_SIZE chars OR every THROTTLE_MS OR last char
            let timeSinceLastUpdate = Date().timeIntervalSince(lastUpdateTime)
            let shouldUpdate = (index + 1) % BATCH_SIZE == 0
                || timeSinceLastUpdate >= Double(THROTTLE_MS) / 1000.0
                || index == responseChars.count - 1

            if shouldUpdate {
                unifiedWindow.viewModel.updateStreamingText(currentText)
                lastUpdateTime = Date()
            }

            try? await Task.sleep(nanoseconds: UInt64(charDelay * 1_000_000_000))
        }

        unifiedWindow.viewModel.finishStreamingMessage()
        finishResponse(topic: topic, userInput: userInput, aiResponse: response, isFirstMessage: isFirstMessage)
    }
}
```

**Complexity**: O(n²) → O(n)

---

### Part 2: Register generate_image Tool

**Files to modify**:
- `core/src/agent/manager.rs`
- `core/src/ffi/processing.rs`

**Changes in manager.rs**:

```rust
// Update BUILTIN_TOOLS
const BUILTIN_TOOLS: &[&str] = &["search", "web_fetch", "youtube", "file_ops", "generate_image"];

// Add generation_registry parameter
pub struct BuiltinToolConfig {
    pub tavily_api_key: Option<String>,
    pub generation_registry: Option<Arc<GenerationProviderRegistry>>,
}

fn create_builtin_tool_server(config: Option<&BuiltinToolConfig>) -> ToolServer {
    let search_tool = if let Some(cfg) = config {
        SearchTool::with_api_key(cfg.tavily_api_key.clone())
    } else {
        SearchTool::new()
    };

    let mut server = ToolServer::new()
        .tool(search_tool)
        .tool(WebFetchTool::new())
        .tool(YouTubeTool::new())
        .tool(FileOpsTool::new());

    // Add image generation tool if registry available
    if let Some(ref cfg) = config {
        if let Some(ref registry) = cfg.generation_registry {
            server = server.tool(ImageGenerateTool::new(Arc::clone(registry)));
        }
    }

    server
}
```

**Changes in processing.rs**:

```rust
fn get_builtin_tool_descriptions(generation_config: &GenerationConfig) -> Vec<ToolDescription> {
    let mut tools = vec![
        ToolDescription::new(
            "file_ops",
            "文件系统操作 - 支持 list、read、write、move、copy、delete、mkdir、search、organize、batch_move"
        ),
        ToolDescription::new("search", "网络搜索 - 搜索互联网获取最新信息"),
        ToolDescription::new("web_fetch", "获取网页内容 - 读取指定URL的网页内容"),
        ToolDescription::new("youtube", "YouTube视频信息 - 获取YouTube视频的标题、描述、字幕等信息"),
    ];

    // Add image generation tool with available providers
    let image_providers: Vec<String> = generation_config
        .get_providers_for_type(GenerationType::Image)
        .iter()
        .map(|(name, _)| name.to_string())
        .collect();

    if !image_providers.is_empty() {
        tools.push(ToolDescription::new(
            "generate_image",
            format!(
                "图片生成 - 根据文字描述生成图片。可用模型: {}。使用 provider 参数指定模型。",
                image_providers.join(", ")
            )
        ));
    }

    tools
}
```

---

### Part 3: Enhance Planner Prompt

**Files to modify**:
- `core/src/planner/prompt.rs`

**Changes**:

Update `get_system_prompt_with_tools` to include:
1. Clear examples of multi-step task decomposition
2. Recognition patterns for generation tasks
3. Example showing knowledge graph generation with file read + analysis + image generation

Key additions to prompt:
- Recognize "绘制", "生成图片", "画" as triggers for generate_image
- Decompose compound requests into task graphs
- Include provider selection in parameters

---

### Part 4: Multi-turn Progress Display

**Files to modify**:
- `platforms/macos/Aether/Sources/MultiTurn/UnifiedConversationViewModel.swift`
- `platforms/macos/Aether/Sources/MultiTurn/UnifiedConversationWindow.swift`
- `platforms/macos/Aether/Sources/MultiTurn/Views/ConversationContentView.swift`

**Changes in UnifiedConversationViewModel.swift**:

```swift
// Add progress tracking properties
@Published var currentToolCall: String? = nil
@Published var planSteps: [PlanStep] = []
@Published var currentStepIndex: Int = 0

struct PlanStep: Identifiable {
    let id: String
    let description: String
    var status: StepStatus = .pending

    enum StepStatus {
        case pending, running, completed, failed
    }
}

func resetProgress() {
    currentToolCall = nil
    planSteps = []
    currentStepIndex = 0
}
```

**Changes in UnifiedConversationWindow.swift**:

```swift
private func setupNotificationObservers() {
    NotificationCenter.default.addObserver(
        forName: .agenticToolCallStarted, object: nil, queue: .main
    ) { [weak self] notification in
        guard let toolName = notification.userInfo?["toolName"] as? String else { return }
        self?.viewModel.currentToolCall = toolName
        if self?.viewModel.currentStepIndex ?? 0 < self?.viewModel.planSteps.count ?? 0 {
            self?.viewModel.planSteps[self!.viewModel.currentStepIndex].status = .running
        }
    }

    NotificationCenter.default.addObserver(
        forName: .agenticToolCallCompleted, object: nil, queue: .main
    ) { [weak self] _ in
        if self?.viewModel.currentStepIndex ?? 0 < self?.viewModel.planSteps.count ?? 0 {
            self?.viewModel.planSteps[self!.viewModel.currentStepIndex].status = .completed
        }
        self?.viewModel.currentToolCall = nil
        self?.viewModel.currentStepIndex += 1
    }

    NotificationCenter.default.addObserver(
        forName: .agenticToolCallFailed, object: nil, queue: .main
    ) { [weak self] _ in
        if self?.viewModel.currentStepIndex ?? 0 < self?.viewModel.planSteps.count ?? 0 {
            self?.viewModel.planSteps[self!.viewModel.currentStepIndex].status = .failed
        }
        self?.viewModel.currentToolCall = nil
    }

    NotificationCenter.default.addObserver(
        forName: .agenticPlanCreated, object: nil, queue: .main
    ) { [weak self] notification in
        guard let steps = notification.userInfo?["steps"] as? [String] else { return }
        self?.viewModel.planSteps = steps.enumerated().map {
            PlanStep(id: "step_\($0.offset)", description: $0.element)
        }
        self?.viewModel.currentStepIndex = 0
    }
}
```

**Changes in ConversationContentView.swift**:

Add progress indicator above message list:

```swift
// Current tool execution indicator
if let tool = viewModel.currentToolCall {
    HStack(spacing: 8) {
        ProgressView()
            .scaleEffect(0.7)
        Text("正在执行: \(tool)")
            .font(.caption)
            .foregroundColor(.secondary)
    }
    .padding(.horizontal)
    .transition(.opacity)
}

// Plan steps list (if multi-step task)
if !viewModel.planSteps.isEmpty {
    VStack(alignment: .leading, spacing: 4) {
        Text("执行计划")
            .font(.caption.bold())
            .foregroundColor(.secondary)

        ForEach(viewModel.planSteps) { step in
            HStack(spacing: 6) {
                stepStatusIcon(step.status)
                Text(step.description)
                    .font(.caption)
                    .foregroundColor(step.status == .running ? .primary : .secondary)
            }
        }
    }
    .padding(12)
    .background(Color.secondary.opacity(0.1))
    .cornerRadius(8)
    .padding(.horizontal)
}

@ViewBuilder
private func stepStatusIcon(_ status: PlanStep.StepStatus) -> some View {
    switch status {
    case .pending:
        Image(systemName: "circle")
            .foregroundColor(.secondary)
    case .running:
        ProgressView()
            .scaleEffect(0.6)
    case .completed:
        Image(systemName: "checkmark.circle.fill")
            .foregroundColor(.green)
    case .failed:
        Image(systemName: "xmark.circle.fill")
            .foregroundColor(.red)
    }
}
```

---

## Implementation Order

1. **Part 1** (Typewriter) - Independent, immediate performance improvement
2. **Part 2** (Tool Registration) - Core prerequisite for Parts 3 & 4
3. **Part 3** (Planner Prompt) - Depends on Part 2
4. **Part 4** (Progress UI) - Can be done in parallel with Part 3

## Testing Checklist

- [ ] Typewriter output no longer causes CPU spike
- [ ] `generate_image` tool appears in agent capabilities
- [ ] AI mentions available image providers (including T8Star)
- [ ] Multi-step requests are decomposed into task graphs
- [ ] Progress indicators appear during tool execution
- [ ] Plan steps are displayed for multi-step tasks
