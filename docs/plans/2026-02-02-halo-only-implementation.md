# Halo-Only 消息流重构实施计划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 完全取消对话窗口，将所有 16+ 种 HaloState 简化为 6 种核心状态，删除 ~3000 行冗余代码

**Architecture:** 基于 "Invisible First" 理念，所有交互通过 Halo 浮层完成。参考 OpenClaw 的简洁事件模型（delta/final），统一消息流路径，移除 FFI + Gateway 双路径复杂性。

**Tech Stack:** Swift/SwiftUI (macOS App), Rust (Gateway Core)

---

## Phase 1: 新状态类型定义 (无破坏性变更)

### Task 1.1: 定义新的 StreamingContext 类型

**Files:**
- Create: `platforms/macos/Aether/Sources/Components/HaloStreamingTypes.swift`

**Step 1: 创建新的流式状态类型文件**

```swift
//
//  HaloStreamingTypes.swift
//  Aether
//
//  New streaming types for simplified Halo state model.
//

import SwiftUI

// MARK: - Streaming Context

/// Context for streaming state (replaces processingWithAI, processing, planProgress, etc.)
struct StreamingContext: Equatable {
    let runId: String
    var text: String
    var toolCalls: [ToolCallInfo]
    var reasoning: String?
    var phase: StreamingPhase

    static let maxToolCalls = 3

    init(runId: String, phase: StreamingPhase = .thinking) {
        self.runId = runId
        self.text = ""
        self.toolCalls = []
        self.reasoning = nil
        self.phase = phase
    }

    mutating func appendText(_ delta: String) {
        text += delta
    }

    mutating func addToolCall(_ tool: ToolCallInfo) {
        toolCalls.append(tool)
        // Keep only the most recent tool calls
        if toolCalls.count > Self.maxToolCalls {
            toolCalls.removeFirst(toolCalls.count - Self.maxToolCalls)
        }
    }

    mutating func updateToolStatus(id: String, status: ToolStatus, progress: String? = nil) {
        if let index = toolCalls.firstIndex(where: { $0.id == id }) {
            toolCalls[index].status = status
            if let progress = progress {
                toolCalls[index].progressText = progress
            }
        }
    }
}

/// Phase within streaming state
enum StreamingPhase: Equatable {
    case thinking       // AI is thinking (pulsing animation)
    case responding     // AI is outputting (text preview)
    case toolExecuting  // Tool is executing (tool cards)
}

/// Information about an active tool call
struct ToolCallInfo: Equatable, Identifiable {
    let id: String
    let name: String
    var status: ToolStatus
    var progressText: String?

    init(id: String, name: String) {
        self.id = id
        self.name = name
        self.status = .pending
        self.progressText = nil
    }
}

/// Status of a tool call
enum ToolStatus: Equatable {
    case pending
    case running
    case completed
    case failed
}

// MARK: - Confirmation Context

/// Context for confirmation state (replaces toolConfirmation, planConfirmation, etc.)
struct ConfirmationContext: Equatable {
    let runId: String
    let type: ConfirmationType
    let title: String
    let description: String
    let options: [ConfirmationOption]
    var selectedOption: Int?

    init(
        runId: String,
        type: ConfirmationType,
        title: String,
        description: String,
        options: [ConfirmationOption] = []
    ) {
        self.runId = runId
        self.type = type
        self.title = title
        self.description = description
        self.options = options.isEmpty ? Self.defaultOptions(for: type) : options
        self.selectedOption = nil
    }

    static func defaultOptions(for type: ConfirmationType) -> [ConfirmationOption] {
        switch type {
        case .toolExecution, .planApproval:
            return [
                ConfirmationOption(id: "execute", label: L("button.execute"), isDestructive: false, isDefault: true),
                ConfirmationOption(id: "cancel", label: L("button.cancel"), isDestructive: false, isDefault: false)
            ]
        case .fileConflict:
            return [
                ConfirmationOption(id: "skip", label: L("agent.conflict.skip"), isDestructive: false, isDefault: false),
                ConfirmationOption(id: "rename", label: L("agent.conflict.rename"), isDestructive: false, isDefault: true),
                ConfirmationOption(id: "overwrite", label: L("agent.conflict.overwrite"), isDestructive: true, isDefault: false)
            ]
        case .userQuestion:
            return []  // Options provided by AskUser event
        }
    }
}

/// Type of confirmation required
enum ConfirmationType: Equatable {
    case toolExecution
    case planApproval
    case fileConflict
    case userQuestion
}

/// A single confirmation option
struct ConfirmationOption: Equatable, Identifiable {
    let id: String
    let label: String
    let isDestructive: Bool
    let isDefault: Bool
}

// MARK: - Result Context

/// Context for result state (replaces success)
struct ResultContext: Equatable {
    let runId: String
    let summary: ResultSummary
    let timestamp: Date
    var autoDismissDelay: TimeInterval

    init(runId: String, summary: ResultSummary, autoDismissDelay: TimeInterval = 2.0) {
        self.runId = runId
        self.summary = summary
        self.timestamp = Date()
        self.autoDismissDelay = autoDismissDelay
    }
}

/// Summary of a completed run
struct ResultSummary: Equatable {
    let status: ResultStatus
    let message: String?
    let toolsExecuted: Int
    let tokensUsed: Int?
    let durationMs: Int
    let finalResponse: String

    static func success(
        message: String? = nil,
        toolsExecuted: Int = 0,
        tokensUsed: Int? = nil,
        durationMs: Int = 0,
        finalResponse: String = ""
    ) -> ResultSummary {
        ResultSummary(
            status: .success,
            message: message,
            toolsExecuted: toolsExecuted,
            tokensUsed: tokensUsed,
            durationMs: durationMs,
            finalResponse: finalResponse
        )
    }

    static func error(message: String, durationMs: Int = 0) -> ResultSummary {
        ResultSummary(
            status: .error,
            message: message,
            toolsExecuted: 0,
            tokensUsed: nil,
            durationMs: durationMs,
            finalResponse: ""
        )
    }
}

/// Status of a completed run
enum ResultStatus: Equatable {
    case success    // ✓ Green
    case partial    // ⚠ Yellow (partially completed)
    case error      // ✗ Red

    var iconName: String {
        switch self {
        case .success: return "checkmark.circle.fill"
        case .partial: return "exclamationmark.circle.fill"
        case .error: return "xmark.circle.fill"
        }
    }

    var color: Color {
        switch self {
        case .success: return .green
        case .partial: return .orange
        case .error: return .red
        }
    }
}

// MARK: - Error Context

/// Context for error state
struct ErrorContext: Equatable {
    let runId: String?
    let type: HaloErrorType
    let message: String
    let suggestion: String?
    let canRetry: Bool

    init(
        runId: String? = nil,
        type: HaloErrorType,
        message: String,
        suggestion: String? = nil,
        canRetry: Bool = false
    ) {
        self.runId = runId
        self.type = type
        self.message = message
        self.suggestion = suggestion
        self.canRetry = canRetry
    }
}

/// Type of error
enum HaloErrorType: Equatable {
    case network
    case provider
    case toolFailure
    case timeout
    case unknown

    var iconName: String {
        switch self {
        case .network: return "wifi.exclamationmark"
        case .provider: return "cloud.fill"
        case .toolFailure: return "wrench.and.screwdriver"
        case .timeout: return "clock.badge.exclamationmark"
        case .unknown: return "exclamationmark.triangle.fill"
        }
    }
}

// MARK: - History List Context

/// Context for history list state (triggered by // command)
struct HistoryListContext: Equatable {
    var topics: [HistoryTopic]
    var searchQuery: String
    var selectedIndex: Int?

    init() {
        self.topics = []
        self.searchQuery = ""
        self.selectedIndex = nil
    }

    var filteredTopics: [HistoryTopic] {
        if searchQuery.isEmpty {
            return topics
        }
        return topics.filter { $0.title.localizedCaseInsensitiveContains(searchQuery) }
    }
}

/// A conversation topic in history
struct HistoryTopic: Equatable, Identifiable {
    let id: String
    let title: String
    let lastMessageAt: Date
    let messageCount: Int

    var relativeTime: String {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: lastMessageAt, relativeTo: Date())
    }
}
```

