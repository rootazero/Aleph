//
//  HaloView.swift
//  Aether
//
//  SwiftUI view for Halo animations with state machine and theme support.
//

import SwiftUI

struct HaloView: View {
    @ObservedObject var viewModel: HaloViewModel
    @ObservedObject var themeEngine: ThemeEngine

    private var theme: any HaloTheme {
        themeEngine.activeTheme
    }

    private var state: HaloState {
        viewModel.state
    }

    private var eventHandler: EventHandler? {
        viewModel.eventHandler
    }

    var body: some View {
        ZStack {
            switch viewModel.state {
            case .idle:
                EmptyView()

            case .listening:
                theme.listeningView()
                    .transition(.scale.combined(with: .opacity))

            case .commandMode:
                CommandListView(
                    manager: viewModel.commandManager,
                    maxHeight: 320
                )
                .frame(width: 400)
                .transition(.scale.combined(with: .opacity))

            case .retrievingMemory:
                theme.retrievingMemoryView()
                    .transition(.scale.combined(with: .opacity))

            case .processingWithAI(let providerColor, let providerName):
                theme.processingWithAIView(providerColor: providerColor, providerName: providerName)
                    .transition(.scale.combined(with: .opacity))

            case .processing(let providerColor, let streamingText):
                theme.processingView(providerColor: providerColor, streamingText: streamingText)
                    .transition(.scale.combined(with: .opacity))

            case .typewriting(let progress):
                theme.typewritingView(progress: progress)
                    .transition(.scale.combined(with: .opacity))

            case .success(let finalText):
                theme.successView(finalText: finalText)
                    .transition(.scale.combined(with: .opacity))

            case .error(let type, let message, let suggestion):
                theme.errorView(
                    type: type,
                    message: message,
                    suggestion: suggestion,
                    onRetry: eventHandler?.handleRetry,
                    onOpenSettings: eventHandler?.handleOpenSettings,
                    onDismiss: eventHandler?.handleDismiss
                )
                .transition(.scale.combined(with: .opacity))

            case .permissionRequired(let permissionType):
                PermissionPromptView(
                    permissionType: permissionType,
                    onOpenSettings: {
                        // Open System Settings to the appropriate permission pane
                        if let url = URL(string: permissionType.systemSettingsURL) {
                            NSWorkspace.shared.open(url)
                        }
                    },
                    onDismiss: {
                        // Dismiss permission prompt
                        eventHandler?.handleDismiss()
                    }
                )
                .transition(.scale.combined(with: .opacity))

            case .toast(let type, let title, let message, _, let onDismiss):
                theme.toastView(
                    type: type,
                    title: title,
                    message: message,
                    onDismiss: onDismiss
                )
                .transition(.scale.combined(with: .opacity))
            }
        }
        .frame(width: dynamicWidth, height: dynamicHeight)
        .animation(.spring(response: 0.4), value: state)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(accessibilityLabelForState)
        .accessibilityValue(accessibilityValueForState ?? "")
        .accessibilityAddTraits(accessibilityTraitsForState)
    }

    // MARK: - Accessibility Support

    /// Accessibility label describing current state
    private var accessibilityLabelForState: String {
        switch state {
        case .idle:
            return "Aether is idle"
        case .listening:
            return "Listening for input"
        case .commandMode:
            return "Command completion mode"
        case .retrievingMemory:
            return "Retrieving memories"
        case .processingWithAI(_, let providerName):
            if let name = providerName {
                return "Processing with \(name)"
            }
            return "Processing with AI"
        case .processing:
            return "Processing request"
        case .typewriting(_):
            return "Typewriter animation in progress"
        case .success:
            return "Request completed successfully"
        case .error(let type, _, _):
            let errorTypeString: String
            switch type {
            case .network:
                errorTypeString = "Network"
            case .permission:
                errorTypeString = "Permission"
            case .quota:
                errorTypeString = "Quota"
            case .timeout:
                errorTypeString = "Timeout"
            case .unknown:
                errorTypeString = "Unknown"
            }
            return "\(errorTypeString) error occurred"
        case .permissionRequired(let permissionType):
            return permissionType.title
        case .toast(let type, let title, _, _, _):
            return "\(type.displayName): \(title)"
        }
    }

    /// Accessibility value for dynamic states
    private var accessibilityValueForState: String? {
        switch state {
        case .typewriting(let progress):
            return "\(Int(progress * 100)) percent complete"
        case .processing(_, let text):
            return text
        case .success(let text):
            return text
        case .toast(_, _, let message, _, _):
            return message
        default:
            return nil
        }
    }

    /// Accessibility traits for state
    private var accessibilityTraitsForState: AccessibilityTraits {
        switch state {
        case .typewriting:
            return [.updatesFrequently]
        case .processing, .retrievingMemory, .processingWithAI:
            return [.updatesFrequently]
        case .error, .permissionRequired, .toast:
            return [.isStaticText]
        default:
            return []
        }
    }

