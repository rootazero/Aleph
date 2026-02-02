//
//  HaloViewV2.swift
//  Aether
//
//  Main Halo overlay view (V2) that integrates all state-specific components.
//  Uses the simplified 7-state HaloState model.
//

import SwiftUI
import Combine

// MARK: - HaloViewV2

/// Main Halo overlay view (V2) with simplified state model
///
/// Features:
/// - 7 unified states: idle, listening, streaming, confirmation, result, error, historyList
/// - Integrated components for each state
/// - Smooth state transitions with animations
///
/// Usage:
/// ```swift
/// HaloViewV2(viewModel: haloViewModelV2)
/// ```
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
                    onConfirm: { optionId in
                        viewModel.callbacks.onConfirm?(optionId)
                    },
                    onCancel: {
                        viewModel.callbacks.onCancel?()
                    }
                )

            case .result(let context):
                HaloResultView(
                    context: context,
                    onDismiss: {
                        viewModel.callbacks.onDismiss?()
                    },
                    onCopy: {
                        viewModel.callbacks.onCopy?()
                    }
                )

            case .error(let context):
                HaloErrorViewV2(
                    context: context,
                    onRetry: context.canRetry ? {
                        viewModel.callbacks.onRetry?()
                    } : nil,
                    onDismiss: {
                        viewModel.callbacks.onDismiss?()
                    }
                )

            case .historyList(let context):
                HaloHistoryListView(
                    context: Binding(
                        get: {
                            if case .historyList(let ctx) = viewModel.state {
                                return ctx
                            }
                            return context
                        },
                        set: { newContext in
                            viewModel.updateHistoryContext(newContext)
                        }
                    ),
                    onSelect: { topic in
                        viewModel.callbacks.onHistorySelect?(topic)
                    },
                    onDismiss: {
                        viewModel.callbacks.onDismiss?()
                    }
                )
            }
        }
        .animation(.easeInOut(duration: 0.2), value: viewModel.stateIdentifier)
    }
}

// MARK: - HaloViewModelV2

/// View model for HaloViewV2 state management
///
/// Features:
/// - Published state for SwiftUI binding
/// - Centralized callbacks for all user interactions
/// - State identifier for animation coordination
class HaloViewModelV2: ObservableObject {
    @Published var state: HaloState = .idle
    let callbacks = HaloCallbacksV2()

    /// Unique string identifier for current state (for animation)
    var stateIdentifier: String {
        switch state {
        case .idle:
            return "idle"
        case .listening:
            return "listening"
        case .streaming(let ctx):
            return "streaming-\(ctx.runId)"
        case .confirmation(let ctx):
            return "confirmation-\(ctx.runId)"
        case .result(let ctx):
            return "result-\(ctx.runId)"
        case .error(let ctx):
            return "error-\(ctx.runId ?? "unknown")"
        case .historyList:
            return "historyList"
        }
    }

    /// Update history context (for search query and selection changes)
    func updateHistoryContext(_ context: HistoryListContext) {
        if case .historyList = state {
            state = .historyList(context)
        }
    }

    /// Reset to idle state and clear all callbacks
    func reset() {
        state = .idle
        callbacks.reset()
    }
}

// MARK: - HaloCallbacksV2

/// Centralized callbacks for HaloViewV2 user interactions
///
/// All callbacks are optional and can be set by the parent controller.
/// Call `reset()` when transitioning to idle to clean up.
class HaloCallbacksV2 {
    /// Called when user confirms with a specific option ID
    var onConfirm: ((String) -> Void)?

    /// Called when user cancels the current operation
    var onCancel: (() -> Void)?

    /// Called when user requests to retry after an error
    var onRetry: (() -> Void)?

    /// Called when user dismisses a result or error
    var onDismiss: (() -> Void)?

    /// Called when user copies the result to clipboard
    var onCopy: (() -> Void)?

    /// Called when user selects a history topic
    var onHistorySelect: ((HistoryTopic) -> Void)?

    /// Reset all callbacks to nil
    func reset() {
        onConfirm = nil
        onCancel = nil
        onRetry = nil
        onDismiss = nil
        onCopy = nil
        onHistorySelect = nil
    }
}

// MARK: - HaloConfirmationViewV2

