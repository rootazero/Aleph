import ApplicationServices
import Foundation

/// Register the `input.*` JSON-RPC handlers — the pid-targeted input rail.
///
/// The methods below were declared in
/// `shared/protocol/src/desktop_bridge/methods/input.rs` long before anything
/// implemented them; until now Aleph published an RPC surface with no handlers
/// behind it. These are those handlers.
///
/// Every one of them delivers its event with `CGEvent.postToPid(_:)` — into the
/// target process's own event queue — so the user's physical cursor never moves
/// and the target app does not have to be frontmost. Read the delivery note at
/// the top of `InputSession.swift` before relying on that: targeted delivery is
/// a rail, not a guarantee, and results say which rail ran (`delivery`) rather
/// than claiming the app acted.
///
/// Every handler first checks `AXIsProcessTrusted()`, mirroring `registerAxHandlers`.
/// Synthesized events from an untrusted process are dropped by the window server
/// *silently*, which is the worst possible failure: the model would be told the
/// click landed and would spend its next turns reasoning about a screen that
/// never changed. A structured `-32001` with a `PermissionGuide` payload turns
/// that silence into something the user can act on.
func registerInputHandlers(_ router: Router) async {
    let session = InputSession()

    await router.register("input.click") { params in
        try requireInputTrusted()
        let args = try decodeCodable(params, as: ClickParams.self)
        return try encodeCodable(try await session.click(args))
    }

    await router.register("input.double_click") { params in
        try requireInputTrusted()
        // Shares `ClickParams` with `input.click` by contract; `click_count` is
        // ignored here — the method name already said 2.
        let args = try decodeCodable(params, as: ClickParams.self)
        return try encodeCodable(try await session.doubleClick(args))
    }

    await router.register("input.type_text") { params in
        try requireInputTrusted()
        let args = try decodeCodable(params, as: TypeTextParams.self)
        return try encodeCodable(try await session.typeText(args))
    }

    await router.register("input.key_combo") { params in
        try requireInputTrusted()
        let args = try decodeCodable(params, as: KeyComboParams.self)
        return try encodeCodable(try await session.keyCombo(args))
    }

    await router.register("input.key_button") { params in
        try requireInputTrusted()
        let args = try decodeCodable(params, as: KeyButtonParams.self)
        return try encodeCodable(try await session.keyButton(args))
    }

    await router.register("input.scroll") { params in
        try requireInputTrusted()
        let args = try decodeCodable(params, as: ScrollParams.self)
        return try encodeCodable(try await session.scroll(args))
    }

    await router.register("input.drag") { params in
        try requireInputTrusted()
        let args = try decodeCodable(params, as: DragParams.self)
        return try encodeCodable(try await session.drag(args))
    }

    await router.register("input.hover") { params in
        try requireInputTrusted()
        let args = try decodeCodable(params, as: HoverParams.self)
        return try encodeCodable(try await session.hover(args))
    }

    await router.register("input.mouse_button") { params in
        try requireInputTrusted()
        let args = try decodeCodable(params, as: MouseButtonParams.self)
        return try encodeCodable(try await session.mouseButton(args))
    }

    await router.register("input.cursor_position") { _ in
        // Read-only, and the one `input.*` method with no rail: it reports where
        // the user's real cursor is. Nothing on the targeted rail moves it.
        try requireInputTrusted()
        return try encodeCodable(try await session.cursorPosition())
    }
}

// MARK: - Permission guard

/// Throw a structured `RpcError` if the process does not hold AX trust.
///
/// Both rails need it: `AXUIElementPerformAction` obviously, and
/// `CGEvent.postToPid` because macOS gates synthesized input on the same
/// Accessibility grant.
private func requireInputTrusted() throws {
    guard AXIsProcessTrusted() else {
        throw RpcError(
            code: -32001,
            message: "permission denied: accessibility",
            data: try encodeCodable(Perm.guide(.accessibility))
        )
    }
}
