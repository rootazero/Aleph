# Panel Community Assets and Mount Validation / Panel 社区资产与挂载验证

Date: 2026-10-30

## Scope / 范围

This report validates the constrained phase from an admitted Core declaration to an explicitly selected browser Panel. It covers one single-file ES module, exact bytes, SHA-256 integrity, the existing `PanelLoader` ABI, the existing `PanelShell` lifecycle, and deterministic cleanup. It does not add install, update, uninstall, marketplace, signature, default-release, live-notification, live-Gateway, external-WASM, multi-file, remote-URL, or Zahir runtime behavior.

本报告验证从 Core 已准入 declaration 到浏览器显式选择 Panel 的受约束阶段。范围包括单个 single-file ES module、exact bytes、SHA-256 integrity、现有 `PanelLoader` ABI、现有 `PanelShell` lifecycle 与 deterministic cleanup。不增加 install、update、uninstall、marketplace、signature、default-release、live-notification、live-Gateway、external-WASM、多文件、remote-URL 或 Zahir runtime 行为。

## Ownership and boundary / 所有权与边界

- Core `src/extension` remains the sole declaration, snapshot and source-revision authority. The authenticated `/ws` JSON-RPC snapshot bridge remains discovery-only.
- `PanelAssetPublicationStore` is a narrow in-process publication seam. It resolves only canonical `(plugin_id, plugin_version, tab_id, source_revision)` plus the declared versioned entry and exact bytes supplied by the host. It does not read a plugin root or filesystem, fetch URLs, parse manifests, redo trust, or create revisions.
- `PanelProjection::explicit_key` and `select_explicit` own browser selection. Snapshot arrival does not select or mount a tab. The only community route shape is `/panel/<plugin_id>/<tab_id>`.
- `PanelShell` and `PanelLoader` remain the browser lifecycle authorities. Existing generation, descriptor identity, integrity, async-result and one-shot cleanup fencing is reused. Built-in `/terminal` behavior is unchanged.
- The production HTTP route remains deferred. No authoritative production byte producer and browser credential injection were approved in this phase; the fixture therefore uses an explicit local adapter and makes no live Gateway claim.

- Core `src/extension` 仍是 declaration、snapshot 与 source-revision 的唯一权威。已认证的 `/ws` JSON-RPC snapshot bridge 仍然只负责 discovery。
- `PanelAssetPublicationStore` 是窄的 in-process publication seam。它只解析 canonical `(plugin_id, plugin_version, tab_id, source_revision)`、声明中的 versioned entry 与 host 提供的 exact bytes；不读取 plugin root 或 filesystem，不抓取 URL，不解析 manifest，不重新做 trust decision，也不创建 revision。
- `PanelProjection::explicit_key` 与 `select_explicit` 负责浏览器 selection。Snapshot arrival 不会选择或挂载 tab；唯一 community route shape 是 `/panel/<plugin_id>/<tab_id>`。
- `PanelShell` 与 `PanelLoader` 仍是浏览器 lifecycle authority。复用既有 generation、descriptor identity、integrity、async-result 与 one-shot cleanup fencing；built-in `/terminal` 行为不变。
- production HTTP route 仍 deferred。本阶段没有批准权威 production bytes producer 与 browser credential injection；因此 fixture 只使用显式 local adapter，不宣称 live Gateway。

## Evidence / 证据

### Prior discovery transport / 之前的 discovery transport

The earlier snapshot transport validation is recorded in `docs/reports/panel-community-snapshot-transport-validation.md` and was proven at the prior plan revision `c3c9e25d6` (the implementation branch was based on the newer current `main`). It established the canonical Core snapshot through the existing authenticated `/ws` method `panel.capabilities.snapshot`, one Core read, webchat epoch/request/revision fencing, reconnect recovery, and complete retraction snapshots. It did not prove asset publication or browser mounting.

之前的 snapshot transport 验证记录在 `docs/reports/panel-community-snapshot-transport-validation.md`，并在之前计划 revision `c3c9e25d6` 上完成验证（本实现分支基于更新后的当前 `main`）。它证明了通过现有已认证 `/ws` method `panel.capabilities.snapshot` 传输 canonical Core snapshot、单次 Core read、webchat epoch/request/revision fencing、重连恢复与完整 retraction snapshot；它没有证明 asset publication 或 browser mounting。

### Publication seam / Publication seam

`src/gateway/server/panel_asset_route.rs` provides:

- canonical `PanelAssetIdentity { plugin_id, plugin_version, tab_id, source_revision }`;
- `publish` admission for enabled, trusted, community declarations with `public_base == "./tabs/"`, safe versioned entry, valid `sha256-` SRI and matching bytes;
- replacement eviction by the same identity slot and explicit `retract`;
- `serve` refusal for unauthorized, missing, stale, retracted, entry-mismatched or unsafe requests.

