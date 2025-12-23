//
//  PerformanceMonitor.swift
//  Aether
//
//  Monitors rendering performance and tracks FPS for Halo overlay.
//  Posts notifications when performance drops below acceptable thresholds.
//

import Foundation
import QuartzCore
import AppKit

/// Notification posted when FPS drops below threshold
extension Notification.Name {
    static let performanceDropDetected = Notification.Name("AetherPerformanceDropDetected")
}

/// Monitors rendering performance using CADisplayLink
class PerformanceMonitor {
    // MARK: - Properties

    /// Singleton instance
    static let shared = PerformanceMonitor()

    /// Display link for tracking frame callbacks
    private var displayLink: CVDisplayLink?

    /// Frame timestamps (last 60 frames)
    private var frameTimestamps: [CFTimeInterval] = []
    private let maxFrameCount = 60

    /// FPS threshold for performance warnings
    private let fpsThreshold: Double = 55.0

    /// Last calculated FPS
    private(set) var currentFPS: Double = 60.0

    /// Whether monitoring is active
    private(set) var isMonitoring: Bool = false

    /// Lock for thread-safe access
    private let lock = NSLock()

    // MARK: - Initialization

    private init() {
        setupDisplayLink()
    }

    deinit {
        stop()
    }

    // MARK: - Display Link Setup

    private func setupDisplayLink() {
        // Create display link
        let displayLinkOutputCallback: CVDisplayLinkOutputCallback = { (
            displayLink: CVDisplayLink,
            inNow: UnsafePointer<CVTimeStamp>,
            inOutputTime: UnsafePointer<CVTimeStamp>,
            flagsIn: CVOptionFlags,
            flagsOut: UnsafeMutablePointer<CVOptionFlags>,
            displayLinkContext: UnsafeMutableRawPointer?
        ) -> CVReturn in
            // Get the monitor instance from context
            let monitor = Unmanaged<PerformanceMonitor>.fromOpaque(displayLinkContext!).takeUnretainedValue()

            // Record frame timestamp
            let timestamp = CACurrentMediaTime()
            monitor.recordFrame(timestamp: timestamp)

            return kCVReturnSuccess
        }

        CVDisplayLinkCreateWithActiveCGDisplays(&displayLink)

        if let displayLink = displayLink {
            // Set output callback
            let context = Unmanaged.passUnretained(self).toOpaque()
            CVDisplayLinkSetOutputCallback(displayLink, displayLinkOutputCallback, context)
        }
    }

    // MARK: - Monitoring Control

    /// Start monitoring performance
    func start() {
        lock.lock()
        defer { lock.unlock() }

        guard !isMonitoring, let displayLink = displayLink else {
            return
        }

        // Clear previous data
        frameTimestamps.removeAll()
        currentFPS = 60.0

        CVDisplayLinkStart(displayLink)
        isMonitoring = true

        print("[PerformanceMonitor] Started monitoring")
    }

    /// Stop monitoring performance
    func stop() {
        lock.lock()
        defer { lock.unlock() }

        guard isMonitoring, let displayLink = displayLink else {
            return
        }

        CVDisplayLinkStop(displayLink)
        isMonitoring = false

        print("[PerformanceMonitor] Stopped monitoring")
    }

    // MARK: - Frame Recording

    private func recordFrame(timestamp: CFTimeInterval) {
        lock.lock()
        defer { lock.unlock() }

        // Add timestamp
        frameTimestamps.append(timestamp)

        // Keep only last 60 frames
        if frameTimestamps.count > maxFrameCount {
            frameTimestamps.removeFirst()
        }

        // Calculate FPS if we have enough samples
        if frameTimestamps.count >= 2 {
            calculateFPS()
        }
    }

    private func calculateFPS() {
        guard frameTimestamps.count >= 2 else {
            return
        }

        // Calculate time span
        let firstTimestamp = frameTimestamps.first!
        let lastTimestamp = frameTimestamps.last!
        let timeSpan = lastTimestamp - firstTimestamp

        // Calculate FPS
        let frameCount = Double(frameTimestamps.count - 1)
        let fps = frameCount / timeSpan

        currentFPS = fps

        // Check for performance drop
        if fps < fpsThreshold {
            notifyPerformanceDrop(fps: fps)
        }
    }

    // MARK: - Notifications

    private var lastNotificationTime: CFTimeInterval = 0
    private let notificationThrottle: CFTimeInterval = 5.0 // Don't spam notifications

    private func notifyPerformanceDrop(fps: Double) {
        let now = CACurrentMediaTime()

        // Throttle notifications (max once per 5 seconds)
        guard now - lastNotificationTime > notificationThrottle else {
            return
        }

        lastNotificationTime = now

        // Post notification on main thread
        DispatchQueue.main.async {
            NotificationCenter.default.post(
                name: .performanceDropDetected,
                object: self,
                userInfo: ["fps": fps]
            )

            print("[PerformanceMonitor] ⚠️ Performance drop detected: \(String(format: "%.1f", fps)) FPS")
        }
    }

    // MARK: - Public API

    /// Get current FPS (thread-safe)
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