**Step 2: Verify the file compiles**

Run: `cd /Volumes/TBU4/Workspace/Aether/platforms/macos && xcodebuild -scheme Aether -configuration Debug build -quiet 2>&1 | head -20`

Expected: Build succeeds or only unrelated warnings

**Step 3: Commit**

```bash
git add platforms/macos/Aether/Sources/Components/HaloStreamingTypes.swift
git commit -m "$(cat <<'EOF'
feat(halo): add new streaming types for simplified state model

Introduces StreamingContext, ConfirmationContext, ResultContext, ErrorContext,
and HistoryListContext types that will replace the 16+ HaloState variants.
This is a non-breaking change - types are defined but not yet used.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 1.2: 定义新的 HaloStateV2 枚举

**Files:**
- Modify: `platforms/macos/Aether/Sources/HaloState.swift` (add new enum, keep old)

**Step 1: Add HaloStateV2 enum to existing file**

Add at the end of `HaloState.swift`, before the final closing brace:

```swift
// MARK: - HaloState V2 (Simplified)

/// Simplified Halo state with 6 core variants
/// This will replace the original HaloState enum after migration
enum HaloStateV2: Equatable {
    /// Hidden - Halo is not visible
    case idle

    /// Listening for clipboard/input (pulsing circle)
    case listening

    /// Streaming response (replaces processingWithAI, processing, planProgress, etc.)
    case streaming(StreamingContext)

    /// Confirmation required (replaces toolConfirmation, planConfirmation, etc.)
    case confirmation(ConfirmationContext)

    /// Result display (replaces success)
    case result(ResultContext)

    /// Error state
    case error(ErrorContext)

    /// History list (triggered by // command)
    case historyList(HistoryListContext)

    // MARK: - State Query Helpers

    var isIdle: Bool {
        if case .idle = self { return true }
        return false
    }

    var isStreaming: Bool {
        if case .streaming = self { return true }
        return false
    }

    var isConfirmation: Bool {
        if case .confirmation = self { return true }
        return false
    }

    var isResult: Bool {
        if case .result = self { return true }
        return false
    }

    var isError: Bool {
        if case .error = self { return true }
        return false
    }

    var isHistoryList: Bool {
        if case .historyList = self { return true }
        return false
    }

    /// Check if state requires user interaction
    var isInteractive: Bool {
        switch self {
        case .confirmation, .error, .historyList:
            return true
        default:
            return false
        }
    }

    // MARK: - Window Size

    /// Recommended window size for this state
    var windowSize: NSSize {
        switch self {
        case .idle:
            return NSSize(width: 0, height: 0)
        case .listening:
            return NSSize(width: 80, height: 60)
        case .streaming(let ctx):
            switch ctx.phase {
            case .thinking:
                return NSSize(width: 80, height: 60)
            case .responding:
                return NSSize(width: 320, height: 100)
            case .toolExecuting:
                return NSSize(width: 280, height: 120)
            }
        case .confirmation:
            return NSSize(width: 340, height: 280)
        case .result:
            return NSSize(width: 280, height: 80)
        case .error:
            return NSSize(width: 320, height: 200)
        case .historyList:
            return NSSize(width: 380, height: 420)
        }
    }
}

// MARK: - Migration Helpers

