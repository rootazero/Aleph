import ApplicationServices
import Foundation

/// Register the AX JSON-RPC handlers.
///
/// Every handler first checks `AXIsProcessTrusted()` and returns a structured
/// `PermissionGuide` when accessibility permission has not been granted.
///
/// When permission is granted, calls are forwarded to the shared `AxQuerier`
/// actor; results are serialised via `encodeCodable(_:)` from JsonBridge.swift.
func registerAxHandlers(_ router: Router) async {
    let querier = AxQuerier()

    await router.register("ax.query_focused") { params in
        try requireAxTrusted()
        // An older client sends no params at all for this method, which decodes
        // to "no pid" — the system-wide question it used to be the only way to
        // ask.
        let args = (try? decodeCodable(params, as: QueryFocusedParams.self)) ?? QueryFocusedParams()
        let el = await querier.queryFocused(pid: args.pid.map { pid_t($0) })
        return try encodeCodable(QueryResult(element: el))
    }

    await router.register("ax.query_tree") { params in
        try requireAxTrusted()
        let args = try decodeCodable(params, as: QueryTreeParams.self)
        // Cap depth to prevent unbounded AX tree traversal (information disclosure / OOM).
        let depth = min(args.max_depth ?? 6, maxAxQueryDepth)
        let (el, budget) = await querier.queryTree(
            pid: args.pid.map { pid_t($0) },
            maxDepth: depth,
            maxNodes: clampMaxNodes(args.max_nodes)
        )
        return try encodeCodable(QueryResult(element: el, budget: budget))
    }

    await router.register("ax.query_by_role") { params in
        try requireAxTrusted()
        let args = try decodeCodable(params, as: QueryByRoleParams.self)
        let (list, budget) = await querier.queryByRole(
            role: args.role,
            pid: args.pid.map { pid_t($0) },
            maxNodes: clampMaxNodes(args.max_nodes)
        )
        return try encodeCodable(QueryListResult(elements: list, budget: budget))
    }

    await router.register("ax.snapshot_references") { params in
        try requireAxTrusted()
        // Presence alone determines refusal. Keep the untyped JSON number out
        // of UInt64 decoding, since the router stores numbers as Double.
        if case .object(let fields) = params {
            switch fields["window_id"] {
            case .none, .some(.null): break
            default: throw AxReferenceRefusal.windowScopeUnsupported.rpcError
            }
        }
        let args = try decodeCodable(params, as: AxSnapshotParams.self)
        return try encodeCodable(await querier.snapshotReferences(args))
    }

    await router.register("ax.act_on_reference") { params in
        try requireAxTrusted()
        let args = try decodeCodable(params, as: AxReferenceActionParams.self)
        return try encodeCodable(await querier.actOnReference(args))
    }

    await router.register("ax.set_value") { params in
        try requireAxTrusted()
        let args = try decodeCodable(params, as: SetValueParams.self)
        let result = try await querier.setValue(args)
        return try encodeCodable(result)
    }

    await router.register("ax.perform_action") { params in
        try requireAxTrusted()
        let args = try decodeCodable(params, as: PerformActionParams.self)
        let result = try await querier.performAction(args)
        return try encodeCodable(result)
    }
}

// MARK: - Node budget

let maxAxQueryDepth = 20
let maxAxNodes = 10_000

/// Mirror of `aleph_protocol::desktop_bridge::methods::ax::clamp_max_nodes`.
///
/// The Rust side clamps before sending; this repeats it because the helper must
/// not be able to be talked into an unbounded walk by a malformed request, and
/// because the field is optional on the wire for older clients.
func clampMaxNodes(_ requested: Int?) -> Int {
    let defaultMaxNodes = 1_500
    guard let requested, requested > 0 else { return defaultMaxNodes }
    return min(requested, maxAxNodes)
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
            data: try encodeCodable(Perm.guide(.accessibility))
        )
    }
}
