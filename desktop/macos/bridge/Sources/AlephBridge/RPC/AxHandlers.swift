import ApplicationServices
import Foundation

/// Register the three AX JSON-RPC handlers.
///
/// Every handler first checks `AXIsProcessTrusted()`.  If accessibility
/// permission has not been granted, all three handlers return a structured
/// `RpcError` with code -32001 and a `PermissionGuide` stub payload that
/// describes how the user can grant the permission.  Stage 4 will replace
/// the stub with a real `PermissionGuide` driven by `PermissionHandlers.swift`.
///
/// When permission is granted, calls are forwarded to the shared `AxQuerier`
/// actor; results are serialised via `encodeCodable(_:)` from JsonBridge.swift.
func registerAxHandlers(_ router: Router) async {
    let querier = AxQuerier()

    await router.register("ax.query_focused") { _ in
        try requireAxTrusted()
        let el = await querier.queryFocused()
        return try encodeCodable(QueryResult(element: el))
    }

    await router.register("ax.query_tree") { params in
        try requireAxTrusted()
        let args = try decodeCodable(params, as: QueryTreeParams.self)
        let depth = args.max_depth ?? 6
        let el = await querier.queryTree(
            pid: args.pid.map { pid_t($0) },
            maxDepth: depth
        )
        return try encodeCodable(QueryResult(element: el))
    }

    await router.register("ax.query_by_role") { params in
        try requireAxTrusted()
        let args = try decodeCodable(params, as: QueryByRoleParams.self)
        let list = await querier.queryByRole(
            role: args.role,
            pid: args.pid.map { pid_t($0) }
        )
        return try encodeCodable(QueryListResult(elements: list))
    }
}

// MARK: - Permission guard

/// Throw a structured `RpcError` if the process does not hold AX trust.
///
/// The `data` payload uses the frozen `PermissionGuide` wire format so that
/// clients can display actionable guidance immediately.  Stage 4 will replace
/// this stub with values driven by the real TCC query path.
private func requireAxTrusted() throws {
    guard AXIsProcessTrusted() else {
        throw RpcError(
            code: -32001,
            message: "permission denied: accessibility",
            data: .object([
                "kind": .string("accessibility"),
                "status": .object([
                    "kind": .string("accessibility"),
                    "granted": .bool(false),
                    "can_request_programmatically": .bool(false),
                    "restricted": .bool(false),
                ]),
                "deep_link": .string(
                    "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension"
                ),
                "human_readable_steps": .array([
                    .string("Open System Settings → Privacy & Security → Accessibility"),
                    .string("Find aleph-server or AlephBridge in the list"),
                    .string("Toggle the switch to enabled"),
                ]),
                "rationale": .string(
                    "Aleph needs Accessibility permission to read UI element trees of other apps"
                ),
            ])
        )
    }
}