extension HaloState {
    /// Convert old HaloState to new HaloStateV2
    /// Used during migration period
    func toV2() -> HaloStateV2 {
        switch self {
        case .idle:
            return .idle
        case .listening:
            return .listening
        case .retrievingMemory, .processingWithAI, .processing:
            var ctx = StreamingContext(runId: "legacy", phase: .thinking)
            if case .processing(let text) = self, let t = text {
                ctx.text = t
                ctx.phase = .responding
            }
            return .streaming(ctx)
        case .typewriting:
            return .streaming(StreamingContext(runId: "legacy", phase: .responding))
        case .success(let message):
            return .result(ResultContext(
                runId: "legacy",
                summary: .success(message: message)
            ))
        case .error(let type, let message, let suggestion):
            return .error(ErrorContext(
                type: type.toHaloErrorType(),
                message: message,
                suggestion: suggestion
            ))
        case .toast(let type, let title, let message, _, _):
            if type == .error {
                return .error(ErrorContext(
                    type: .unknown,
                    message: "\(title): \(message)"
                ))
            }
            return .result(ResultContext(
                runId: "legacy",
                summary: .success(message: "\(title): \(message)")
            ))
        case .clarification:
            return .confirmation(ConfirmationContext(
                runId: "legacy",
                type: .userQuestion,
                title: L("user_input.title"),
                description: ""
            ))
        case .conversationInput:
            return .idle  // No longer supported
        case .toolConfirmation(let id, let name, let desc, let reason, _):
            return .confirmation(ConfirmationContext(
                runId: id,
                type: .toolExecution,
                title: name,
                description: "\(desc)\n\(reason)"
            ))
        case .planConfirmation(let info):
            return .confirmation(ConfirmationContext(
                runId: info.planId,
                type: .planApproval,
                title: L("plan.confirmation.title"),
                description: info.description
            ))
        case .planProgress(let info):
            var ctx = StreamingContext(runId: info.planId, phase: .toolExecuting)
            ctx.text = info.currentStepName
            return .streaming(ctx)
        case .taskGraphConfirmation(let graph):
            return .confirmation(ConfirmationContext(
                runId: graph.planId,
                type: .planApproval,
                title: L("dag.confirm_title"),
                description: graph.title
            ))
        case .taskGraphProgress(let graph, _):
            var ctx = StreamingContext(runId: graph.planId, phase: .toolExecuting)
            ctx.text = graph.title
            return .streaming(ctx)
        case .agentPlan(let planId, let title, _, _):
            return .confirmation(ConfirmationContext(
                runId: planId,
                type: .planApproval,
                title: title,
                description: ""
            ))
        case .agentProgress(let planId, _, let currentOp, _, _):
            var ctx = StreamingContext(runId: planId, phase: .toolExecuting)
            ctx.text = currentOp
            return .streaming(ctx)
        case .agentConflict(let planId, let fileName, let targetPath, _):
            return .confirmation(ConfirmationContext(
                runId: planId,
                type: .fileConflict,
                title: L("agent.conflict.title"),
                description: "\(fileName)\n\(targetPath)"
            ))
        }
    }
}

extension ErrorType {
    /// Convert old ErrorType to new HaloErrorType
    func toHaloErrorType() -> HaloErrorType {
        switch self {
        case .network: return .network
        case .provider: return .provider
        case .permission: return .toolFailure
        case .unknown: return .unknown
        }
    }
}
```

**Step 2: Verify the file compiles**

Run: `cd /Volumes/TBU4/Workspace/Aether/platforms/macos && xcodebuild -scheme Aether -configuration Debug build -quiet 2>&1 | head -20`

Expected: Build succeeds

**Step 3: Commit**

```bash
git add platforms/macos/Aether/Sources/HaloState.swift
git commit -m "$(cat <<'EOF'
feat(halo): add HaloStateV2 with 6 simplified states

Adds HaloStateV2 enum alongside existing HaloState for gradual migration.
Includes migration helpers (toV2()) to convert old states to new format.
- idle, listening, streaming, confirmation, result, error, historyList
- windowSize computed property for automatic sizing

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2: 新 UI 组件

### Task 2.1: 创建 HaloStreamingView 组件

**Files:**
- Create: `platforms/macos/Aether/Sources/Components/HaloStreamingView.swift`

**Step 1: Create the streaming view component**

```swift
//
//  HaloStreamingView.swift
//  Aether
//
//  Unified streaming view for all processing states.
//

import SwiftUI

/// View for streaming state (thinking, responding, tool executing)
struct HaloStreamingView: View {
    let context: StreamingContext

    var body: some View {
        VStack(spacing: 8) {
            switch context.phase {
            case .thinking:
                thinkingView
            case .responding:
                respondingView
            case .toolExecuting:
                toolExecutingView
            }
        }
        .animation(.easeInOut(duration: 0.2), value: context.phase)
    }

    // MARK: - Thinking Phase

    private var thinkingView: some View {
        VStack(spacing: 4) {
            ArcSpinner()
            if let reasoning = context.reasoning, !reasoning.isEmpty {
                Text(reasoning.suffix(40))
                    .font(.caption2)
                    .foregroundColor(.secondary)
                    .lineLimit(1)
            }
        }
    }

    // MARK: - Responding Phase

    private var respondingView: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                ArcSpinner()
                Text(L("halo.responding"))
                    .font(.caption)
                    .foregroundColor(.secondary)
            }

            if !context.text.isEmpty {
                Text(textPreview)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundColor(.primary)
                    .lineLimit(3)
                    .frame(maxWidth: 280, alignment: .leading)
            }
        }
        .padding(8)
        .background(.ultraThinMaterial)
        .cornerRadius(8)
    }

    private var textPreview: String {
        let text = context.text
        if text.count > 120 {
            return "..." + String(text.suffix(117))
        }
        return text
    }

    // MARK: - Tool Executing Phase

    private var toolExecutingView: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(context.toolCalls) { tool in
                HStack(spacing: 6) {
                    toolStatusIcon(tool.status)
                    Text(tool.name)
                        .font(.caption)
                        .fontWeight(.medium)
                    Spacer()
                    if let progress = tool.progressText {
                        Text(progress)
                            .font(.caption2)
                            .foregroundColor(.secondary)
                    }
                }
            }
        }
        .padding(10)
        .background(.ultraThinMaterial)
        .cornerRadius(8)
    }

    @ViewBuilder
    private func toolStatusIcon(_ status: ToolStatus) -> some View {
        switch status {
        case .pending:
            Image(systemName: "circle")
                .font(.caption)
                .foregroundColor(.secondary)
        case .running:
            ProgressView()
                .scaleEffect(0.6)
                .frame(width: 12, height: 12)
        case .completed:
            Image(systemName: "checkmark.circle.fill")
                .font(.caption)
                .foregroundColor(.green)
        case .failed:
            Image(systemName: "xmark.circle.fill")
                .font(.caption)
                .foregroundColor(.red)
        }
    }
}

#if DEBUG
struct HaloStreamingView_Previews: PreviewProvider {
    static var previews: some View {
        VStack(spacing: 20) {
            // Thinking
            HaloStreamingView(context: StreamingContext(runId: "1", phase: .thinking))
                .frame(width: 80, height: 60)

            // Responding
            HaloStreamingView(context: {
                var ctx = StreamingContext(runId: "2", phase: .responding)
                ctx.text = "This is a sample response that shows how text appears during streaming..."
                return ctx
            }())
            .frame(width: 320, height: 100)

            // Tool Executing
            HaloStreamingView(context: {
                var ctx = StreamingContext(runId: "3", phase: .toolExecuting)
                ctx.toolCalls = [
                    ToolCallInfo(id: "1", name: "read_file"),
                    ToolCallInfo(id: "2", name: "write_file"),
                ]
                ctx.toolCalls[0].status = .completed
                ctx.toolCalls[1].status = .running
                return ctx
            }())
            .frame(width: 280, height: 120)
        }
        .padding()
        .background(Color.gray.opacity(0.2))
    }
}
#endif
```

