import Foundation

/// Register the Stage 0 bridge.* handlers: ping / handshake / shutdown.
/// Later stages add ax.*, perm.*, media.*, screen.*, etc.
func registerBridgeHandlers(_ router: Router) async {
    await router.register("bridge.ping") { _ in
        .object(["pong": .bool(true)])
    }

    await router.register("bridge.handshake") { [router] _ in
        // `supported_methods` is required by the Rust HandshakeResult schema
        // and asserted by bridge_e2e.rs. Omitting it causes the desktop
        // bridge to degrade to disabled mode at every boot. Method
        // enumeration here only exposes the locally-registered IPC surface
        // (already gated by the Unix-socket peer-uid + token check), so it
        // does not widen the threat model.
        let methods = await router.supportedMethods()
        return .object([
            "swift_version": .string("2026.04.24"),
            "protocol_version": .number(2),
            "supported_methods": .array(methods.map { .string($0) }),
        ])
    }
}
