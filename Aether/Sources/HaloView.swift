//
//  HaloView.swift
//  Aether
//
//  SwiftUI view for Halo animations with state machine and theme support.
//

import SwiftUI

struct HaloView: View {
    @State var state: HaloState = .idle
    @ObservedObject var themeEngine: ThemeEngine
    var eventHandler: EventHandler?

    private var theme: any HaloTheme {
        themeEngine.activeTheme
    }

    var body: some View {
        ZStack {
            switch state {
            case .idle:
                EmptyView()

            case .listening:
                theme.listeningView()
                    .transition(.scale.combined(with: .opacity))

            case .processing(let providerColor, let streamingText):
                theme.processingView(providerColor: providerColor, streamingText: streamingText)
                    .transition(.scale.combined(with: .opacity))

            case .success(let finalText):
                theme.successView(finalText: finalText)
                    .transition(.scale.combined(with: .opacity))

            case .error(let type, let message):
                theme.errorView(
                    type: type,
                    message: message,
                    onRetry: eventHandler?.handleRetry,
                    onOpenSettings: eventHandler?.handleOpenSettings,
                    onDismiss: eventHandler?.handleDismiss
                )
                .transition(.scale.combined(with: .opacity))
            }
        }
        .frame(width: dynamicWidth, height: dynamicHeight)
        .animation(.spring(response: 0.4), value: state)
        .animation(.easeInOut(duration: 0.5), value: themeEngine.currentTheme)
    }

    // Dynamic sizing based on state
    private var dynamicWidth: CGFloat {
        switch state {
        case .processing(_, let text), .success(let text):
            return text != nil ? 300 : 120
        case .error:
            return 300
        default:
            return 120
        }
    }

    private var dynamicHeight: CGFloat {
        switch state {
        case .processing(_, let text):
            return text != nil ? 200 : 120
        case .success(let text):
            return text != nil ? 150 : 120
        case .error:
            return 180
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
        Group {
            HaloView(state: .listening)
                .previewDisplayName("Listening")
                .frame(width: 120, height: 120)
                .background(Color.black.opacity(0.3))

            HaloView(state: .processing(providerColor: .green))
                .previewDisplayName("Processing")
                .frame(width: 120, height: 120)
                .background(Color.black.opacity(0.3))

            HaloView(state: .success)
                .previewDisplayName("Success")
                .frame(width: 120, height: 120)
                .background(Color.black.opacity(0.3))

            HaloView(state: .error)
                .previewDisplayName("Error")
                .frame(width: 120, height: 120)
                .background(Color.black.opacity(0.3))
        }
    }
}