**Step 2: Verify the file compiles**

Run: `cd /Volumes/TBU4/Workspace/Aether/platforms/macos && xcodebuild -scheme Aether -configuration Debug build -quiet 2>&1 | head -20`

Expected: Build succeeds

**Step 3: Commit**

```bash
git add platforms/macos/Aether/Sources/Components/HaloStreamingView.swift
git commit -m "$(cat <<'EOF'
feat(halo): add HaloStreamingView for unified streaming display

Creates a single view component that handles all streaming phases:
- thinking: pulsing spinner with optional reasoning preview
- responding: text preview with streaming indicator
- toolExecuting: tool cards with status icons

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2.2: 创建 HaloResultView 组件

**Files:**
- Create: `platforms/macos/Aether/Sources/Components/HaloResultView.swift`

**Step 1: Create the result view component**

```swift
//
//  HaloResultView.swift
//  Aether
//
//  Result display view (replaces success state).
//

import SwiftUI

/// View for displaying run results
struct HaloResultView: View {
    let context: ResultContext
    let onDismiss: (() -> Void)?
    let onCopy: (() -> Void)?

    @State private var scale: CGFloat = 0.5
    @State private var opacity: Double = 0.0

    var body: some View {
        HStack(spacing: 10) {
            // Status icon
            Image(systemName: context.summary.status.iconName)
                .font(.system(size: 18, weight: .medium))
                .foregroundColor(context.summary.status.color)
                .scaleEffect(scale)
                .opacity(opacity)

            VStack(alignment: .leading, spacing: 2) {
                // Message or default text
                if let message = context.summary.message {
                    Text(message)
                        .font(.caption)
                        .foregroundColor(.primary)
                        .lineLimit(2)
                } else {
                    Text(defaultMessage)
                        .font(.caption)
                        .foregroundColor(.primary)
                }

                // Stats row
                HStack(spacing: 8) {
                    if context.summary.toolsExecuted > 0 {
                        Label("\(context.summary.toolsExecuted)", systemImage: "wrench")
                            .font(.caption2)
                            .foregroundColor(.secondary)
                    }

                    if context.summary.durationMs > 0 {
                        Text(formattedDuration)
                            .font(.caption2)
                            .foregroundColor(.secondary)
                    }
                }
            }

            Spacer()

            // Copy button (if response available)
            if !context.summary.finalResponse.isEmpty {
                Button(action: { onCopy?() }) {
                    Image(systemName: "doc.on.doc")
                        .font(.caption)
                }
                .buttonStyle(.borderless)
                .help(L("button.copy"))
            }
        }
        .padding(10)
        .background(.ultraThinMaterial)
        .cornerRadius(8)
        .onAppear {
            withAnimation(.spring(response: 0.3, dampingFraction: 0.6)) {
                scale = 1.0
                opacity = 1.0
            }
        }
        .onTapGesture {
            onDismiss?()
        }
    }

    private var defaultMessage: String {
        switch context.summary.status {
        case .success:
            return L("halo.completed")
        case .partial:
            return L("halo.partial_complete")
        case .error:
            return L("error.aether")
        }
    }

    private var formattedDuration: String {
        let seconds = Double(context.summary.durationMs) / 1000.0
        if seconds < 1 {
            return "\(context.summary.durationMs)ms"
        } else if seconds < 60 {
            return String(format: "%.1fs", seconds)
        } else {
            let minutes = Int(seconds / 60)
            let secs = Int(seconds.truncatingRemainder(dividingBy: 60))
            return "\(minutes)m \(secs)s"
        }
    }
}

