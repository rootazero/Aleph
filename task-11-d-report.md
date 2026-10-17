# Task11-D / Task11-D

## 完成内容 / Delivered

- 新增 `src/tools/scoped/task11_rewrite_tests.rs`，仅包含 source-only scoped dispatch witnesses：
  - `task11_file_ops_rewrite_rechecks_final_claim_before_handler`
  - `task11_config_rewrite_records_original_and_final_approval_actions`
  - `task11_confirmation_rewrite_requires_final_bytes_and_approval`
  - `task11_hook_ask_without_transport_is_nobody_asked`
- `src/tools/scoped/tests.rs` 仅追加 `task11_rewrite_tests` 模块声明。
- 使用真实 `HookExecutor`/Unix command hook、真实 `ScopedToolService` dispatch、计数 handler effects、最终输入 marker 和 approval actions；测试 session 名称均唯一。

## 保留的既有 witness / Existing witness retained

- `src/tools/scoped/tests.rs:5443` `proof_for_original_input_cannot_authorize_rewrite` 保留不动。它已经精确证明：original approval 不能授权 rewritten bytes、rewritten call 最终拒绝、handler zero effects、effective marker 为 `None`、original/rewritten approval grant keys 不同、card summaries 区分两份输入，因此本任务没有重复复制该 assertion。
- 既有 `execute_with_cancel_effective_returns_rewritten_input`、hook-denied effective `None` 与 no-hook effective capture 也保持不变。

## 边界与限制 / Boundary and limitations

- 未修改生产代码、dispatch gates、Hook implementation、registry、已有 assertions 或其他文件。
- 未运行 Cargo、rustc、测试二进制、安装、git 或 delegation；仅运行 scoped `rustfmt`：
  `rustfmt --edition 2021 src/tools/scoped/task11_rewrite_tests.rs src/tools/scoped/tests.rs`
- 因此本交付只声明 source handoff，未声明编译或运行时测试通过。主流程应在 source freeze 后串行执行其既定验证。
- Windows shell-hook runtime 未验证，按 brief 留作独立平台验证。

## SHA256

- `src/tools/scoped/task11_rewrite_tests.rs`: `140d6f086399e1aa75445fcd7c76a40f4dd3975cd1470106da2a9d9521642196`
- `src/tools/scoped/tests.rs`（含本次追加声明）: `dcb1ceb5b3411fb69c8c5cb23bb86cc7a30e4fee25b4c74e077f8eac850b6526`

## 未完成 / Not done

- 未执行 Cargo/rustc/test verification；未宣称 tests compile or pass。
- 未修改或补造 durable audit ledger；报告依 brief 明确保留该边界。
