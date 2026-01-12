//
//  HaloStateCoordinator.swift
//  Aether
//
//  Protocol and coordinator for decoupling EventHandler from HaloWindow.
//  This breaks the bidirectional coupling by using a protocol-based abstraction.
//

import Foundation
import AppKit

// MARK: - HaloStateCoordinator Protocol

/// Protocol for coordinating Halo window state updates.
///
/// This protocol decouples the EventHandler from direct HaloWindow manipulation,
/// enabling:
/// - Independent testing of EventHandler
/// - Flexible window management strategies
/// - Multiple observers for state changes
protocol HaloStateCoordinatorProtocol: AnyObject {
    /// Update the Halo window to a new state
    /// - Parameter state: The new HaloState to display
    func updateState(_ state: HaloState)

    /// Show the Halo window centered on screen
    func showCentered()

    /// Show the Halo window at the current mouse position
    func showAtCurrentPosition()

    /// Show the Halo window at a specific position
    /// - Parameter position: The screen position to show at
    func show(at position: NSPoint)

    /// Show the Halo window below the specified position
    /// - Parameter position: The position to show below
    func showBelow(at position: NSPoint)

    /// Hide the Halo window
    func hide()

    /// Force hide the Halo window immediately
    func forceHide()

    /// Get the time the Halo window was last shown
    var showTime: Date? { get }

    /// Update typewriter progress
    /// - Parameter percent: Progress percentage (0-100)
    func updateTypewriterProgress(_ percent: Float)
}

// MARK: - Default HaloStateCoordinator Implementation

/// Default implementation of HaloStateCoordinator that delegates to HaloWindow
final class DefaultHaloStateCoordinator: HaloStateCoordinatorProtocol {
    /// Weak reference to HaloWindow to avoid retain cycles
    private weak var haloWindow: HaloWindow?

    /// Initialize with a HaloWindow
    /// - Parameter haloWindow: The Halo window to coordinate
    init(haloWindow: HaloWindow?) {
        self.haloWindow = haloWindow
    }

    /// Update the Halo window reference
    /// - Parameter haloWindow: The new Halo window
    func setHaloWindow(_ haloWindow: HaloWindow?) {
        self.haloWindow = haloWindow
    }

    // MARK: - HaloStateCoordinatorProtocol Implementation

    func updateState(_ state: HaloState) {
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindow?.updateState(state)
        }
    }

    func showCentered() {
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindow?.showCentered()
        }
    }

    func showAtCurrentPosition() {
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindow?.showAtCurrentPosition()
        }
    }

    func show(at position: NSPoint) {
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindow?.show(at: position)
        }
    }

    func showBelow(at position: NSPoint) {
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindow?.showBelow(at: position)
        }
    }

    func hide() {
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindow?.hide()
        }
    }

    func forceHide() {
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindow?.forceHide()
        }
    }

    var showTime: Date? {
        return haloWindow?.showTime
    }

    func updateTypewriterProgress(_ percent: Float) {
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindow?.updateTypewriterProgress(percent)
        }
    }
}

// MARK: - MockHaloStateCoordinator for Testing

/// Mock implementation for testing EventHandler without a real window
final class MockHaloStateCoordinator: HaloStateCoordinatorProtocol {
    /// Recorded state changes for verification
    private(set) var stateHistory: [HaloState] = []

    /// Whether the window is currently visible
    private(set) var isVisible: Bool = false

    /// Current position of the window
    private(set) var currentPosition: NSPoint?

    /// Simulated show time
    private(set) var _showTime: Date?

    /// Last typewriter progress value
    private(set) var lastProgress: Float?

    func updateState(_ state: HaloState) {
        stateHistory.append(state)
    }

    func showCentered() {
        isVisible = true
        currentPosition = nil
        _showTime = Date()
    }

    func showAtCurrentPosition() {
        isVisible = true
        _showTime = Date()
    }

    func show(at position: NSPoint) {
        isVisible = true
        currentPosition = position
        _showTime = Date()
    }

    func showBelow(at position: NSPoint) {
        isVisible = true
        currentPosition = NSPoint(x: position.x, y: position.y - 50)
        _showTime = Date()
    }

    func hide() {
        isVisible = false
    }

    func forceHide() {
        isVisible = false
    }

    var showTime: Date? {
        return _showTime
    }

    func updateTypewriterProgress(_ percent: Float) {
        lastProgress = percent
    }

    /// Reset all recorded state for fresh test
    func reset() {
        stateHistory.removeAll()
        isVisible = false
        currentPosition = nil
        _showTime = nil
        lastProgress = nil
    }
}