#if DEBUG
struct HaloResultView_Previews: PreviewProvider {
    static var previews: some View {
        VStack(spacing: 20) {
            // Success
            HaloResultView(
                context: ResultContext(
                    runId: "1",
                    summary: .success(
                        message: "File created successfully",
                        toolsExecuted: 3,
                        durationMs: 1250,
                        finalResponse: "Done!"
                    )
                ),
                onDismiss: nil,
                onCopy: nil
            )

            // Partial
            HaloResultView(
                context: ResultContext(
                    runId: "2",
                    summary: ResultSummary(
                        status: .partial,
                        message: "Completed with warnings",
                        toolsExecuted: 2,
                        tokensUsed: 1500,
                        durationMs: 3200,
                        finalResponse: ""
                    )
                ),
                onDismiss: nil,
                onCopy: nil
            )

            // Error
            HaloResultView(
                context: ResultContext(
                    runId: "3",
                    summary: .error(message: "Network timeout", durationMs: 5000)
                ),
                onDismiss: nil,
                onCopy: nil
            )
        }
        .padding()
        .frame(width: 300)
        .background(Color.gray.opacity(0.2))
    }
}
#endif
```

**Step 2: Verify the file compiles**

Run: `cd /Volumes/TBU4/Workspace/Aether/platforms/macos && xcodebuild -scheme Aether -configuration Debug build -quiet 2>&1 | head -20`

Expected: Build succeeds

**Step 3: Commit**

```bash
git add platforms/macos/Aether/Sources/Components/HaloResultView.swift
git commit -m "$(cat <<'EOF'
feat(halo): add HaloResultView for run completion display

Compact toast-style view showing:
- Status icon with animation
- Message and stats (tools executed, duration)
- Copy button for final response
- Click to dismiss

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2.3: 创建 HaloHistoryListView 组件

**Files:**
- Create: `platforms/macos/Aether/Sources/Components/HaloHistoryListView.swift`

**Step 1: Create the history list view component**

```swift
//
//  HaloHistoryListView.swift
//  Aether
//
//  History list view triggered by // command.
//

import SwiftUI

/// View for displaying conversation history list
struct HaloHistoryListView: View {
    @Binding var context: HistoryListContext
    let onSelect: (HistoryTopic) -> Void
    let onDismiss: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack {
                Image(systemName: "clock.arrow.circlepath")
                    .foregroundColor(.purple)
                Text(L("history.title"))
                    .font(.headline)
                Spacer()
                Button(action: onDismiss) {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundColor(.secondary)
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)

            Divider()

            // Search field
            HStack {
                Image(systemName: "magnifyingglass")
                    .foregroundColor(.secondary)
                TextField(L("history.search"), text: $context.searchQuery)
                    .textFieldStyle(.plain)
                if !context.searchQuery.isEmpty {
                    Button(action: { context.searchQuery = "" }) {
                        Image(systemName: "xmark.circle.fill")
                            .foregroundColor(.secondary)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(8)
            .background(Color.gray.opacity(0.1))
            .cornerRadius(6)
            .padding(.horizontal, 12)
            .padding(.vertical, 8)

            // Topic list
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(groupedTopics, id: \.key) { group in
                        Section {
                            ForEach(group.value) { topic in
                                TopicRow(
                                    topic: topic,
                                    isSelected: context.selectedIndex == context.topics.firstIndex(of: topic)
                                )
                                .onTapGesture {
                                    onSelect(topic)
                                }
                            }
                        } header: {
                            Text(group.key)
                                .font(.caption)
                                .fontWeight(.semibold)
                                .foregroundColor(.secondary)
                                .padding(.horizontal, 12)
                                .padding(.top, 12)
                                .padding(.bottom, 4)
                        }
                    }

                    if context.filteredTopics.isEmpty {
                        Text(L("history.empty"))
                            .font(.caption)
                            .foregroundColor(.secondary)
                            .frame(maxWidth: .infinity)
                            .padding(20)
                    }
                }
            }
        }
        .frame(width: 360, height: 380)
        .background(.ultraThinMaterial)
        .cornerRadius(12)
    }

    private var groupedTopics: [(key: String, value: [HistoryTopic])] {
        let topics = context.filteredTopics
        let calendar = Calendar.current
        let now = Date()

        var groups: [String: [HistoryTopic]] = [:]

        for topic in topics {
            let key: String
            if calendar.isDateInToday(topic.lastMessageAt) {
                key = L("history.today")
            } else if calendar.isDateInYesterday(topic.lastMessageAt) {
                key = L("history.yesterday")
            } else if let weekAgo = calendar.date(byAdding: .day, value: -7, to: now),
                      topic.lastMessageAt > weekAgo {
                key = L("history.this_week")
            } else {
                key = L("history.earlier")
            }

            groups[key, default: []].append(topic)
        }

        // Sort groups by recency
        let order = [L("history.today"), L("history.yesterday"), L("history.this_week"), L("history.earlier")]
        return order.compactMap { key in
            groups[key].map { (key: key, value: $0) }
        }
    }
}

/// Single topic row in the history list
private struct TopicRow: View {
    let topic: HistoryTopic
    let isSelected: Bool

    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(topic.title)
                    .font(.callout)
                    .lineLimit(1)
                HStack(spacing: 4) {
                    Text(topic.relativeTime)
                        .font(.caption2)
                        .foregroundColor(.secondary)
                    Text("•")
                        .font(.caption2)
                        .foregroundColor(.secondary)
                    Text("\(topic.messageCount) messages")
                        .font(.caption2)
                        .foregroundColor(.secondary)
                }
            }
            Spacer()
            Image(systemName: "chevron.right")
                .font(.caption)
                .foregroundColor(.secondary)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(isSelected ? Color.accentColor.opacity(0.1) : Color.clear)
        .contentShape(Rectangle())
    }
}

#if DEBUG
struct HaloHistoryListView_Previews: PreviewProvider {
    static var previews: some View {
        HaloHistoryListView(
            context: .constant({
                var ctx = HistoryListContext()
                ctx.topics = [
                    HistoryTopic(id: "1", title: "代码重构讨论", lastMessageAt: Date(), messageCount: 5),
                    HistoryTopic(id: "2", title: "Bug 修复", lastMessageAt: Date().addingTimeInterval(-3600 * 4), messageCount: 12),
                    HistoryTopic(id: "3", title: "架构设计", lastMessageAt: Date().addingTimeInterval(-3600 * 24), messageCount: 8),
                    HistoryTopic(id: "4", title: "性能优化", lastMessageAt: Date().addingTimeInterval(-3600 * 24 * 3), messageCount: 3),
                ]
                return ctx
            }()),
            onSelect: { _ in },
            onDismiss: {}
        )
        .padding()
        .background(Color.gray.opacity(0.3))
    }
}
#endif
```

