//
//  HaloView.swift
//  Aether
//
//  Simplified Halo overlay view without theme support.
//  Uses unified visual style with 16x16 purple spinner.
//

import SwiftUI
import Combine

// MARK: - HaloView

/// Main Halo overlay view (simplified, no themes)
struct HaloView: View {
    @ObservedObject var viewModel: HaloViewModel

    var body: some View {
        Group {
            switch viewModel.state {
            case .idle:
                EmptyView()

            case .listening:
                HaloListeningView()

            case .retrievingMemory:
                HaloProcessingView(text: L("halo.retrieving_memory"))

            case .processingWithAI(let providerName):
                HaloProcessingView(text: providerName)

            case .processing(let streamingText):
                HaloProcessingView(text: streamingText)

            case .typewriting(let progress):
                HaloTypewritingView(progress: progress)

            case .success(let message):
                HaloSuccessView(message: message)

            case .error(let type, let message, let suggestion):
                HaloErrorView(
                    errorType: type,
                    message: message,
                    suggestion: suggestion,
                    onRetry: viewModel.callbacks.toolConfirmationOnExecute,
                    onDismiss: viewModel.callbacks.toastOnDismiss
                )

            case .toast(let type, let title, let message, _):
                HaloToastView(
                    type: type,
                    title: title,
                    message: message,
                    onDismiss: viewModel.callbacks.toastOnDismiss
                )

            case .clarification(let request):
                HaloClarificationView(
                    request: request,
                    onSubmit: { _ in },
                    onCancel: viewModel.callbacks.toastOnDismiss
                )

            case .conversationInput:
                // Handled by MultiTurnInputWindow, not HaloView
                EmptyView()

            case .toolConfirmation(_, let toolName, let toolDescription, let reason, let confidence):
                HaloToolConfirmationView(
                    toolName: toolName,
                    toolDescription: toolDescription,
                    reason: reason,
                    confidence: confidence,
                    onExecute: viewModel.callbacks.toolConfirmationOnExecute,
                    onCancel: viewModel.callbacks.toolConfirmationOnCancel
                )

            case .planConfirmation(let planInfo):
                PlanConfirmationView(
                    planInfo: planInfo,
                    onExecute: { viewModel.callbacks.planConfirmationOnExecute?() },
                    onCancel: { viewModel.callbacks.planConfirmationOnCancel?() }
                )

            case .planProgress(let progressInfo):
                PlanProgressView(
                    progressInfo: progressInfo,
                    onCancel: viewModel.callbacks.planConfirmationOnCancel
                )
            }
        }
        .animation(.easeInOut(duration: 0.2), value: viewModel.state)
    }
}

// MARK: - HaloViewModel

/// View model for HaloView state management
class HaloViewModel: ObservableObject {
    @Published var state: HaloState = .idle
    let callbacks = HaloStateCallbacks()
}

// MARK: - Component Views

/// 16x16 purple spinner for processing states (no background)
struct HaloProcessingView: View {
    var text: String?

    var body: some View {
        ArcSpinner()
    }
}

/// Listening state view (pulsing circle, no background)
struct HaloListeningView: View {
    @State private var scale: CGFloat = 1.0

    var body: some View {
        Circle()
            .fill(Color.purple.opacity(0.6))
            .frame(width: 12, height: 12)
            .scaleEffect(scale)
            .onAppear {
                withAnimation(.easeInOut(duration: 0.8).repeatForever(autoreverses: true)) {
                    scale = 1.3
                }
            }
    }
}

/// Typewriting progress view (no background)
struct HaloTypewritingView: View {
    let progress: Float

    var body: some View {
        VStack(spacing: 4) {
            Image(systemName: "keyboard")
                .font(.system(size: 14))
                .foregroundColor(.purple)

            ProgressView(value: Double(progress))
                .progressViewStyle(.linear)
                .frame(width: 60)
        }
    }
}

/// Success state view with checkmark (for OCR and other quick operations)
struct HaloSuccessView: View {
    var message: String?
    @State private var scale: CGFloat = 0.5
    @State private var opacity: Double = 0.0

    var body: some View {
        Image(systemName: "checkmark.circle.fill")
            .font(.system(size: 16, weight: .medium))
            .foregroundColor(.green)
            .scaleEffect(scale)
            .opacity(opacity)
            .onAppear {
                withAnimation(.spring(response: 0.3, dampingFraction: 0.6)) {
                    scale = 1.0
                    opacity = 1.0
                }
            }
    }
}

/// Error view with action buttons
struct HaloErrorView: View {
    let errorType: ErrorType
    let message: String
    let suggestion: String?
    let onRetry: (() -> Void)?
    let onDismiss: (() -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundColor(.red)
                Text(L("error.aether"))
                    .font(.headline)
            }

            Text(message)
                .font(.caption)
                .foregroundColor(.secondary)
                .lineLimit(3)

            if let suggestion = suggestion {
                Text(suggestion)
                    .font(.caption2)
                    .foregroundColor(.secondary)
            }

            HStack(spacing: 8) {
                if onRetry != nil {
                    Button(L("button.retry")) {
                        onRetry?()
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.small)
                }

                Button(L("button.dismiss")) {
                    onDismiss?()
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
            }
        }
        .padding(12)
        .frame(maxWidth: 280)
        .background(.ultraThinMaterial)
        .cornerRadius(8)
    }
}

/// Tool confirmation view
struct HaloToolConfirmationView: View {
    let toolName: String
    let toolDescription: String
    let reason: String
    let confidence: Float
    let onExecute: (() -> Void)?
    let onCancel: (() -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "wrench.and.screwdriver.fill")
                    .foregroundColor(.purple)
                Text(toolName)
                    .font(.headline)
            }

            Text(toolDescription)
                .font(.caption)
                .foregroundColor(.secondary)

            Text(reason)
                .font(.caption2)
                .foregroundColor(.secondary)

            HStack(spacing: 8) {
                Button(L("button.execute")) {
                    onExecute?()
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.small)

                Button(L("button.cancel")) {
                    onCancel?()
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
            }
        }
        .padding(12)
        .frame(maxWidth: 280)
        .background(.ultraThinMaterial)
        .cornerRadius(8)
    }
}

/// Clarification input view
struct HaloClarificationView: View {
    let request: ClarificationRequest
    let onSubmit: (String) -> Void
    let onCancel: (() -> Void)?

    @State private var inputText = ""
    @State private var selectedIndex: Int?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(request.prompt)
                .font(.headline)

            if request.clarificationType == .select, let options = request.options {
                ForEach(Array(options.enumerated()), id: \.offset) { index, option in
                    Button(action: {
                        selectedIndex = index
                        onSubmit(option.value)
                    }) {
                        HStack {
                            Text(option.label)
                            Spacer()
                            if selectedIndex == index {
                                Image(systemName: "checkmark")
                            }
                        }
                    }
                    .buttonStyle(.bordered)
                }
            } else {
                TextField(request.placeholder ?? "", text: $inputText)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit {
                        onSubmit(inputText)
                    }
            }

            Button(L("button.cancel")) {
                onCancel?()
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
        }
        .padding(12)
        .frame(maxWidth: 280)
        .background(.ultraThinMaterial)
        .cornerRadius(8)
    }
}
