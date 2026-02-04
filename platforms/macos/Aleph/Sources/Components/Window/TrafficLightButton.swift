//
//  TrafficLightButton.swift
//  Aleph
//
//  Custom traffic light button (red/yellow/green) for macOS 26 window design.
//  Mimics native macOS traffic lights with hover state and gradient fill.
//

import SwiftUI

/// Custom traffic light button component
///
/// Renders a circular button with gradient fill that matches native macOS traffic lights.
/// On hover, displays an icon indicating the button's action (close, minimize, or fullscreen).
struct TrafficLightButton: View {
    // MARK: - Properties

    /// Button color (red, yellow, or green)
    let color: Color

    /// Action to execute when button is clicked
    let action: () -> Void

    // MARK: - State

    /// Tracks whether the mouse is hovering over the button
    @State private var isHovering = false

    // MARK: - Body

    var body: some View {
        Button(action: action) {
            ZStack {
                // Circular background with gradient fill
                Circle()
                    .fill(color.gradient)

                // Symbol icon (visible only on hover)
                if isHovering {
                    Image(systemName: symbolName)
                        .font(.system(size: 7, weight: .bold))
                        .foregroundStyle(.black.opacity(0.7))
                }
            }
            .frame(width: 13, height: 13)
        }
        .buttonStyle(.plain)  // Remove default button styling
        .onHover { hovering in
            isHovering = hovering
        }
    }

    // MARK: - Helper Methods

    /// Returns the appropriate SF Symbol name based on button color
    private var symbolName: String {
        switch color {
        case .red:
            return "xmark"  // Close symbol
        case .yellow:
            return "minus"  // Minimize symbol
        case .green:
            return "arrow.up.left.and.arrow.down.right"  // Fullscreen toggle symbol
        default:
            return ""
        }
    }
}

// MARK: - Preview

#Preview("Traffic Light Buttons") {
    VStack(spacing: 20) {
        // All three buttons in a row
        HStack(spacing: 8) {
            TrafficLightButton(color: .red) {
                print("Close clicked")
            }

            TrafficLightButton(color: .yellow) {
                print("Minimize clicked")
            }

            TrafficLightButton(color: .green) {
                print("Fullscreen clicked")
            }
        }
        .padding()
        .background(Color.gray.opacity(0.2))
        .cornerRadius(8)

        Text("Hover over buttons to see icons")
            .font(.caption)
            .foregroundColor(.secondary)
    }
    .frame(width: 200, height: 150)
}

#Preview("Individual Buttons - Light Mode") {
    VStack(spacing: 16) {
        TrafficLightButton(color: .red, action: {})
        TrafficLightButton(color: .yellow, action: {})
        TrafficLightButton(color: .green, action: {})
    }
    .padding()
}

#Preview("Individual Buttons - Dark Mode") {
    VStack(spacing: 16) {
        TrafficLightButton(color: .red, action: {})
        TrafficLightButton(color: .yellow, action: {})
        TrafficLightButton(color: .green, action: {})
    }
    .padding()
    .preferredColorScheme(.dark)
}