**Step 2: Verify the file compiles**

Run: `cd /Volumes/TBU4/Workspace/Aether/platforms/macos && xcodebuild -scheme Aether -configuration Debug build -quiet 2>&1 | head -20`

Expected: Build succeeds

**Step 3: Commit**

```bash
git add platforms/macos/Aether/Sources/Components/HaloHistoryListView.swift
git commit -m "$(cat <<'EOF'
feat(halo): add HaloHistoryListView for // command

History list panel featuring:
- Search field with clear button
- Grouped by time (Today, Yesterday, This Week, Earlier)
- Topic rows with title, relative time, message count
- Click to select and load topic context

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3: 迁移 HaloView 到 V2

### Task 3.1: 创建 HaloViewV2 组件

**Files:**
- Create: `platforms/macos/Aether/Sources/Components/HaloViewV2.swift`

**Step 1: Create the new HaloView**

```swift
//
//  HaloViewV2.swift
//  Aether
//
//  Simplified HaloView with 6 core states.
//

import SwiftUI

/// Main Halo overlay view (V2 - simplified)
struct HaloViewV2: View {
    @ObservedObject var viewModel: HaloViewModelV2

    var body: some View {
        Group {
            switch viewModel.state {
            case .idle:
                EmptyView()

            case .listening:
                HaloListeningView()

            case .streaming(let context):
                HaloStreamingView(context: context)

            case .confirmation(let context):
                HaloConfirmationViewV2(
                    context: context,
                    onConfirm: { viewModel.callbacks.onConfirm?($0) },
                    onCancel: { viewModel.callbacks.onCancel?() }
                )

            case .result(let context):
                HaloResultView(
                    context: context,
                    onDismiss: { viewModel.callbacks.onDismiss?() },
                    onCopy: { viewModel.callbacks.onCopy?() }
                )

            case .error(let context):
                HaloErrorViewV2(
                    context: context,
                    onRetry: context.canRetry ? { viewModel.callbacks.onRetry?() } : nil,
                    onDismiss: { viewModel.callbacks.onDismiss?() }
                )

            case .historyList(let context):
                HaloHistoryListView(
                    context: Binding(
                        get: { context },
                        set: { viewModel.updateHistoryContext($0) }
                    ),
                    onSelect: { viewModel.callbacks.onHistorySelect?($0) },
                    onDismiss: { viewModel.callbacks.onDismiss?() }
                )
            }
        }
        .animation(.easeInOut(duration: 0.2), value: viewModel.stateIdentifier)
    }
}

// MARK: - ViewModel

/// View model for HaloViewV2 state management
class HaloViewModelV2: ObservableObject {
    @Published var state: HaloStateV2 = .idle
    let callbacks = HaloCallbacksV2()

    /// Identifier for animation purposes
    var stateIdentifier: String {
        switch state {
        case .idle: return "idle"
        case .listening: return "listening"
        case .streaming: return "streaming"
        case .confirmation: return "confirmation"
        case .result: return "result"
        case .error: return "error"
        case .historyList: return "historyList"
        }
    }

    func updateHistoryContext(_ context: HistoryListContext) {
        if case .historyList = state {
            state = .historyList(context)
        }
    }
}

/// Callbacks for HaloViewV2
class HaloCallbacksV2 {
    var onConfirm: ((String) -> Void)?  // Option ID
    var onCancel: (() -> Void)?
    var onRetry: (() -> Void)?
    var onDismiss: (() -> Void)?
    var onCopy: (() -> Void)?
    var onHistorySelect: ((HistoryTopic) -> Void)?

    func reset() {
        onConfirm = nil
        onCancel = nil
        onRetry = nil
        onDismiss = nil
        onCopy = nil
        onHistorySelect = nil
    }
}

// MARK: - Confirmation View V2

struct HaloConfirmationViewV2: View {
    let context: ConfirmationContext
    let onConfirm: (String) -> Void
    let onCancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            // Icon and title
            HStack(spacing: 8) {
                Image(systemName: iconName)
                    .font(.title2)
                    .foregroundColor(iconColor)
                Text(context.title)
                    .font(.headline)
            }

            // Description
            if !context.description.isEmpty {
                Text(context.description)
                    .font(.callout)
                    .foregroundColor(.secondary)
                    .lineLimit(5)
            }

            Spacer()

            // Buttons
            HStack(spacing: 8) {
                ForEach(context.options) { option in
                    Button(action: { onConfirm(option.id) }) {
                        Text(option.label)
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(option.isDefault ? .borderedProminent : .bordered)
                    .tint(option.isDestructive ? .red : nil)
                    .controlSize(.regular)
                }
            }
        }
        .padding(16)
        .frame(maxWidth: 320, maxHeight: 260)
        .background(.ultraThinMaterial)
        .cornerRadius(12)
    }

    private var iconName: String {
        switch context.type {
        case .toolExecution: return "wrench.and.screwdriver.fill"
        case .planApproval: return "list.bullet.clipboard.fill"
        case .fileConflict: return "exclamationmark.triangle.fill"
        case .userQuestion: return "questionmark.circle.fill"
        }
    }

    private var iconColor: Color {
        switch context.type {
        case .toolExecution: return .purple
        case .planApproval: return .blue
        case .fileConflict: return .orange
        case .userQuestion: return .green
        }
    }
}

// MARK: - Error View V2

struct HaloErrorViewV2: View {
    let context: ErrorContext
    let onRetry: (() -> Void)?
    let onDismiss: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            // Icon and title
            HStack(spacing: 8) {
                Image(systemName: context.type.iconName)
                    .font(.title2)
                    .foregroundColor(.red)
                Text(L("error.aether"))
                    .font(.headline)
            }