/// Confirmation dialog view for V2 state model
///
/// Features:
/// - Dynamic icon and color based on confirmation type
/// - Title and description display
/// - Option buttons with destructive styling support
///
/// Usage:
/// ```swift
/// HaloConfirmationViewV2(
///     context: confirmationContext,
///     onConfirm: { optionId in /* handle */ },
///     onCancel: { /* handle */ }
/// )
/// ```
struct HaloConfirmationViewV2: View {
    let context: ConfirmationContext
    let onConfirm: (String) -> Void
    let onCancel: () -> Void

    @State private var isAppearing = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            // Icon and title
            HStack(spacing: 8) {
                Image(systemName: iconName)
                    .font(.system(size: 18, weight: .medium))
                    .foregroundColor(iconColor)

                Text(context.title)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(.primary)
                    .lineLimit(2)
            }

            // Description
            Text(context.description)
                .font(.system(size: 12))
                .foregroundColor(.secondary)
                .lineLimit(6)
                .fixedSize(horizontal: false, vertical: true)

            Spacer(minLength: 8)

            // Option buttons
            HStack(spacing: 8) {
                ForEach(context.options) { option in
                    Button(action: {
                        onConfirm(option.id)
                    }) {
                        Text(option.label)
                            .font(.system(size: 12, weight: .medium))
                            .padding(.horizontal, 12)
                            .padding(.vertical, 6)
                    }
                    .buttonStyle(.bordered)
                    .tint(option.isDestructive ? .red : (option.isDefault ? .accentColor : nil))
                }

                Spacer()

                Button(action: onCancel) {
                    Text(L("button.cancel"))
                        .font(.system(size: 12, weight: .medium))
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                }
                .buttonStyle(.bordered)
            }
        }
        .padding(16)
        .frame(maxWidth: 320, maxHeight: 260)
        .background(.ultraThinMaterial)
        .cornerRadius(12)
        .scaleEffect(isAppearing ? 1.0 : 0.95)
        .opacity(isAppearing ? 1.0 : 0.0)
        .onAppear {
            withAnimation(.spring(response: 0.3, dampingFraction: 0.8)) {
                isAppearing = true
            }
        }
    }

    // MARK: - Private Computed Properties

    /// SF Symbol icon name based on confirmation type
    private var iconName: String {
        switch context.type {
        case .toolExecution:
            return "wrench.and.screwdriver.fill"
        case .planApproval:
            return "list.bullet.clipboard.fill"
        case .fileConflict:
            return "exclamationmark.triangle.fill"
        case .userQuestion:
            return "questionmark.circle.fill"
        }
    }

    /// Icon color based on confirmation type
    private var iconColor: Color {
        switch context.type {
        case .toolExecution:
            return .purple
        case .planApproval:
            return .blue
        case .fileConflict:
            return .orange
        case .userQuestion:
            return .green
        }
    }
}

// MARK: - HaloErrorViewV2

/// Error view for V2 state model
///
/// Features:
/// - Dynamic icon based on error type
/// - Message and optional suggestion display
/// - Retry button (if error is retryable)
/// - Dismiss button
///
/// Usage:
/// ```swift
/// HaloErrorViewV2(
///     context: errorContext,
///     onRetry: { /* handle retry */ },
///     onDismiss: { /* handle dismiss */ }
/// )
/// ```
struct HaloErrorViewV2: View {
    let context: ErrorContext
    let onRetry: (() -> Void)?
    let onDismiss: () -> Void

    @State private var isAppearing = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            // Icon and title
            HStack(spacing: 8) {
                Image(systemName: context.type.iconName)
                    .font(.system(size: 18, weight: .medium))
                    .foregroundColor(.red)

                Text(L("error.aether"))
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(.primary)
            }

            // Message
            Text(context.message)
                .font(.system(size: 12))
                .foregroundColor(.secondary)
                .lineLimit(4)
                .fixedSize(horizontal: false, vertical: true)

            // Suggestion (if present)
            if let suggestion = context.suggestion {
                Text(suggestion)
                    .font(.system(size: 11))
                    .foregroundColor(.secondary.opacity(0.8))
                    .italic()
                    .lineLimit(2)
            }

            Spacer(minLength: 8)

