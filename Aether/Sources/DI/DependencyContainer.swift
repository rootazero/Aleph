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
//  Generic Registration API:
//  - register<T>(_:factory:) - Register a factory that creates new instances
//  - registerSingleton<T>(_:instance:) - Register a singleton instance
//  - resolve<T>(_:) - Resolve a registered dependency
//

import Foundation
import SwiftUI
import Combine

// MARK: - Registration Mode

/// How a dependency should be instantiated
enum RegistrationMode {
    /// Create a new instance each time resolve is called
    case factory
    /// Return the same instance every time
    case singleton
}

// MARK: - Dependency Registration

/// Type-erased wrapper for dependency factories
private struct AnyDependencyFactory {
    let mode: RegistrationMode
    let factory: () -> Any

    /// Cached singleton instance (only used when mode == .singleton)
    private var cachedInstance: Any?

    init(mode: RegistrationMode, factory: @escaping () -> Any) {
        self.mode = mode
        self.factory = factory
        // Pre-create singleton instance
        if mode == .singleton {
            self.cachedInstance = factory()
        }
    }

    func resolve() -> Any {
        switch mode {
        case .factory:
            return factory()
        case .singleton:
            return cachedInstance ?? factory()
        }
    }
}

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

    // MARK: - Generic Registration Storage

    /// Storage for registered dependencies, keyed by type name
    private var registrations: [String: AnyDependencyFactory] = [:]

    /// Lock for thread-safe access to registrations
    private let registrationLock = NSLock()

    // MARK: - Generic Registration API

    /// Register a factory that creates new instances each time
    ///
    /// - Parameters:
    ///   - type: The protocol or type to register
    ///   - factory: Closure that creates instances of the type
    func register<T>(_ type: T.Type, factory: @escaping () -> T) {
        let key = String(describing: type)
        registrationLock.lock()
        defer { registrationLock.unlock() }
        registrations[key] = AnyDependencyFactory(mode: .factory, factory: factory)
        print("[DependencyContainer] Registered factory for \(key)")
    }

    /// Register a singleton instance
    ///
    /// - Parameters:
    ///   - type: The protocol or type to register
    ///   - instance: The singleton instance to return
    func registerSingleton<T>(_ type: T.Type, instance: T) {
        let key = String(describing: type)
        registrationLock.lock()
        defer { registrationLock.unlock() }
        registrations[key] = AnyDependencyFactory(mode: .singleton, factory: { instance })
        print("[DependencyContainer] Registered singleton for \(key)")
    }

    /// Register a lazy singleton that's created on first resolve
    ///
    /// - Parameters:
    ///   - type: The protocol or type to register
    ///   - factory: Closure that creates the singleton instance
    func registerLazySingleton<T>(_ type: T.Type, factory: @escaping () -> T) {
        let key = String(describing: type)
        registrationLock.lock()
        defer { registrationLock.unlock() }
        registrations[key] = AnyDependencyFactory(mode: .singleton, factory: factory)
        print("[DependencyContainer] Registered lazy singleton for \(key)")
    }

    /// Resolve a registered dependency
    ///
    /// - Parameter type: The protocol or type to resolve
    /// - Returns: The resolved instance, or nil if not registered
    func resolve<T>(_ type: T.Type) -> T? {
        let key = String(describing: type)
        registrationLock.lock()
        defer { registrationLock.unlock() }
        guard let factory = registrations[key] else {
            print("[DependencyContainer] Warning: No registration found for \(key)")
            return nil
        }
        return factory.resolve() as? T
    }

    /// Resolve a registered dependency, throwing if not found
    ///
    /// - Parameter type: The protocol or type to resolve
    /// - Returns: The resolved instance
    /// - Throws: DependencyError.notRegistered if type is not registered
    func require<T>(_ type: T.Type) throws -> T {
        guard let instance = resolve(type) else {
            throw DependencyError.notRegistered(String(describing: type))
        }
        return instance
    }

    /// Unregister a dependency
    ///
    /// - Parameter type: The type to unregister
    func unregister<T>(_ type: T.Type) {
        let key = String(describing: type)
        registrationLock.lock()
        defer { registrationLock.unlock() }
        registrations.removeValue(forKey: key)
        print("[DependencyContainer] Unregistered \(key)")
    }

    /// Check if a type is registered
    ///
    /// - Parameter type: The type to check
    /// - Returns: true if the type is registered
    func isRegistered<T>(_ type: T.Type) -> Bool {
        let key = String(describing: type)
        registrationLock.lock()
        defer { registrationLock.unlock() }
        return registrations[key] != nil
    }

    // MARK: - Core Services (initialized after permissions)

    /// Rust core instance (- rig-core based, unified interface)
    private(set) var core: AetherCore?

    /// Event handler for Rust callbacks
    private(set) var eventHandler: EventHandler?

    // Theme engine removed - using unified visual style

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

    /// Halo window (lazy, initialized during coordinator setup)
    private var _haloWindow: HaloWindow?
    var haloWindow: HaloWindow? {
        return _haloWindow
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

        // Theme engine removed - using unified visual style

        // Get config path
        let configPath = getConfigPath()

        // === Core Initialization (rig-core based, unified interface) ===
        print("[DependencyContainer] Initializing core...")
        eventHandler = EventHandler(haloWindow: nil)
        core = try initCore(configPath: configPath, handler: eventHandler!)
        eventHandler?.setCore(core!)
        print("[DependencyContainer] core initialized successfully")

        isCoreInitialized = true
        print("[DependencyContainer] Core services initialized successfully")
    }

    /// Get the path to the configuration file
    private func getConfigPath() -> String {
        let homeDir = FileManager.default.homeDirectoryForCurrentUser
        let configDir = homeDir.appendingPathComponent(".config/aether")

        // Create config directory if it doesn't exist
        try? FileManager.default.createDirectory(
            at: configDir,
            withIntermediateDirectories: true,
            attributes: nil
        )

        return configDir.appendingPathComponent("config.toml").path
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

        // Create HaloWindow directly (no controller wrapper needed)
        _haloWindow = HaloWindow()

        // Connect event handler to Halo window
        if let eventHandler = eventHandler {
            eventHandler.setHaloWindow(_haloWindow)
        }

        // TODO: Create other coordinators as they are extracted from AppDelegate
        // _inputCoordinator = InputCoordinator(...)
        // _outputCoordinator = OutputCoordinator(...)

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
        _outputCoordinator = nil
        _inputCoordinator = nil
        _haloWindow = nil

        // Clear core services
        core = nil
        eventHandler = nil

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

    // Theme engine removed - using unified visual style
}

// MARK: - Dependency Errors

/// Errors that can occur when accessing dependencies
enum DependencyError: LocalizedError {
    case coreNotInitialized
    case eventHandlerNotInitialized
    case coordinatorNotInitialized(String)
    case notRegistered(String)

    var errorDescription: String? {
        switch self {
        case .coreNotInitialized:
            return "AetherCore has not been initialized. Call initializeCoreServices() first."
        case .eventHandlerNotInitialized:
            return "EventHandler has not been initialized. Call initializeCoreServices() first."
        case .coordinatorNotInitialized(let name):
            return "\(name) has not been initialized. Call initializeCoordinators() first."
        case .notRegistered(let typeName):
            return "No registration found for type: \(typeName). Register it first with register() or registerSingleton()."
        }
    }
}

// MARK: - Placeholder Types (to be created)

// HaloWindowController - Implemented in Controllers/HaloWindowController.swift

// InputCoordinator - Implemented in Coordinator/InputCoordinator.swift

// OutputCoordinator - Implemented in Coordinator/OutputCoordinator.swift
