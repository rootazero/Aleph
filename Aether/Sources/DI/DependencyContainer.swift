//
//  DependencyContainer.swift
//  Aether
//
//  Dependency Injection container for managing shared services lifecycle.
//  This replaces global singletons with explicit dependency injection.
//
//  Usage:
//  1. Initialize core services after permission validation
//  2. Access services through container.shared or direct injection
//  3. Use protocols for testability
//

import Foundation
import SwiftUI
import Combine

// MARK: - Dependency Container

/// Central container for managing all shared services and their lifecycles
///
/// This container replaces scattered global singletons (e.g., `ClipboardManager.shared`)
/// with a unified dependency injection pattern, improving testability and
/// making dependencies explicit.
///
/// Initialization order:
/// 1. Container created as singleton
/// 2. `initializeCoreServices()` called after permissions granted
/// 3. `initializeCoordinators()` called after core is ready
/// 4. Components access dependencies through container
final class DependencyContainer: ObservableObject {

    // MARK: - Singleton

    /// Shared container instance
    /// This is the only singleton in the app - all other dependencies flow through it
    static let shared = DependencyContainer()

    // MARK: - Core Services (initialized after permissions)

    /// Rust core instance
    private(set) var core: AetherCore?

    /// Event handler for Rust callbacks
    private(set) var eventHandler: EventHandler?

    /// Theme management engine
    private(set) var themeEngine: ThemeEngine?

    // MARK: - Managers (lazy-loaded, protocol-based for testability)

    /// Clipboard operations manager
    private(set) lazy var clipboardManager: any ClipboardManagerProtocol = ClipboardManager.shared

    /// Clipboard change monitor
    private(set) lazy var clipboardMonitor: any ClipboardMonitorProtocol = ClipboardMonitor.shared

    /// Clarification flow manager
    private(set) lazy var clarificationManager: any ClarificationManagerProtocol = ClarificationManager.shared

    /// Multi-turn conversation manager
    private(set) lazy var conversationManager: any ConversationManagerProtocol = ConversationManager.shared

    /// Launch at login manager (accessed on main actor)
    @MainActor
    var launchAtLoginManager: LaunchAtLoginManager {
        LaunchAtLoginManager.shared
    }

    // MARK: - Coordinators (initialized after core services)

    /// Halo window controller (lazy, requires themeEngine and eventHandler)
    private var _haloWindowController: HaloWindowController?
    var haloWindowController: HaloWindowController? {
        return _haloWindowController
    }

    /// Input coordinator (lazy, requires core)
    private var _inputCoordinator: InputCoordinator?
    var inputCoordinator: InputCoordinator? {
        return _inputCoordinator
    }

    /// Output coordinator (lazy, requires core)
    private var _outputCoordinator: OutputCoordinator?
    var outputCoordinator: OutputCoordinator? {
        return _outputCoordinator
    }

    /// Conversation coordinator (lazy, requires input and output coordinators)
    private var _conversationCoordinator: ConversationCoordinator?
    var conversationCoordinator: ConversationCoordinator? {
        return _conversationCoordinator
    }

    // MARK: - State

    /// Whether core services have been initialized
    @Published private(set) var isCoreInitialized: Bool = false

    /// Whether coordinators have been initialized
    @Published private(set) var areCoordinatorsInitialized: Bool = false

    // MARK: - Initialization

    private init() {
        // Empty - services initialized lazily or via explicit methods
    }

    // MARK: - Core Services Initialization

    /// Initialize core services after permissions are granted
    ///
    /// Call this method after verifying that all required permissions
    /// (Accessibility, Input Monitoring) are granted.
    ///
    /// - Throws: Error if core initialization fails
    func initializeCoreServices() throws {
        guard !isCoreInitialized else {
            print("[DependencyContainer] Core services already initialized")
            return
        }

        print("[DependencyContainer] Initializing core services...")

        // Create theme engine first (no dependencies)
        themeEngine = ThemeEngine()

        // Create event handler (haloWindow set later via setHaloWindow)
        eventHandler = EventHandler(haloWindow: nil)

        // Create Rust core with event handler
        core = try AetherCore(handler: eventHandler!)

        // Wire up bidirectional references
        eventHandler?.setCore(core!)

        isCoreInitialized = true
        print("[DependencyContainer] Core services initialized successfully")
    }