            // Message
            Text(context.message)
                .font(.callout)
                .foregroundColor(.secondary)
                .lineLimit(4)

            // Suggestion
            if let suggestion = context.suggestion {
                Text(suggestion)
                    .font(.caption)
                    .foregroundColor(.secondary)
            }

            Spacer()

            // Buttons
            HStack(spacing: 8) {
                if let onRetry = onRetry {
                    Button(action: onRetry) {
                        Text(L("button.retry"))
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.regular)
                }

                Button(action: onDismiss) {
                    Text(L("button.dismiss"))
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.bordered)
                .controlSize(.regular)
            }
        }
        .padding(16)
        .frame(maxWidth: 300, maxHeight: 200)
        .background(.ultraThinMaterial)
        .cornerRadius(12)
    }
}

#if DEBUG
struct HaloViewV2_Previews: PreviewProvider {
    static var previews: some View {
        VStack(spacing: 20) {
            // Listening
            HaloViewV2(viewModel: {
                let vm = HaloViewModelV2()
                vm.state = .listening
                return vm
            }())
            .frame(width: 80, height: 60)

            // Streaming
            HaloViewV2(viewModel: {
                let vm = HaloViewModelV2()
                var ctx = StreamingContext(runId: "1", phase: .responding)
                ctx.text = "This is a sample streaming response..."
                vm.state = .streaming(ctx)
                return vm
            }())
            .frame(width: 320, height: 100)

            // Confirmation
            HaloViewV2(viewModel: {
                let vm = HaloViewModelV2()
                vm.state = .confirmation(ConfirmationContext(
                    runId: "1",
                    type: .toolExecution,
                    title: "Execute shell command",
                    description: "This will run 'ls -la' in the current directory"
                ))
                return vm
            }())
            .frame(width: 340, height: 280)
        }
        .padding()
        .background(Color.gray.opacity(0.3))
    }
}
#endif
```

**Step 2: Verify the file compiles**

Run: `cd /Volumes/TBU4/Workspace/Aether/platforms/macos && xcodebuild -scheme Aether -configuration Debug build -quiet 2>&1 | head -20`

Expected: Build succeeds

**Step 3: Commit**

```bash
git add platforms/macos/Aether/Sources/Components/HaloViewV2.swift
git commit -m "$(cat <<'EOF'
feat(halo): add HaloViewV2 with simplified 6-state switch

Complete HaloViewV2 implementation with:
- HaloViewModelV2 for state management
- HaloCallbacksV2 for unified callback handling
- HaloConfirmationViewV2 for all confirmation types
- HaloErrorViewV2 for error display
- Integration with HaloStreamingView, HaloResultView, HaloHistoryListView

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4: 删除旧代码

### Task 4.1: 删除 MultiTurn 目录

**Files:**
- Delete: `platforms/macos/Aether/Sources/MultiTurn/` (entire directory)

**Step 1: Remove the MultiTurn directory**

```bash
rm -rf /Volumes/TBU4/Workspace/Aether/platforms/macos/Aether/Sources/MultiTurn
```

**Step 2: Update project.yml to remove MultiTurn references**

Search for and remove any references to MultiTurn files in `platforms/macos/project.yml`

**Step 3: Verify build still works**

Run: `cd /Volumes/TBU4/Workspace/Aether/platforms/macos && xcodegen generate && xcodebuild -scheme Aether -configuration Debug build -quiet 2>&1 | head -30`

Expected: Build may fail due to missing references - this is expected, we'll fix in next task

**Step 4: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
refactor(halo): remove MultiTurn directory

Deletes ~3000 lines of conversation window code:
- UnifiedConversationWindow.swift (399 lines)
- UnifiedConversationView.swift (268 lines)
- UnifiedConversationViewModel.swift (1610 lines)
- MultiTurnCoordinator.swift (724 lines)

This is part of the Halo-Only refactor - all interactions now go through Halo.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4.2: 更新 EventHandler 移除 MultiTurn 引用

**Files:**
- Modify: `platforms/macos/Aether/Sources/EventHandler.swift`

**Step 1: Remove MultiTurn mode checks and forwarding**

Remove or comment out all code paths that check `isInMultiTurnMode` and forward to `MultiTurnCoordinator`. Replace with direct Halo state updates.

Key changes:
1. Remove `isInMultiTurnMode` property
2. Remove all `MultiTurnCoordinator.shared` calls
3. Update callbacks to use Halo directly

**Step 2: Verify build**

Run: `cd /Volumes/TBU4/Workspace/Aether/platforms/macos && xcodebuild -scheme Aether -configuration Debug build -quiet 2>&1 | head -20`

**Step 3: Commit**

```bash
git add platforms/macos/Aether/Sources/EventHandler.swift
git commit -m "$(cat <<'EOF'
refactor(halo): remove MultiTurn mode from EventHandler

All callbacks now update Halo directly instead of checking multi-turn mode.
Removes ~200 lines of conditional forwarding code.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4.3: 切换 HaloWindow 到 V2

**Files:**
- Modify: `platforms/macos/Aether/Sources/HaloWindow.swift`

**Step 1: Update HaloWindow to use HaloViewModelV2**

Replace:
- `viewModel: HaloViewModel` with `viewModel: HaloViewModelV2`
- `HaloView(viewModel: viewModel)` with `HaloViewV2(viewModel: viewModel)`
- `updateState(_ state: HaloState)` with `updateState(_ state: HaloStateV2)`
- Remove old `updateWindowSize()` - use `state.windowSize` directly

**Step 2: Verify build**

Run: `cd /Volumes/TBU4/Workspace/Aether/platforms/macos && xcodebuild -scheme Aether -configuration Debug build -quiet 2>&1 | head -20`

**Step 3: Commit**

```bash
git add platforms/macos/Aether/Sources/HaloWindow.swift
git commit -m "$(cat <<'EOF'
refactor(halo): switch HaloWindow to V2 state model

