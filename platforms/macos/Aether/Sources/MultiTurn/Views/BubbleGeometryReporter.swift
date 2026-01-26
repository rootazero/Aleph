//
//  BubbleGeometryReporter.swift
//  Aether
//
//  Reports bubble geometry from SwiftUI to Metal layer.
//  Uses preference keys to collect frame data.
//

import SwiftUI
import Combine
import simd

// MARK: - BubbleGeometryPreferenceKey

struct BubbleGeometryPreferenceKey: PreferenceKey {
    nonisolated(unsafe) static var defaultValue: [BubbleGeometry] = []

    static func reduce(value: inout [BubbleGeometry], nextValue: () -> [BubbleGeometry]) {
        value.append(contentsOf: nextValue())
    }
}

// MARK: - BubbleGeometry

struct BubbleGeometry: Equatable, Sendable {
    let id: String
    let frame: CGRect
    let isUser: Bool
    let timestamp: TimeInterval
    let index: Int
}

// MARK: - BubbleGeometryReporter Modifier

struct BubbleGeometryReporter: ViewModifier {
    let id: String
    let isUser: Bool
    let timestamp: TimeInterval
    let index: Int
    let coordinateSpace: CoordinateSpace

    func body(content: Content) -> some View {
        content
            .background(
                GeometryReader { geometry in
                    Color.clear
                        .preference(
                            key: BubbleGeometryPreferenceKey.self,
                            value: [
                                BubbleGeometry(
                                    id: id,
                                    frame: geometry.frame(in: coordinateSpace),
                                    isUser: isUser,
                                    timestamp: timestamp,
                                    index: index
                                )
                            ]
                        )
                }
            )
    }
}

// MARK: - View Extension

extension View {
    func reportBubbleGeometry(
        id: String,
        isUser: Bool,
        timestamp: TimeInterval,
        index: Int,
        coordinateSpace: CoordinateSpace = .named("liquidGlass")
    ) -> some View {
        modifier(BubbleGeometryReporter(
            id: id,
            isUser: isUser,
            timestamp: timestamp,
            index: index,
            coordinateSpace: coordinateSpace
        ))
    }
}

// MARK: - Geometry to BubbleData Converter

extension BubbleGeometry {
    func toBubbleData(in viewportSize: CGSize, startTime: TimeInterval) -> BubbleData {
        // Convert SwiftUI coordinates to Metal coordinates
        // SwiftUI: origin at top-left, Y increases downward
        // Metal texture: origin at top-left, same convention

        let center = SIMD2<Float>(
            Float(frame.midX),
            Float(frame.midY)
        )

        let size = SIMD2<Float>(
            Float(frame.width),
            Float(frame.height)
        )

        return BubbleData(
            center: center,
            size: size,
            cornerRadius: 12,
            fusionWeight: 1.0,
            timestamp: Float(timestamp - startTime),
            isUser: isUser,
            isHovered: false,
            isPressed: false
        )
    }

    func toBubbleInfo(startTime: TimeInterval) -> BubbleInfo {
        return BubbleInfo(
            center: SIMD2<Float>(Float(frame.midX), Float(frame.midY)),
            size: SIMD2<Float>(Float(frame.width), Float(frame.height)),
            timestamp: Float(timestamp - startTime),
            isUser: isUser
        )
    }
}

// MARK: - BubbleDataCollector

@MainActor
class BubbleDataCollector: ObservableObject {
    @Published var bubbles: [BubbleData] = []
    @Published var hoveredIndex: Int = -1

    private var geometries: [BubbleGeometry] = []
    private let startTime: TimeInterval = Date().timeIntervalSince1970

    // Debounce mechanism to prevent multiple updates per frame
    private var updateTask: Task<Void, Never>?
    private var pendingGeometries: [BubbleGeometry]?
    private var pendingViewportSize: CGSize?

    func updateGeometries(_ newGeometries: [BubbleGeometry], viewportSize: CGSize) {
        // Store pending update
        pendingGeometries = newGeometries
        pendingViewportSize = viewportSize

        // Cancel previous pending update
        updateTask?.cancel()

        // Debounce: wait one frame before applying
        updateTask = Task { @MainActor in
            // Wait for next frame
            try? await Task.sleep(nanoseconds: 16_000_000)  // ~1 frame at 60fps

            // Check if cancelled
            guard !Task.isCancelled,
                  let geometries = pendingGeometries,
                  let viewport = pendingViewportSize else {
                return
            }

            // Apply update
            self.geometries = geometries.sorted { $0.index < $1.index }
            recalculateBubbles(viewportSize: viewport)

            // Clear pending
            pendingGeometries = nil
            pendingViewportSize = nil
        }
    }

    func setHoveredBubble(id: String?) {
        if let id = id {
            hoveredIndex = geometries.firstIndex { $0.id == id } ?? -1
        } else {
            hoveredIndex = -1
        }
    }

    private func recalculateBubbles(viewportSize: CGSize, scrollVelocity: Float = 0) {
        // Convert geometries to BubbleInfo for fusion calculation
        let bubbleInfos = geometries.map { $0.toBubbleInfo(startTime: startTime) }

        // Calculate fusion weights
        let weights = BubbleFusionCalculator.calculateFusionWeights(
            bubbles: bubbleInfos,
            hoveredIndex: hoveredIndex,
            scrollVelocity: scrollVelocity,
            currentTime: Float(Date().timeIntervalSince1970 - startTime)
        )

        // Convert to BubbleData with weights
        var newBubbles: [BubbleData] = []
        for (index, geometry) in geometries.enumerated() {
            var bubbleData = geometry.toBubbleData(in: viewportSize, startTime: startTime)
            if index < weights.count {
                bubbleData.fusionWeight = weights[index]
            }
            bubbleData.isHovered = index == hoveredIndex
            newBubbles.append(bubbleData)
        }

        bubbles = newBubbles
    }
}