    /// Initialize all coordinators after core services are ready
    ///
    /// Call this method after `initializeCoreServices()` has completed.
    /// Coordinators depend on core services being available.
    func initializeCoordinators() {
        guard isCoreInitialized else {
            print("[DependencyContainer] Cannot initialize coordinators - core not initialized")
            return
        }

        guard !areCoordinatorsInitialized else {
            print("[DependencyContainer] Coordinators already initialized")
            return
        }

        print("[DependencyContainer] Initializing coordinators...")

        // TODO: Create coordinators as they are extracted from AppDelegate
        // _haloWindowController = HaloWindowController(...)
        // _inputCoordinator = InputCoordinator(...)
        // _outputCoordinator = OutputCoordinator(...)
        // _conversationCoordinator = ConversationCoordinator(...)

        areCoordinatorsInitialized = true
        print("[DependencyContainer] Coordinators initialized successfully")
    }

    // MARK: - Cleanup

    /// Clean up all resources
    ///
    /// Call this during app termination to properly release resources.
    func cleanup() {
        print("[DependencyContainer] Cleaning up resources...")

        // Stop monitoring
        clipboardMonitor.stopMonitoring()

        // Clear coordinators
        _conversationCoordinator = nil
        _outputCoordinator = nil
        _inputCoordinator = nil
        _haloWindowController = nil

        // Clear core services
        core = nil
        eventHandler = nil
        themeEngine = nil

        isCoreInitialized = false
        areCoordinatorsInitialized = false

        print("[DependencyContainer] Cleanup complete")
    }

    // MARK: - Convenience Accessors

    /// Get core, throwing if not initialized
    func requireCore() throws -> AetherCore {
        guard let core = core else {
            throw DependencyError.coreNotInitialized
        }
        return core
    }

    /// Get event handler, throwing if not initialized
    func requireEventHandler() throws -> EventHandler {
        guard let eventHandler = eventHandler else {
            throw DependencyError.eventHandlerNotInitialized
        }
        return eventHandler
    }

    /// Get theme engine, throwing if not initialized
    func requireThemeEngine() throws -> ThemeEngine {
        guard let themeEngine = themeEngine else {
            throw DependencyError.themeEngineNotInitialized
        }
        return themeEngine
    }
}

// MARK: - Dependency Errors

/// Errors that can occur when accessing dependencies
enum DependencyError: LocalizedError {
    case coreNotInitialized
    case eventHandlerNotInitialized
    case themeEngineNotInitialized
    case coordinatorNotInitialized(String)

    var errorDescription: String? {
        switch self {
        case .coreNotInitialized:
            return "AetherCore has not been initialized. Call initializeCoreServices() first."
        case .eventHandlerNotInitialized:
            return "EventHandler has not been initialized. Call initializeCoreServices() first."
        case .themeEngineNotInitialized:
            return "ThemeEngine has not been initialized. Call initializeCoreServices() first."
        case .coordinatorNotInitialized(let name):
            return "\(name) has not been initialized. Call initializeCoordinators() first."
        }
    }
}

// MARK: - Placeholder Types (to be created)

/// Placeholder for HaloWindowController (to be extracted from AppDelegate)
class HaloWindowController {
    // TODO: Implement in Step 1.3
}

/// Placeholder for InputCoordinator (to be extracted from AppDelegate)
class InputCoordinator {
    // TODO: Implement in Step 1.5
}

/// Placeholder for OutputCoordinator (to be extracted from AppDelegate)
class OutputCoordinator {
    // TODO: Implement in Step 1.4
}

/// Placeholder for ConversationCoordinator (to be extracted from AppDelegate)
class ConversationCoordinator {
    // TODO: Implement in Step 1.6
}
