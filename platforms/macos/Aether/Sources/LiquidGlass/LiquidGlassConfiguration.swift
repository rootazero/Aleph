//
//  LiquidGlassConfiguration.swift
//  Aether
//
//  Central configuration for Liquid Glass effects.
//

import Foundation
import simd

// MARK: - LiquidGlassConfiguration

struct LiquidGlassConfiguration {

    // MARK: - Animation

    struct Animation {
        /// Aurora flow speed (time scale)
        static let auroraFlowSpeed: Float = 0.1

        /// Aurora noise scale
        static let auroraNoiseScale: Float = 2.0

        /// FBM octaves for aurora
        static let auroraOctaves: Int = 4

        /// Breathing animation period (seconds)
        static let breathPeriod: Float = 4.0

        /// Breathing amplitude (brightness variation)
        static let breathAmplitude: Float = 0.15

        /// Edge glow variation
        static let edgeGlowVariation: Float = 0.10

        /// Hover rise height (visual pixels)
        static let hoverRiseHeight: Float = 4.0

        /// Hover shadow multiplier
        static let hoverShadowMultiplier: Float = 1.5

        /// Hover transition duration (seconds)
        static let hoverTransitionDuration: Double = 0.2

        /// Ripple expansion speed (pixels/second)
        static let rippleSpeed: Float = 200

        /// Ripple fade duration (seconds)
        static let rippleFadeDuration: Float = 0.5

        /// Input focus glow width (pixels)
        static let inputGlowWidth: Float = 3

        /// Input focus pulse period (seconds)
        static let inputPulsePeriod: Float = 2.0

        /// AI thinking rotation speed (radians/second)
        static let thinkingRotationSpeed: Float = 0.3

        /// AI thinking light band count
        static let thinkingLightBands: Int = 3

        /// AI thinking opacity
        static let thinkingOpacity: Float = 0.3
    }

    // MARK: - Fusion

    struct Fusion {
        /// Distance at which fusion begins (pixels)
        static let startDistance: Float = 60

        /// Distance at which fusion is complete (pixels)
        static let completeDistance: Float = 8

        /// Temporal fusion window (seconds)
        static let temporalWindow: Float = 5.0

        /// Same-turn fusion bonus
        static let sameTurnBonus: Float = 0.2

        /// Time factor decay rate
        static let timeDecayRate: Float = 0.0  // Placeholder for non-linear decay
    }

    // MARK: - Glass

    struct Glass {
        /// Overall glass transparency
        static let transparency: Float = 0.85

        /// Refraction strength
        static let refractionStrength: Float = 0.02

        /// Fresnel edge highlight intensity
        static let fresnelIntensity: Float = 0.6

        /// Fresnel power exponent
        static let fresnelPower: Float = 2.0

        /// Top highlight intensity
        static let topHighlightIntensity: Float = 0.15

        /// Inner depth tint (center darker)
        static let depthTintMin: Float = 0.95
        static let depthTintMax: Float = 1.0
    }

    // MARK: - Color

    struct Color {
        /// Accent color blend ratio (vs wallpaper)
        static let accentBlendRatio: Float = 0.4

        /// Wallpaper sample interval (seconds)
        static let sampleInterval: TimeInterval = 5.0

        /// Color transition duration (seconds)
        static let transitionDuration: TimeInterval = 0.8

        /// Low vibrancy threshold (inject accent if below)
        static let lowVibrancyThreshold: Float = 0.2

        /// Vibrancy boost amount
        static let vibrancyBoostAmount: Float = 0.2
    }

    // MARK: - Performance

    struct Performance {
        /// Maximum bubbles to render
        static let maxBubbles: Int = 50

        /// LOD reduction threshold (bubble count)
        static let lodThreshold: Int = 20

        /// Target frame rate
        static let targetFrameRate: Int = 60

        /// Triple buffer count
        static let bufferCount: Int = 3
    }

    // MARK: - Scroll Physics

    struct ScrollPhysics {
        /// Velocity threshold for fusion adjustment
        static let velocityThreshold: Float = 500

        /// Maximum fusion threshold multiplier
        static let maxFusionMultiplier: Float = 1.5

        /// Bubble spacing increase factor
        static let spacingIncreaseFactor: Float = 0.3
    }
}