            // Action buttons
            HStack(spacing: 8) {
                if let onRetry = onRetry {
                    Button(action: onRetry) {
                        Text(L("button.retry"))
                            .font(.system(size: 12, weight: .medium))
                            .padding(.horizontal, 12)
                            .padding(.vertical, 6)
                    }
                    .buttonStyle(.borderedProminent)
                }

                Button(action: onDismiss) {
                    Text(L("button.dismiss"))
                        .font(.system(size: 12, weight: .medium))
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                }
                .buttonStyle(.bordered)

                Spacer()
            }
        }
        .padding(16)
        .frame(maxWidth: 300, maxHeight: 200)
        .background(.ultraThinMaterial)
        .cornerRadius(12)
        .scaleEffect(isAppearing ? 1.0 : 0.95)
        .opacity(isAppearing ? 1.0 : 0.0)
        .onAppear {
            withAnimation(.spring(response: 0.3, dampingFraction: 0.8)) {
                isAppearing = true
            }
        }
    }
}

// MARK: - Previews

#if DEBUG
#Preview("Listening State") {
    ZStack {
        Color.black.opacity(0.8)
        HaloViewV2(viewModel: {
            let vm = HaloViewModelV2()
            vm.state = .listening
            return vm
        }())
    }
    .frame(width: 200, height: 100)
}

#Preview("Streaming - Thinking") {
    ZStack {
        Color.black.opacity(0.8)
        HaloViewV2(viewModel: {
            let vm = HaloViewModelV2()
            vm.state = .streaming(StreamingContext(
                runId: "preview-1",
                reasoning: "Analyzing your request...",
                phase: .thinking
            ))
            return vm
        }())
    }
    .frame(width: 360, height: 150)
}

#Preview("Streaming - Responding") {
    ZStack {
        Color.black.opacity(0.8)
        HaloViewV2(viewModel: {
            let vm = HaloViewModelV2()
            vm.state = .streaming(StreamingContext(
                runId: "preview-2",
                text: "Here is my response to your question about implementing the new feature...",
                phase: .responding
            ))
            return vm
        }())
    }
    .frame(width: 360, height: 200)
}

#Preview("Streaming - Tool Executing") {
    ZStack {
        Color.black.opacity(0.8)
        HaloViewV2(viewModel: {
            let vm = HaloViewModelV2()
            vm.state = .streaming(StreamingContext(
                runId: "preview-3",
                toolCalls: [
                    ToolCallInfo(id: "1", name: "read_file", status: .completed),
                    ToolCallInfo(id: "2", name: "search_code", status: .running, progressText: "Searching..."),
                    ToolCallInfo(id: "3", name: "write_file", status: .pending)
                ],
                phase: .toolExecuting
            ))
            return vm
        }())
    }
    .frame(width: 360, height: 200)
}

#Preview("Confirmation - Tool Execution") {
    ZStack {
        Color.black.opacity(0.8)
        HaloViewV2(viewModel: {
            let vm = HaloViewModelV2()
            vm.state = .confirmation(ConfirmationContext(
                runId: "preview-confirm-1",
                type: .toolExecution,
                title: "Execute Shell Command",
                description: "This will run: rm -rf /tmp/cache\n\nThis operation will delete all files in the cache directory.",
                options: ConfirmationContext.defaultOptions(for: .toolExecution)
            ))
            return vm
        }())
    }
    .frame(width: 400, height: 320)
}

#Preview("Confirmation - Plan Approval") {
    ZStack {
        Color.black.opacity(0.8)
        HaloViewV2(viewModel: {
            let vm = HaloViewModelV2()
            vm.state = .confirmation(ConfirmationContext(
                runId: "preview-confirm-2",
                type: .planApproval,
                title: "Refactor Authentication Module",
                description: "1. Extract login logic to separate file\n2. Add unit tests\n3. Update imports",
                options: ConfirmationContext.defaultOptions(for: .planApproval)
            ))
            return vm
        }())
    }
    .frame(width: 400, height: 320)
}

#Preview("Confirmation - File Conflict") {
    ZStack {
        Color.black.opacity(0.8)
        HaloViewV2(viewModel: {
            let vm = HaloViewModelV2()
            vm.state = .confirmation(ConfirmationContext(
                runId: "preview-confirm-3",
                type: .fileConflict,
                title: "File Already Exists",
                description: "The file 'config.json' already exists at the target location.",
                options: ConfirmationContext.defaultOptions(for: .fileConflict)
            ))
            return vm
        }())
    }
    .frame(width: 400, height: 320)
}

