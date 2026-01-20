//
//  StreamingTextView.swift
//  Aether
//
//  SwiftUI component for displaying streaming AI responses with typewriter animation
//

import SwiftUI

/// Displays streaming text with typewriter animation
struct StreamingTextView: View {
    let text: String
    let textColor: Color

    @State private var visibleCharacters: Int = 0
    @State private var animationTimer: Timer?

    // Configuration
    private let maxLines: Int = 3
    private let charactersPerSecond: Double = 50.0 // Typewriter speed

    var body: some View {
        Text(displayedText)
            .font(.system(.caption, design: .monospaced))
            .foregroundColor(textColor)
            .lineLimit(maxLines)
            .multilineTextAlignment(.center)
            .padding(.horizontal, 16)
            .frame(maxWidth: 280)
            .fixedSize(horizontal: false, vertical: true)
            .onChange(of: text) { _, newValue in
                // When text changes, extend visible characters to include new content
                if newValue.count > visibleCharacters {
                    animateNewText(from: visibleCharacters, to: newValue.count)
                }
            }
            .onAppear {
                // Animate initial text
                if !text.isEmpty {
                    animateNewText(from: 0, to: text.count)
                }
            }
            .onDisappear {
                // Clean up timer
                animationTimer?.invalidate()
                animationTimer = nil
            }
    }

    /// Displayed text with visible characters limit
    private var displayedText: String {
        if visibleCharacters >= text.count {
            return text
        }

        let endIndex = text.index(text.startIndex, offsetBy: visibleCharacters, limitedBy: text.endIndex) ?? text.endIndex
        return String(text[..<endIndex])
    }

    /// Animate text appearance from start to end index
    private func animateNewText(from start: Int, to end: Int) {
        // Stop existing animation
        animationTimer?.invalidate()

        // Calculate delay between characters based on speed
        let delayPerCharacter = 1.0 / charactersPerSecond

        // If text is short, reveal instantly
        if (end - start) < 5 {
            visibleCharacters = end
            return
        }

        // Start typewriter animation using a class to hold mutable state
        final class AnimationState: @unchecked Sendable {
            var currentIndex: Int
            // Store timer reference for safe invalidation from within closure
            weak var timer: Timer?
            init(_ start: Int) { self.currentIndex = start }
        }
        let state = AnimationState(start)

        // Capture the binding to update visibleCharacters
        let visibleCharactersBinding = _visibleCharacters

        let timer = Timer.scheduledTimer(withTimeInterval: delayPerCharacter, repeats: true) { _ in
            // Timer callbacks run on main thread
            MainActor.assumeIsolated {
                if state.currentIndex < end {
                    state.currentIndex += 1
                    visibleCharactersBinding.wrappedValue = state.currentIndex
                } else {
                    state.timer?.invalidate()
                }
            }
        }
        state.timer = timer
        animationTimer = timer
    }
}

// MARK: - Preview

struct StreamingTextView_Previews: PreviewProvider {
    static var previews: some View {
        Group {
            StreamingTextView(
                text: "Hello, this is a streaming response from AI...",
                textColor: .white
            )
            .frame(width: 300, height: 100)
            .background(Color.black.opacity(0.3))
            .previewDisplayName("Short Text")

            StreamingTextView(
                // swiftlint:disable:next line_length
                text: "This is a much longer streaming response that will demonstrate the line wrapping and truncation behavior when text exceeds the maximum number of lines allowed in the view.",
                textColor: Color.cyan
            )
            .frame(width: 300, height: 100)
            .background(Color.black.opacity(0.3))
            .previewDisplayName("Long Text")

            StreamingTextView(
                text: "function calculateTotal(items) {\n  return items.reduce((sum, item) => sum + item.price, 0);\n}",
                textColor: Color.green
            )
            .frame(width: 300, height: 100)
            .background(Color.black.opacity(0.3))
            .previewDisplayName("Code")
        }
    }
}