- Uses HaloViewModelV2 and HaloViewV2
- Window size now computed from state.windowSize
- Simplified updateState() method
- Removed complex switch for window sizing

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4.4: 清理旧 HaloState 枚举

**Files:**
- Modify: `platforms/macos/Aether/Sources/HaloState.swift`

**Step 1: Remove old HaloState enum**

Keep only:
- `HaloStateV2` (rename to `HaloState`)
- Supporting types used by V2
- Remove old `HaloState` enum and all its supporting types

**Step 2: Update all references**

Search and replace `HaloStateV2` → `HaloState` across the codebase

**Step 3: Verify build**

Run: `cd /Volumes/TBU4/Workspace/Aether/platforms/macos && xcodebuild -scheme Aether -configuration Debug build -quiet 2>&1 | head -20`

**Step 4: Commit**

```bash
git add platforms/macos/Aether/Sources/HaloState.swift
git commit -m "$(cat <<'EOF'
refactor(halo): replace HaloState with simplified V2 version

Final migration step - old HaloState with 16+ variants is now replaced
by the simplified 6-state model. Removes ~300 lines of old state code.

States: idle, listening, streaming, confirmation, result, error, historyList

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5: Gateway 事件简化

### Task 5.1: 添加 Delta 节流机制

**Files:**
- Modify: `core/src/gateway/event_emitter.rs`

**Step 1: Add throttling to GatewayEventEmitter**

Add 150ms throttling for Delta events (similar to OpenClaw):

```rust
use std::time::Instant;
use tokio::sync::Mutex;

pub struct GatewayEventEmitter {
    event_bus: Arc<GatewayEventBus>,
    seq_counter: AtomicU64,
    // Throttling state
    delta_buffer: Mutex<String>,
    last_delta_at: Mutex<Instant>,
}

impl GatewayEventEmitter {
    const DELTA_THROTTLE_MS: u64 = 150;

    pub fn new(event_bus: Arc<GatewayEventBus>) -> Self {
        Self {
            event_bus,
            seq_counter: AtomicU64::new(0),
            delta_buffer: Mutex::new(String::new()),
            last_delta_at: Mutex::new(Instant::now()),
        }
    }

    /// Emit response chunk with throttling
    pub async fn emit_response_chunk_throttled(
        &self,
        run_id: &str,
        content: &str,
        chunk_index: u32,
        is_final: bool,
    ) {
        if is_final {
            // Always send final chunk immediately with any buffered content
            let mut buffer = self.delta_buffer.lock().await;
            let full_content = if buffer.is_empty() {
                content.to_string()
            } else {
                let buffered = std::mem::take(&mut *buffer);
                format!("{}{}", buffered, content)
            };
            drop(buffer);

            self.emit_response_chunk(run_id, &full_content, chunk_index, true).await;
            return;
        }

        let now = Instant::now();
        let mut last_at = self.last_delta_at.lock().await;
        let elapsed = now.duration_since(*last_at).as_millis() as u64;

        if elapsed < Self::DELTA_THROTTLE_MS {
            // Buffer the content
            self.delta_buffer.lock().await.push_str(content);
            return;
        }

        // Send buffered + new content
        let mut buffer = self.delta_buffer.lock().await;
        let full_content = if buffer.is_empty() {
            content.to_string()
        } else {
            let buffered = std::mem::take(&mut *buffer);
            format!("{}{}", buffered, content)
        };
        drop(buffer);

        *last_at = now;
        drop(last_at);

        self.emit_response_chunk(run_id, &full_content, chunk_index, false).await;
    }
}
```

**Step 2: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test gateway::event_emitter`

**Step 3: Commit**

```bash
git add core/src/gateway/event_emitter.rs
git commit -m "$(cat <<'EOF'
feat(gateway): add 150ms throttling for response chunks

Implements OpenClaw-style delta throttling to reduce WebSocket traffic:
- Buffers chunks within 150ms window
- Sends accumulated content on throttle boundary
- Always sends final chunk immediately with any buffered content

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 6: 集成测试

### Task 6.1: 手动测试清单

**Test Cases:**

1. **基本流程测试**
   - [ ] 启动应用，Halo 初始为 idle
   - [ ] 触发 AI 请求，观察 streaming → thinking
   - [ ] 输出开始，观察 streaming → responding
   - [ ] 完成后观察 result 状态，2秒后自动消失

2. **工具执行测试**
   - [ ] 触发需要工具的请求
   - [ ] 观察 streaming → toolExecuting
   - [ ] 工具卡片正确显示名称和状态

3. **确认流程测试**
   - [ ] 触发需要确认的操作
   - [ ] 观察 confirmation 状态
   - [ ] 点击确认/取消，验证回调

4. **错误处理测试**
   - [ ] 触发错误情况
   - [ ] 观察 error 状态
   - [ ] 点击重试/取消

5. **历史列表测试**
   - [ ] 输入 // 命令
   - [ ] 观察 historyList 状态
   - [ ] 搜索功能
   - [ ] 选择主题

---

## Summary

**Total Tasks:** 11 tasks across 6 phases

**Estimated Code Changes:**
- New code: ~1000 lines
- Deleted code: ~3500 lines
- Net reduction: ~2500 lines

**Key Files Modified:**
- `HaloState.swift` - 16+ states → 6 states
- `HaloView.swift` → `HaloViewV2.swift`
- `HaloWindow.swift` - simplified sizing
- `EventHandler.swift` - removed MultiTurn paths
- `event_emitter.rs` - added throttling

**Deleted Files:**
- `MultiTurn/UnifiedConversationWindow.swift`
- `MultiTurn/UnifiedConversationView.swift`
- `MultiTurn/UnifiedConversationViewModel.swift`
- `MultiTurn/MultiTurnCoordinator.swift`

**New Files:**
- `Components/HaloStreamingTypes.swift`
- `Components/HaloStreamingView.swift`
- `Components/HaloResultView.swift`
- `Components/HaloHistoryListView.swift`
- `Components/HaloViewV2.swift`
