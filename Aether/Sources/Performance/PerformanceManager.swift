//
//  PerformanceManager.swift
//  Aether
//
//  Manages performance settings based on GPU capabilities.
//  Detects GPU on app launch and sets appropriate quality level.
//

import Foundation
import Metal
import AppKit

/// Quality level for visual effects
enum EffectsQuality: String, Codable, CaseIterable {
    case high = "High"
    case medium = "Medium"
    case low = "Low"

    /// User-friendly display name
    var displayName: String {
        return rawValue
    }

    /// Description of quality level
    var description: String {
        switch self {
        case .high:
            return "Full effects with gradients and animations"
        case .medium:
            return "Basic gradients, simplified animations"
        case .low:
            return "Solid colors, linear animations"
        }
    }
}

/// Manages performance settings and GPU detection
class PerformanceManager {
    // MARK: - Singleton

    static let shared = PerformanceManager()

    // MARK: - Properties

    /// UserDefaults keys
    private let qualityKey = "AetherEffectsQuality"
    private let manualOverrideKey = "AetherManualQualityOverride"

    /// Detected GPU name
    private(set) var gpuName: String = "Unknown"

    /// Detected GPU family (Apple Silicon, Intel, AMD, etc.)
    private(set) var gpuFamily: String = "Unknown"

    /// Current effects quality level
    var effectsQuality: EffectsQuality {
        get {
            // Check for manual override first
            if isManualOverride {
                if let savedQuality = UserDefaults.standard.string(forKey: qualityKey),
                   let quality = EffectsQuality(rawValue: savedQuality) {
                    return quality
                }
            }

            // Otherwise return auto-detected quality
            return autoDetectedQuality
        }
        set {
            UserDefaults.standard.set(newValue.rawValue, forKey: qualityKey)
            UserDefaults.standard.set(true, forKey: manualOverrideKey)
            print("[PerformanceManager] Quality manually set to: \(newValue.rawValue)")
        }
    }

    /// Whether quality is manually overridden
    var isManualOverride: Bool {
        UserDefaults.standard.bool(forKey: manualOverrideKey)
    }

    /// Auto-detected quality based on GPU
    private var autoDetectedQuality: EffectsQuality = .medium

    // MARK: - Initialization

    private init() {
        detectGPU()
    }

    // MARK: - GPU Detection

    private func detectGPU() {
        guard let device = MTLCreateSystemDefaultDevice() else {
            print("[PerformanceManager] ⚠️ Failed to create Metal device, defaulting to medium quality")
            autoDetectedQuality = .medium
            return
        }

        gpuName = device.name
        print("[PerformanceManager] Detected GPU: \(gpuName)")

        // Determine GPU family
        if gpuName.contains("Apple") {
            gpuFamily = "Apple Silicon"
            autoDetectedQuality = determineAppleSiliconQuality(device: device)
        } else if gpuName.contains("Intel") {
            gpuFamily = "Intel"
            autoDetectedQuality = determineIntelQuality(gpuName: gpuName)
        } else if gpuName.contains("AMD") || gpuName.contains("Radeon") {
            gpuFamily = "AMD"
            autoDetectedQuality = determineAMDQuality(gpuName: gpuName)
        } else if gpuName.contains("NVIDIA") {
            gpuFamily = "NVIDIA"
            autoDetectedQuality = .high // NVIDIA dGPUs are generally high-end
        } else {
            gpuFamily = "Unknown"
            autoDetectedQuality = .medium // Safe default
        }

        print("[PerformanceManager] GPU Family: \(gpuFamily)")
        print("[PerformanceManager] Auto-detected quality: \(autoDetectedQuality.rawValue)")
    }

    // MARK: - Quality Detection Logic

    private func determineAppleSiliconQuality(device: MTLDevice) -> EffectsQuality {
        // All Apple Silicon Macs (M1, M2, M3, etc.) have excellent GPU performance
        // Check for specific capabilities
        if device.supportsFamily(.apple7) || device.supportsFamily(.apple8) {
            // M2/M3 generation
            return .high
        } else if device.supportsFamily(.apple6) {
            // M1 generation
            return .high
        } else {
            // Older A-series chips (unlikely on Mac)
            return .medium
        }
    }

    private func determineIntelQuality(gpuName: String) -> EffectsQuality {
        let lowerName = gpuName.lowercased()

        // High-end Intel GPUs (Iris Xe, Iris Plus 655+)
        if lowerName.contains("iris xe") ||
           lowerName.contains("iris plus 655") ||
           lowerName.contains("iris plus 645") {
            return .high
        }

        // Mid-range Intel GPUs (Iris Plus, UHD 630+)
        if lowerName.contains("iris plus") ||
           lowerName.contains("uhd 630") ||
           lowerName.contains("uhd 620") {
            return .medium
        }

        // Low-end Intel GPUs (HD 3000, 4000, 5000, 6000)
        if lowerName.contains("hd 3000") ||
           lowerName.contains("hd 4000") ||
           lowerName.contains("hd 5000") ||
           lowerName.contains("hd 6000") {
            return .low
        }

        // Default for unknown Intel GPUs
        return .medium
    }

    private func determineAMDQuality(gpuName: String) -> EffectsQuality {
        let lowerName = gpuName.lowercased()

        // High-end AMD GPUs (Radeon Pro 5000+, Vega)
        if lowerName.contains("vega") ||
           lowerName.contains("pro 5") ||
           lowerName.contains("pro 6") ||
           lowerName.contains("pro 7") {
            return .high
        }

        // Mid-range AMD GPUs (Radeon Pro 400-500 series)
        if lowerName.contains("pro 4") ||
           lowerName.contains("pro 5") {
            return .medium
        }

        // Default for unknown AMD GPUs
        return .high // Most AMD dGPUs in Macs are high-end
    }

    // MARK: - Manual Override

    /// Reset to auto-detected quality
    func resetToAutoDetected() {
        UserDefaults.standard.removeObject(forKey: manualOverrideKey)
        print("[PerformanceManager] Reset to auto-detected quality: \(autoDetectedQuality.rawValue)")
    }

    /// Set quality manually
    func setQuality(_ quality: EffectsQuality) {
        effectsQuality = quality
    }

    // MARK: - Quality Checks

    /// Check if high-quality effects should be used
    func shouldUseHighQuality() -> Bool {
        return effectsQuality == .high
    }

    /// Check if medium-quality effects should be used
    func shouldUseMediumQuality() -> Bool {
        return effectsQuality == .medium
    }

    /// Check if low-quality effects should be used (simplified)
    func shouldUseLowQuality() -> Bool {
        return effectsQuality == .low
    }

    // MARK: - Debug Info

    /// Get debug information about GPU and quality settings
    func getDebugInfo() -> [String: Any] {
        return [
            "gpuName": gpuName,
            "gpuFamily": gpuFamily,
            "autoDetectedQuality": autoDetectedQuality.rawValue,
            "currentQuality": effectsQuality.rawValue,
            "isManualOverride": isManualOverride
        ]
    }

    /// Print debug information
    func printDebugInfo() {
        print("===== Performance Manager Debug Info =====")
        print("GPU Name: \(gpuName)")
        print("GPU Family: \(gpuFamily)")
        print("Auto-detected Quality: \(autoDetectedQuality.rawValue)")
        print("Current Quality: \(effectsQuality.rawValue)")
        print("Manual Override: \(isManualOverride)")
        print("==========================================")
    }
}
