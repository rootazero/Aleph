# Cargo.lock Audit for NSH v0.5 / NSH v0.5 Cargo.lock 审计

**Date / 日期:** 2026-11-15
**Compared / 对比:** frozen pre-NSH lock blob from commit `e08af3789dbf194f02f08461c215eb3c7701557f` versus the retained generated `Cargo.lock`
**Baseline SHA-256 / 基线哈希:** `bb1b647bb8f53785467c2dd47b40ed0bd46f2514c9de4e6e94bc29fccc900345`
**Retained lock SHA-256 / 当前锁哈希:** `d6f2e99dd2df5868b15f2940d74cf8845ea7c26d3ff69819ce42422180bb1cdb`
**Purpose / 目的:** determine whether the pinned NSH v0.5 graph can be added without broad resolver churn.

## Findings / 结果

- HEAD contains 1,537 package entries; the current lock contains 1,596.
- Comparing full package identities `(name, version, source)` gives 328 added identities and 269 removed identities.
- Comparing package names gives 37 newly introduced names, 12 removed names, and 285 names whose version/source sets changed. The earlier approximate count of 280 unrelated version-set changes used the release review’s narrower classification; the exact current name-set comparison is 285.
- The current lock contains 7 pinned NSH Git packages: `nsh-base`, `nsh-core`, `nsh-engine`, `nsh-macros`, `nsh-platform-apple`, `nsh-platform-macos`, and `nsh-updater`.
- The NSH packages changed from the old local v0.4 package entries to v0.5 Git entries at commit `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`.
- The generated graph also introduces transitive packages such as `rust-i18n`, `rust-i18n-support`, `ar_archive_writer`, `nsh-platform-apple`, `nsh-updater`, `zruntime`, and related platform/encoding packages. Several existing package families move at the same time, including ICU, Leptos, compression, Cranelift and Windows/macOS support crates.

## Minimal-merge experiment / 最小合并实验

A minimal lock merge containing `aleph-nsh-shell` and the generated NSH dependency closure added 115 sections but failed Cargo resolver validation. The conflicts were:

- existing `webpki-roots 1.0.9` versus NSH-selected `webpki-roots 1.0.12`;
- after attempting to pin that package back, existing `toml 0.8.2` versus NSH `rust-i18n-support` requiring `toml 0.8.23`.

The experiment was restored before retaining Cargo’s full generated lock. This proves that manually splicing only the visible NSH sections does not produce a valid locked graph under the current resolver.

## Decision / 决策

Retain the full generated `Cargo.lock` for the approved NSH v0.5 implementation slice. It is the only lock state that passed offline locked compilation for `alephcore`, `aleph-server`, and `aleph-nsh-shell` after the missing crates were fetched. The broad churn is a material dependency review risk and must be reviewed before release or wider workspace adoption.

The audit does not claim that every changed package is required by NSH alone; it records resolver-wide consequences of adding the pinned NSH graph to the existing workspace. A future lock-minimization pass needs a resolver-compatible package/version strategy and separate validation. It is outside the current Phase9 planning-only scope.

## Evidence / 证据

- HEAD snapshot used for comparison: `/tmp/aleph-cargo-lock-head.toml`.
- Pinned source: `https://github.com/longbridge/nsh.git`, rev `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`.
- Current checks passed with `--offline --locked`: `cargo check -p alephcore --lib`, `cargo check --bin aleph-server`, and `cargo test -p aleph-nsh-shell`.