#Preview("Confirmation - User Question") {
    ZStack {
        Color.black.opacity(0.8)
        HaloViewV2(viewModel: {
            let vm = HaloViewModelV2()
            vm.state = .confirmation(ConfirmationContext(
                runId: "preview-confirm-4",
                type: .userQuestion,
                title: "Continue with changes?",
                description: "Would you like me to proceed with the suggested modifications?",
                options: ConfirmationContext.defaultOptions(for: .userQuestion)
            ))
            return vm
        }())
    }
    .frame(width: 400, height: 320)
}

#Preview("Result - Success") {
    ZStack {
        Color.black.opacity(0.8)
        HaloViewV2(viewModel: {
            let vm = HaloViewModelV2()
            vm.state = .result(ResultContext(
                runId: "preview-result-1",
                summary: .success(
                    message: "Task completed successfully",
                    toolsExecuted: 3,
                    durationMs: 2500,
                    finalResponse: "The operation completed without errors."
                )
            ))
            return vm
        }())
    }
    .frame(width: 360, height: 120)
}

#Preview("Error - Network") {
    ZStack {
        Color.black.opacity(0.8)
        HaloViewV2(viewModel: {
            let vm = HaloViewModelV2()
            vm.state = .error(ErrorContext(
                type: .network,
                message: "Unable to connect to the server. Please check your internet connection.",
                suggestion: "Try again in a few moments."
            ))
            return vm
        }())
    }
    .frame(width: 380, height: 260)
}

#Preview("Error - Provider") {
    ZStack {
        Color.black.opacity(0.8)
        HaloViewV2(viewModel: {
            let vm = HaloViewModelV2()
            vm.state = .error(ErrorContext(
                type: .provider,
                message: "The AI provider returned an unexpected error.",
                suggestion: "Try switching to a different model.",
                canRetry: true
            ))
            return vm
        }())
    }
    .frame(width: 380, height: 260)
}

#Preview("Error - Tool Failure (No Retry)") {
    ZStack {
        Color.black.opacity(0.8)
        HaloViewV2(viewModel: {
            let vm = HaloViewModelV2()
            vm.state = .error(ErrorContext(
                type: .toolFailure,
                message: "The shell command failed with exit code 1.",
                canRetry: false
            ))
            return vm
        }())
    }
    .frame(width: 380, height: 260)
}

#Preview("All States Overview") {
    ScrollView {
        VStack(spacing: 40) {
            // Listening
            VStack {
                Text("Listening").font(.caption).foregroundColor(.gray)
                HaloListeningView()
            }

            // Streaming - Thinking
            VStack {
                Text("Streaming (Thinking)").font(.caption).foregroundColor(.gray)
                HaloStreamingView(
                    context: StreamingContext(
                        runId: "all-1",
                        reasoning: "Analyzing...",
                        phase: .thinking
                    )
                )
            }

            // Streaming - Responding
            VStack {
                Text("Streaming (Responding)").font(.caption).foregroundColor(.gray)
                HaloStreamingView(
                    context: StreamingContext(
                        runId: "all-2",
                        text: "Here is my response...",
                        phase: .responding
                    )
                )
            }

            // Confirmation
            VStack {
                Text("Confirmation").font(.caption).foregroundColor(.gray)
                HaloConfirmationViewV2(
                    context: ConfirmationContext(
                        runId: "all-3",
                        type: .toolExecution,
                        title: "Execute Command",
                        description: "Run shell command",
                        options: ConfirmationContext.defaultOptions(for: .toolExecution)
                    ),
                    onConfirm: { _ in },
                    onCancel: {}
                )
            }

            // Result
            VStack {
                Text("Result").font(.caption).foregroundColor(.gray)
                HaloResultView(
                    context: ResultContext(
                        runId: "all-4",
                        summary: .success(
                            message: "Completed",
                            toolsExecuted: 2,
                            durationMs: 1500,
                            finalResponse: "Done"
                        )
                    ),
                    onDismiss: nil,
                    onCopy: nil
                )
                .frame(maxWidth: 300)
            }

            // Error
            VStack {
                Text("Error").font(.caption).foregroundColor(.gray)
                HaloErrorViewV2(
                    context: ErrorContext(
                        type: .network,
                        message: "Connection failed",
                        suggestion: "Check network"
                    ),
                    onRetry: {},
                    onDismiss: {}
                )
            }
        }
        .padding(20)
    }
    .background(Color.black.opacity(0.8))
    .frame(width: 420, height: 900)
}
#endif
