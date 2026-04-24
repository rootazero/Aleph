import Foundation

/// Register the three permission JSON-RPC handlers.
///
/// - `perm.check`         → `Perm.check(kind)` → `PermissionStatus`
/// - `perm.guide`         → `Perm.guide(kind)` → `PermissionGuide`
/// - `perm.open_settings` → `Perm.openSettings(kind)` → `OpenSettingsResult`
func registerPermHandlers(_ router: Router) async {
    await router.register("perm.check") { params in
        let args = try decodeCodable(params, as: CheckParams.self)
        return try encodeCodable(Perm.check(args.kind))
    }

    await router.register("perm.guide") { params in
        let args = try decodeCodable(params, as: GuideParams.self)
        return try encodeCodable(Perm.guide(args.kind))
    }

    await router.register("perm.open_settings") { params in
        let args = try decodeCodable(params, as: OpenSettingsParams.self)
        let ok = Perm.openSettings(args.kind)
        return try encodeCodable(OpenSettingsResult(ok: ok))
    }
}
