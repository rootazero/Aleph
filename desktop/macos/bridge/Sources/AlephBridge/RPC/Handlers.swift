import Foundation

/// Register the Stage 0 bridge.* handlers: ping / handshake.
/// Graceful shutdown is not an RPC method — the helper exits on stdin EOF
/// (see `Server.run`) or when its parent dies (see `ParentWatch`).
/// Later stages add ax.*, perm.*, media.*, screen.*, etc.
func registerBridgeHandlers(_ router: Router) async {
    await router.register("bridge.ping") { _ in
        .object(["pong": .bool(true)])
    }

    await router.register("bridge.handshake") { [router] _ in
        // `supported_methods` is required by the Rust HandshakeResult schema
        // and asserted by bridge_e2e.rs. Omitting it causes the desktop
        // bridge to degrade to disabled mode at every boot. Method
        // enumeration here only exposes the locally-registered IPC surface;
        // it does not widen the threat model because the helper has no
        // transport layer beyond stdio — `Server.swift` reads from
        // `FileHandle.standardInput.fileDescriptor`, which is a pipe owned
        // exclusively by `aleph-server` (the spawner). The pipe's OS-enforced
        // ownership isolation is the only auth on the channel; there is no
        // socket listener, peer-uid check, or token exchange. `setpgid` and
        // `ParentWatch` (kqueue `NOTE_EXIT` on the parent) make the helper
        // exit when its parent dies; together with the pipe ownership that
        // is the full threat model. **Do not add a TCP/Unix-socket listener
        // here without first implementing a real peer-uid + token check** —
        // the existing comment in the previous revision of this block named
        // those checks as already in place, which was wrong, and misled
        // anyone reading it into thinking the auth gap was closed.
        let methods = await router.supportedMethods()
        return .object([
            "swift_version": .string("2026.04.24"),
            "protocol_version": .number(2),
            "supported_methods": .array(methods.map { .string($0) }),
        ])
    }
}
