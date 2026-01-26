//
//  BubbleFusionCalculator.swift
//  Aether
//
//  Calculates fusion weights for bubble merging based on distance, time, and interaction.
//

import Foundation
import simd

// MARK: - BubbleFusionCalculator

struct BubbleFusionCalculator {

    // MARK: - Configuration

    struct Config {
        /// Distance at which fusion begins
        var fusionStartDistance: Float = 60

        /// Distance at which fusion is complete
        var fusionCompleteDistance: Float = 8

        /// Time window for temporal fusion (seconds)
        var temporalFusionWindow: Float = 5.0

        /// Same-turn fusion bonus
        var sameTurnBonus: Float = 0.2

        /// Hover isolation factor (reduces fusion when hovering)
        var hoverIsolationFactor: Float = 0.5

        /// Scroll velocity impact on fusion threshold
        var scrollFusionMultiplier: Float = 0.5
    }

    static let defaultConfig = Config()

    // MARK: - Calculation

    /// Calculate fusion weights for all bubbles
    /// - Parameters:
    ///   - bubbles: Array of bubble data with positions and timestamps
    ///   - hoveredIndex: Index of currently hovered bubble (-1 if none)
    ///   - scrollVelocity: Current scroll velocity
    ///   - currentTime: Current timestamp
    ///   - config: Configuration parameters
    /// - Returns: Array of fusion weights (0 = isolated, 1 = fully fused)
    static func calculateFusionWeights(
        bubbles: [BubbleInfo],
        hoveredIndex: Int,
        scrollVelocity: Float,
        currentTime: Float,
        config: Config = defaultConfig
    ) -> [Float] {
        guard !bubbles.isEmpty else { return [] }

        var weights = [Float](repeating: 1.0, count: bubbles.count)

        // Adjust fusion threshold based on scroll velocity
        let velocityFactor = 1.0 + min(abs(scrollVelocity) / 500, 1.0) * config.scrollFusionMultiplier
        let adjustedStartDistance = config.fusionStartDistance * velocityFactor

        for i in 0..<bubbles.count {
            var fusionWeight: Float = 1.0

            // Check distance to adjacent bubbles
            if i > 0 {
                let distanceAbove = calculateDistance(bubbles[i], bubbles[i - 1])
                let distanceFusion = distanceFusionFactor(
                    distance: distanceAbove,
                    startDistance: adjustedStartDistance,
                    completeDistance: config.fusionCompleteDistance
                )

                // Time-based fusion
                let timeDelta = abs(bubbles[i].timestamp - bubbles[i - 1].timestamp)
                let timeFusion = timeFusionFactor(timeDelta: timeDelta, window: config.temporalFusionWindow)

                // Same turn bonus
                let sameRole = bubbles[i].isUser == bubbles[i - 1].isUser
                let turnBonus: Float = sameRole ? 0 : config.sameTurnBonus

                // Combine factors
                fusionWeight = min(fusionWeight, distanceFusion + timeFusion * 0.3 + turnBonus)
            }

            if i < bubbles.count - 1 {
                let distanceBelow = calculateDistance(bubbles[i], bubbles[i + 1])
                let distanceFusion = distanceFusionFactor(
                    distance: distanceBelow,
                    startDistance: adjustedStartDistance,
                    completeDistance: config.fusionCompleteDistance
                )

                fusionWeight = min(fusionWeight, distanceFusion)
            }

            // Hover isolation
            if i == hoveredIndex {
                fusionWeight *= config.hoverIsolationFactor
            }

            weights[i] = fusionWeight
        }

        return weights
    }

    // MARK: - Helper Functions

    private static func calculateDistance(_ a: BubbleInfo, _ b: BubbleInfo) -> Float {
        // Calculate vertical gap between bubbles (bottom of one to top of other)
        let aBottom = a.center.y - a.size.y / 2
        let bTop = b.center.y + b.size.y / 2
        let verticalGap = abs(aBottom - bTop)

        // Horizontal overlap consideration
        let horizontalOverlap = min(a.center.x + a.size.x / 2, b.center.x + b.size.x / 2) -
                               max(a.center.x - a.size.x / 2, b.center.x - b.size.x / 2)

        if horizontalOverlap > 0 {
            return verticalGap
        } else {
            // No horizontal overlap, use euclidean distance
            let dx = a.center.x - b.center.x
            let dy = a.center.y - b.center.y
            return sqrt(dx * dx + dy * dy)
        }
    }

    private static func distanceFusionFactor(distance: Float, startDistance: Float, completeDistance: Float) -> Float {
        if distance <= completeDistance {
            return 1.0
        } else if distance >= startDistance {
            return 0.0
        } else {
            // Smooth interpolation
            let t = (distance - completeDistance) / (startDistance - completeDistance)
            return 1.0 - smoothstep(t)
        }
    }

    private static func timeFusionFactor(timeDelta: Float, window: Float) -> Float {
        if timeDelta >= window {
            return 0.0
        }
        let t = timeDelta / window
        return 1.0 - smoothstep(t)
    }

    private static func smoothstep(_ t: Float) -> Float {
        let x = max(0, min(1, t))
        return x * x * (3 - 2 * x)
    }
}

// MARK: - BubbleInfo

struct BubbleInfo {
    var center: SIMD2<Float>
    var size: SIMD2<Float>
    var timestamp: Float
    var isUser: Bool
}
