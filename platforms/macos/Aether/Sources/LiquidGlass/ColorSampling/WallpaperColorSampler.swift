//
//  WallpaperColorSampler.swift
//  Aether
//
//  Samples colors from the desktop wallpaper and system accent color.
//  Updates periodically and on system events.
//

import AppKit
import Combine
import simd

// MARK: - WallpaperColorSampler

@MainActor
final class WallpaperColorSampler: ObservableObject {

    // MARK: - Published Properties

    @Published private(set) var accentColor = SIMD4<Float>(0.0, 0.478, 1.0, 1.0)
    @Published private(set) var dominantColors: [SIMD4<Float>] = []

    // MARK: - Private Properties

    private var sampleTimer: Timer?
    private var lastSampleTime: Date = .distantPast
    private var windowObserver: NSObjectProtocol?
    private var wallpaperObserver: NSObjectProtocol?

    private let sampleInterval: TimeInterval = 5.0
    private let transitionDuration: TimeInterval = 0.8

    private var targetColors: [SIMD4<Float>] = []
    private var transitionProgress: Float = 1.0
    private var previousColors: [SIMD4<Float>] = []

    // MARK: - Initialization

    init() {
        setupDefaultColors()
        setupObservers()
        startPeriodicSampling()

        // Initial sample
        Task {
            await sample()
        }
    }

    // MARK: - Cleanup
    // Note: Observer cleanup is handled automatically by ARC.
    // Timer and notification observers are released when this object is deallocated.

    /// Call this method when the sampler is no longer needed
    func stopSampling() {
        sampleTimer?.invalidate()
        sampleTimer = nil
        if let observer = windowObserver {
            NotificationCenter.default.removeObserver(observer)
            windowObserver = nil
        }
        if let observer = wallpaperObserver {
            DistributedNotificationCenter.default().removeObserver(observer)
            wallpaperObserver = nil
        }
    }

    // MARK: - Setup

    private func setupDefaultColors() {
        dominantColors = [
            SIMD4<Float>(0.3, 0.5, 0.7, 1.0),
            SIMD4<Float>(0.5, 0.3, 0.6, 1.0),
            SIMD4<Float>(0.4, 0.6, 0.5, 1.0),
            SIMD4<Float>(0.6, 0.4, 0.5, 1.0),
            SIMD4<Float>(0.5, 0.5, 0.6, 1.0),
        ]
        targetColors = dominantColors
        previousColors = dominantColors
    }

    private func setupObservers() {
        // Window moved notification
        windowObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.didMoveNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor in
                self?.scheduleSample()
            }
        }

        // Wallpaper changed notification
        wallpaperObserver = DistributedNotificationCenter.default().addObserver(
            forName: NSNotification.Name("com.apple.desktop.background.changed"),
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor in
                self?.scheduleSample()
            }
        }
    }

    private func startPeriodicSampling() {
        sampleTimer = Timer.scheduledTimer(withTimeInterval: sampleInterval, repeats: true) { [weak self] _ in
            Task { @MainActor in
                await self?.sample()
            }
        }
    }

    // MARK: - Sampling

    private func scheduleSample() {
        // Debounce rapid events
        let now = Date()
        if now.timeIntervalSince(lastSampleTime) > 0.5 {
            Task {
                await sample()
            }
        }
    }

    func sample() async {
        lastSampleTime = Date()

        // Sample system accent color
        let accent = sampleAccentColor()

        // Sample wallpaper colors
        let wallpaperColors = await sampleWallpaperColors()

        // Mix accent with wallpaper colors
        var finalColors = wallpaperColors
        if !finalColors.isEmpty {
            // Blend accent color into first color slot (40% accent, 60% wallpaper)
            finalColors[0] = mix(accent, finalColors[0], t: 0.6)
        }

        // Start transition
        previousColors = dominantColors
        targetColors = finalColors
        transitionProgress = 0

        // Animate transition
        await animateTransition()

        accentColor = accent
    }

    private func sampleAccentColor() -> SIMD4<Float> {
        let nsColor = NSColor.controlAccentColor
        var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0

        if let rgbColor = nsColor.usingColorSpace(.deviceRGB) {
            rgbColor.getRed(&r, green: &g, blue: &b, alpha: &a)
        }

        return SIMD4<Float>(Float(r), Float(g), Float(b), Float(a))
    }

    private func sampleWallpaperColors() async -> [SIMD4<Float>] {
        // Get wallpaper image URL from workspace (avoids deprecated CGWindowListCreateImage)
        guard let screen = NSScreen.main,
              let wallpaperURL = NSWorkspace.shared.desktopImageURL(for: screen) else {
            return dominantColors
        }

        // Load wallpaper image directly from file
        guard let nsImage = NSImage(contentsOf: wallpaperURL) else {
            return dominantColors
        }

        // Extract dominant colors
        return DominantColorExtractor.extract(from: nsImage, count: 5)
    }

    private func animateTransition() async {
        let steps = 30
        let stepDuration = transitionDuration / Double(steps)

        for i in 1...steps {
            try? await Task.sleep(nanoseconds: UInt64(stepDuration * 1_000_000_000))

            transitionProgress = Float(i) / Float(steps)

            // Interpolate colors
            var interpolated: [SIMD4<Float>] = []
            for j in 0..<min(previousColors.count, targetColors.count) {
                let color = mix(previousColors[j], targetColors[j], t: transitionProgress)
                interpolated.append(color)
            }

            dominantColors = interpolated
        }
    }

    // MARK: - Helpers

    private func mix(_ a: SIMD4<Float>, _ b: SIMD4<Float>, t: Float) -> SIMD4<Float> {
        return a * (1 - t) + b * t
    }

    // MARK: - Manual Trigger

    func forceSample() {
        Task {
            await sample()
        }
    }
}
