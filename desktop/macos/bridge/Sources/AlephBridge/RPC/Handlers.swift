import Foundation

/// Register the Stage 0 bridge.* handlers: ping / handshake / shutdown.
/// Later stages add ax.*, perm.*, media.*, screen.*, etc.
func registerBridgeHandlers(_ router: Router) async {
    await router.register("bridge.ping") { _ in
        .object(["pong": .bool(true)])
    }

    await router.register("bridge.handshake") { _ in
        // Intentionally omits `supported_methods` — method enumeration helps
        // attackers map the attack surface. Only return version info.
        return .object([
            "swift_version": .string("2026.04.24"),
            "protocol_version": .number(2),
        ])
    }
}
