import Foundation

/// Register the Stage 0 bridge.* handlers: ping / handshake / shutdown.
/// Later stages add ax.*, perm.*, media.*, screen.*, etc.
func registerBridgeHandlers(_ router: Router) async {
    await router.register("bridge.ping") { _ in
        .object(["pong": .bool(true)])
    }

    await router.register("bridge.handshake") { _ in
        let methods = await router.supportedMethods()
        return .object([
            "swift_version": .string("2026.04.24"),
            "protocol_version": .number(2),
            "supported_methods": .array(methods.map { .string($0) }),
        ])
    }

    await router.register("bridge.shutdown") { _ in
        // Best-effort: return success, then exit on the next run-loop tick.
        Task.detached {
            try? await Task.sleep(nanoseconds: 50_000_000)
            exit(0)
        }
        return .object(["shutting_down": .bool(true)])
    }
}