Because the seam accepts bytes rather than paths, filesystem traversal and symlink escape are structurally unavailable. The tests also reject a traversal entry, arbitrary URL, wrong integrity bytes, unauthorized request, missing identity and generic plugin fields. The core contract compiled with `cargo check -p alephcore --tests -j1`; the targeted core test binary was attempted with `-j1` but test-binary linking exceeded the available command timeouts (1800s, then 900s), so runtime execution of those Rust unit tests is recorded as unverified.

由于 seam 接收 bytes 而不是 paths，filesystem traversal 与 symlink escape 在结构上不可用。测试也拒绝 traversal entry、arbitrary URL、错误 integrity bytes、unauthorized request、missing identity 与 generic plugin fields。core contract 通过 `cargo check -p alephcore --tests -j1` 编译；targeted core test binary 曾以 `-j1` 尝试运行，但 test-binary linking 超过命令超时（1800s，随后 900s），因此 Rust unit tests 的 runtime execution 记录为未验证。

### Explicit mount fixture / 显式挂载 fixture

The `panel-tab-spike` fixture uses `interfaces/webchat/tests/fixtures/community-panel.mjs`, whose exact integrity is:

```text
sha256-/81ggyzAwnDzBLiMA4vU6nbBeCk6HAuZIri/T/CqkWE=
```

The build script publishes only `tabs/community.fixture/1.0.0/verified.mjs` from an in-process fixed map and emits `.panel-spike/community-publication.json` with identity, entry, public base, integrity and the explicit non-live source marker. `/panel/` makes no publication request. `/panel/community.fixture/verified` uses the seeded canonical snapshot, explicit selection, the real `PanelShell`, the existing exact-byte loader, and the fixture module; the test checks both mount roots, one exact publication request, close-tab cleanup, no remaining shell content and no second request after returning to `/panel/`.

`panel-tab-spike` fixture 使用 `interfaces/webchat/tests/fixtures/community-panel.mjs`，其 exact integrity 为上面的值。

Build script 只从 in-process fixed map 发布 `tabs/community.fixture/1.0.0/verified.mjs`，并生成 `.panel-spike/community-publication.json`，其中包含 identity、entry、public base、integrity 与明确的 non-live source marker。`/panel/` 不产生 publication request。`/panel/community.fixture/verified` 使用 seeded canonical snapshot、explicit selection、真实 `PanelShell`、现有 exact-byte loader 与 fixture module；测试检查两个 mount roots、一次 exact publication request、close-tab cleanup、无残留 shell content，以及返回 `/panel/` 后没有第二次 request。

## Checks / 检查结果

- `cargo check --no-default-features --features panel-tab-spike -j1`: passed.
- `cargo test --no-default-features --features panel-modular panel:: --lib -j1`: passed 73/73.
- `bun run panel:spike:build`: passed; this builds the panel WASM, wasm-bindgen output, Tailwind CSS and terminal pilot fixture.
- `bun run panel:spike:test`: passed 5 artifact tests; 6 live-Gateway tests skipped by the existing gate.
- `rustfmt --edition 2024` was run on the new Rust publication module and targeted Rust changes. `git diff --check` passed.
- The repository-wide `cargo fmt --check` remains blocked by unrelated baseline formatting differences; it was not treated as a clean full-format pass.
- `bun check` for the build script is not an available validation because the repository invocation lacks ambient Node/Bun/DOM declarations; the build itself is the relevant executable check.

- `cargo check --no-default-features --features panel-tab-spike -j1`：通过。
- `cargo test --no-default-features --features panel-modular panel:: --lib -j1`：73/73 通过。
- `bun run panel:spike:build`：通过；构建 panel WASM、wasm-bindgen output、Tailwind CSS 与 terminal pilot fixture。
- `bun run panel:spike:test`：5 个 artifact tests 通过；6 个 live-Gateway tests 按既有 gate 跳过。
- `rustfmt --edition 2024` 已用于新 Rust publication module 与 targeted Rust changes；`git diff --check` 通过。
- repository-wide `cargo fmt --check` 仍被无关的 baseline formatting differences 阻塞，不能记作 full-format pass。
- build script 的 `bun check` 因仓库调用缺少 ambient Node/Bun/DOM declarations，不能作为可用验证；build 本身是相关 executable check。

## Deferred and blocked / 延后与阻塞

The phase does not prove a live Gateway HTTP publication route, browser credential propagation for that route, live notifications, or Zahir runtime mount. It does not introduce install/update/uninstall, signatures, marketplace policy, default release changes, automatic selection, raw filesystem publication, arbitrary URLs, multi-file assets, relative imports, or external WASM. The prior discovery transport evidence remains valid and separate from this fixture evidence.

本阶段没有证明 live Gateway HTTP publication route、该 route 的 browser credential propagation、live notifications 或 Zahir runtime mount。没有引入 install/update/uninstall、signatures、marketplace policy、default release changes、automatic selection、raw filesystem publication、arbitrary URLs、多文件 assets、relative imports 或 external WASM。之前的 discovery transport evidence 仍然有效，并且与本 fixture evidence 分开。

The worktree must remain local and clean after the final commit. No merge or push is authorized by this phase.

最终 commit 后 worktree 必须保持本地且 clean。本阶段没有授权 merge 或 push。
