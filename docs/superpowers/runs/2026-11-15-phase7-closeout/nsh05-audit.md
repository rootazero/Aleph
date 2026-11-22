# NSH 0.5 compatibility scope freeze / NSH 0.5 兼容范围冻结

Date / 日期: 2026-11-15. This is a read-only upstream audit for the Capability Phase7 handoff; it does not implement or accept the separate NSH migration. / 本文是 Capability Phase7 交接所需的上游只读审计，不实施或验收独立的 NSH 迁移。

## Pinned upstream / 冻结上游

- Release: `v0.5.0`, published `2026-11-11T12:04:05Z`.
- Annotated tag object: `729ea4c6810112160e890684e8a8d34528ec6da9`.
- Release commit: `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`.
- Sources: [release notes](https://github.com/longbridge/nsh/releases/tag/v0.5.0), [versioned compatibility contract](https://github.com/longbridge/nsh/blob/v0.5.0/docs/dev/compatibility.md), [versioned embedding guide](https://github.com/longbridge/nsh/blob/v0.5.0/docs/dev/embedding.md), [workspace manifest](https://github.com/longbridge/nsh/blob/v0.5.0/Cargo.toml), [platform matrix](https://github.com/longbridge/nsh/blob/v0.5.0/README.md).
- Exact fetched text, URLs and SHA-256 values are retained in `artifacts-20261112-a/p7-nsh05-upstream-audit-v1.json`, `p7-nsh05-upstream-api-audit-v1.json` and `p7-nsh05-host-types-audit-v1.json`.

## Compatibility findings / 兼容性结论

1. Upstream explicitly says: “No release is recorded yet, 0.3.0, 0.4.0 and 0.5.0 included: the first one recorded starts the contract.” The permanent compatibility promise therefore cannot be used to claim that Aleph's 0.4 spike runs unchanged on 0.5. / 官方明确排除 0.3、0.4、0.5 的永久兼容承诺，不能据此认定本地 0.4 spike 已兼容 0.5。
2. Release notes replace the old desktop window API with `WindowRef`, `<Window>` and window kinds, rather than an alias. Secondary-window and shutdown ownership must be ported and tested. Local migration remains unexecuted. / 新窗口 API 是替换而非别名；次级窗口与退出所有权需要后续移植验证。
3. Native hosts use the matching 0.5 platform crate's `AppView`: macOS `nsh_platform_macos`, Windows `nsh_platform_windows`, Linux `nsh_platform_linux`. The embedding guide documents the host channel and optional Tokio runtime handle. This is upstream API evidence, not an Aleph native-host execution result. / 这些是上游 API 事实，不是 Aleph 原生宿主运行结果。
4. Version-pinned workspace `rust-version` is `1.97`. The published platform matrix is macOS 14+, Windows 10 1809+, Linux GTK 4.14+. Linux WebView requirements additionally include WebKitGTK 6.0 (2.40+) per the upstream workspace manifest. / 冻结工具链与平台下限，后续两桌面 QA 必须使用这些约束。
5. JS modules/types, CLI, crates, runtime and platform host must all be selected from this same release. Media capability grants, window types and activation-policy changes are explicit migration checks. Merely building Aleph's Rust Core does not establish their compatibility. / CLI、crate、runtime、host、TS 类型必须同版；Core 编译不能替代此项验收。

## Track boundary / 轨道边界

The approved continuation plan separates Capability Phase8 (Integration & Security) from NSH Phases0–6. This audit freezes the next NSH Phase0 scope. Phase1–4 parity, ADR-006/008 prerequisites for the NSH PluginRuntime track, and the two-week dual-run before NSH Phase6/Tauri removal stay on that separate migration track. They are not silently marked passed and no Tauri removal is performed. / 已批准计划将 Capability Phase8 与 NSH Phase0–6 分列；本审计冻结下一步 NSH Phase0 范围。Parity、PluginRuntime ADR 前提和移除 Tauri 前的两周双运行继续保留在 NSH 迁移轨道，不虚报通过，也不在本轮删除 Tauri。

Unperformed / 未执行: no CLI installation/update, dependency installation, local NSH 0.5 compilation, native host launch, platform parity QA, dual-run or Tauri migration. The retained local 0.4 spike remains historical evidence only. / 未安装或更新 CLI/依赖，未编译或启动本地 0.5 宿主，未做平台 parity、双运行或 Tauri 迁移。
