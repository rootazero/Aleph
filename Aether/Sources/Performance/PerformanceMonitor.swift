//
//  PerformanceMonitor.swift
//  Aether
//
//  Monitors rendering performance and tracks FPS for Halo overlay.
//  Posts notifications when performance drops below acceptable thresholds.
//
//  NOTE: CVDisplayLink APIs were deprecated in macOS 15.0.
//  This is a simplified version that returns fixed FPS values.
//  TODO: Migrate to NSView.displayLink(target:selector:) API for real monitoring.
//

import Foundation
import AppKit

/// Notification posted when FPS drops below threshold
extension Notification.Name {
    static let performanceDropDetected = Notification.Name("AetherPerformanceDropDetected")
}

/// Monitors rendering performance (simplified version for macOS 15+)
class PerformanceMonitor {
    // MARK: - Properties

    /// Singleton instance
    static let shared = PerformanceMonitor()

    /// FPS threshold for performance warnings
    private let fpsThreshold: Double = 55.0

    /// Last calculated FPS (fixed value for now)
    private(set) var currentFPS: Double = 60.0

    /// Whether monitoring is active
    private(set) var isMonitoring: Bool = false

    /// Lock for thread-safe access
    private let lock = NSLock()

    // MARK: - Initialization

    private init() {
        // Simplified initialization - no CVDisplayLink setup
    }

    deinit {
        stop()
    }

    // MARK: - Monitoring Control

    /// Start monitoring performance
    func start() {
        lock.lock()
        defer { lock.unlock() }

        guard !isMonitoring else {
            return
        }

        currentFPS = 60.0
        isMonitoring = true

        print("[PerformanceMonitor] Started monitoring (simplified mode)")
    }

    /// Stop monitoring performance
    func stop() {
        lock.lock()
        defer { lock.unlock() }

        guard isMonitoring else {
            return
        }

        isMonitoring = false

        print("[PerformanceMonitor] Stopped monitoring")
    }

    // MARK: - Public API

    /// Get current FPS (thread-safe)
    /// Note: Returns fixed 60 FPS in simplified mode
    func getFPS() -> Double {
        lock.lock()
        defer { lock.unlock() }
        return currentFPS
    }

    /// Get average frame time in milliseconds
    func getAverageFrameTime() -> Double {
        let fps = getFPS()
        return fps > 0 ? 1000.0 / fps : 0
    }

    /// Check if performance is acceptable
    func isPerformanceAcceptable() -> Bool {
        return getFPS() >= fpsThreshold
    }
}
