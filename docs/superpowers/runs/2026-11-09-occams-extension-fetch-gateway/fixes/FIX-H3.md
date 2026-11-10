# FIX-H3 — region-splice track (raw API)

Second-wave follow-up / 第二轮修复复核。以下既有 H3 条目状态来自上一轮，本轮仅重新处理 H3-03 与提示中的新增问题。

## H3-03 — FIXED — `handle_update` now calls `matches_bind_address`, preserving the original host when its projected mode is unchanged; only explicit mode switches select the canonical address. / update 已接入模式判据，同模式保留原始 host，显式切换才选标准地址。

Added one handler-level regression test covering GET→UPDATE for `192.168.1.20`, `::`, `::1`, `127.0.0.1`, and `0.0.0.0`; it checks exact persisted hosts, unrelated SSRF setting persistence, changed/unchanged restart flags, and explicit switches in both directions. / 新增一条 handler 级回归测试，覆盖指定地址、IPv6、默认地址、其他设置落盘及 restart 判据。

Trade-off / 取舍：the existing two-value DTO cannot normalize a custom address within the same mode; the wire enum is unchanged. / 保持两值 wire 枚举，因此不能用同模式保存将自定义地址规范化为默认地址。

## H3-03-UNWIRED — FIXED — The previously unused helper now controls the actual update path; existing conditional host writes and other settings/restart handling remain intact. / 原未使用 helper 已连接真实更新路径，保留原写入及其他设置处理。

## H3-PROJECTS-CLONE — SKIPPED — Verified `Project.workspace_path: Option<PathBuf>` and `resolve_in_workspace(root: &std::path::Path, rel: &str)`; captured `PathBuf`/`String` clones are valid, so no code change is needed. / 已核实真实类型与签名，非缺陷，不改代码。

## H3-EXECUTION-SERIAL — SKIPPED — `Cargo.toml` already declares `serial_test = "4"` under dev-dependencies; fully qualified `#[serial_test::serial(...)]` needs no import. / 已存在测试依赖，全限定属性无需额外 import，不改代码。

Validation / 验证：`rustfmt --edition 2021 --config skip_children=true --check src/gateway/handlers/security_config.rs` and scoped `git diff --check` passed. No cargo commands or test execution, compilation, git history changes, or unrelated refactors were performed. / 格式与差异空白检查通过；按要求未运行 cargo、测试或编译，未更改 git 历史或无关代码。

Previous-wave statuses (not re-audited) / 上轮状态（本轮未复审）：
- H3-02: FIXED
- H3-01: FIXED
- H3-04: FIXED
- H3-05: FIXED
- <ID>: SKIP(no location)
- H3-06: FIXED
- H3-07: FIXED

---

# 第二轮区域拼接修复 / second-wave region-splice track (raw API)

Worktree: `/home/zou/data/workspace/Aleph-wt-fix-extfx`. Model: `kimi/kimi-for-coding`. One API call per finding.

- H3-03: FIXED