    // Dynamic sizing based on state
    private var dynamicWidth: CGFloat {
        switch state {
        case .commandMode:
            return 400  // Width for command list (wider for hints)
        case .retrievingMemory, .processingWithAI:
            return 120
        case .processing(_, let text):
            return text != nil ? 300 : 120
        case .success:
            return 120
        case .typewriting:
            return 120
        case .error:
            return 300
        case .permissionRequired:
            return 480  // Wider for permission prompt
        case .toast:
            return 400  // Max width for toast
        default:
            return 120
        }
    }

    private var dynamicHeight: CGFloat {
        switch state {
        case .commandMode:
            // Fixed height for command mode to prevent window jumping during filtering
            // Height fits 8 commands (max visible) + header
            return 320
        case .retrievingMemory, .processingWithAI:
            return 120
        case .processing(_, let text):
            return text != nil ? 200 : 120
        case .typewriting:
            return 120
        case .success:
            return 120
        case .error:
            return 180
        case .permissionRequired:
            return 450  // Taller for permission prompt
        case .toast(_, _, let message, _, _):
            // Dynamic height based on message length
            let lineCount = min(5, max(1, message.count / 50 + 1))
            return CGFloat(80 + lineCount * 16)
        default:
            return 120
        }
    }
}

// MARK: - Listening State: Pulsing Ring

struct PulsingRingView: View {
    @State private var isPulsing = false

    var body: some View {
        Circle()
            .stroke(lineWidth: 4)
            .foregroundColor(.blue)
            .frame(width: 60, height: 60)
            .scaleEffect(isPulsing ? 1.2 : 1.0)
            .opacity(isPulsing ? 0.5 : 1.0)
            .onAppear {
                withAnimation(.easeInOut(duration: 0.8).repeatForever(autoreverses: true)) {
                    isPulsing = true
                }
            }
    }
}

// MARK: - Processing State: Spinner

struct SpinnerView: View {
    let color: Color
    @State private var rotation: Double = 0

    var body: some View {
        Circle()
            .trim(from: 0, to: 0.7)
            .stroke(color, style: StrokeStyle(lineWidth: 4, lineCap: .round))
            .frame(width: 60, height: 60)
            .rotationEffect(.degrees(rotation))
            .onAppear {
                withAnimation(.linear(duration: 1.0).repeatForever(autoreverses: false)) {
                    rotation = 360
                }
            }
    }
}

// MARK: - Success State: Checkmark

struct CheckmarkView: View {
    @State private var showCheckmark = false

    var body: some View {
        ZStack {
            Circle()
                .fill(Color.green.opacity(0.2))
                .frame(width: 80, height: 80)

            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 50))
                .foregroundColor(.green)
                .scaleEffect(showCheckmark ? 1.0 : 0.5)
                .opacity(showCheckmark ? 1.0 : 0.0)
        }
        .onAppear {
            withAnimation(.spring(response: 0.4, dampingFraction: 0.6)) {
                showCheckmark = true
            }
        }
    }
}

// MARK: - Error State: X Icon with Shake

struct ErrorView: View {
    @State private var showError = false
    @State private var shake = false

    var body: some View {
        ZStack {
            Circle()
                .fill(Color.red.opacity(0.2))
                .frame(width: 80, height: 80)

            Image(systemName: "xmark.circle.fill")
                .font(.system(size: 50))
                .foregroundColor(.red)
                .scaleEffect(showError ? 1.0 : 0.5)
                .opacity(showError ? 1.0 : 0.0)
                .offset(x: shake ? -8 : 8)
        }
        .onAppear {
            withAnimation(.spring(response: 0.4, dampingFraction: 0.6)) {
                showError = true
            }
            withAnimation(.easeInOut(duration: 0.1).repeatCount(3, autoreverses: true)) {
                shake.toggle()
            }
        }
    }
}

// MARK: - Preview

struct HaloView_Previews: PreviewProvider {
    static var previews: some View {
        let themeEngine = ThemeEngine()

        Group {
            HaloView(viewModel: {
                let vm = HaloViewModel()
                vm.state = .listening
                return vm
            }(), themeEngine: themeEngine)
                .previewDisplayName("Listening")
                .frame(width: 120, height: 120)
                .background(Color.black.opacity(0.3))

            HaloView(viewModel: {
                let vm = HaloViewModel()
                vm.state = .processing(providerColor: .green, streamingText: nil)
                return vm
            }(), themeEngine: themeEngine)
                .previewDisplayName("Processing")
                .frame(width: 120, height: 120)
                .background(Color.black.opacity(0.3))

            HaloView(viewModel: {
                let vm = HaloViewModel()
                vm.state = .success(finalText: nil)
                return vm
            }(), themeEngine: themeEngine)
                .previewDisplayName("Success")
                .frame(width: 120, height: 120)
                .background(Color.black.opacity(0.3))

            HaloView(viewModel: {
                let vm = HaloViewModel()
                vm.state = .error(type: .unknown, message: "Test error", suggestion: nil)
                return vm
            }(), themeEngine: themeEngine)
                .previewDisplayName("Error")
                .frame(width: 120, height: 120)
                .background(Color.black.opacity(0.3))
        }
    }
}
