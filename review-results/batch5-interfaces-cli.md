# 静态审查报告：interfaces-cli

## 审查范围

| 项目 | 值 |
|------|-----|
| 单元名 | interfaces-cli |
| 路径 | interfaces/cli |
| 关注点 | CLI 客户端；R4 红线：shell 应纯 I/O 无业务逻辑 |
| 审查文件数 | 49 个 `.rs` 文件 |
| 代码行数（LOC） | 约 12,748 行 |

审查方式为无 diff 全量静态阅读，结合 `/tmp/rd-interfaces.json`（本次为空/损坏，未采用）进行。所有结论均基于当前代码亲自确认。

## 历史问题验证

| 历史问题 | 状态 | 说明 |
|----------|------|------|
| `commands/plugin_cmd.rs` TOML 注入 | **已修复** | `scaffold_plugin` 使用 `is_safe_plugin_name` 白名单（`plugin_cmd.rs:573-579`），禁止引号、等号、换行等危险字符，模板字符串插值安全 |
| `commands/doctor.rs` shell 内嵌 prompt 工程 | 仍存在，降级为 Low | `build_repair_brief`（`doctor.rs:235-256`）仍在 CLI 层拼接多句 LLM 指令；代码将其辩解为 R9/R10，但按 R4 仍属于接口层 prompt 工程 |
| `main.rs` marketplace 路由启发式 | 仍存在，降级为 Low | `main.rs:585` 仍用 `source.starts_with("github:") \|\| source.ends_with(".zip")` 做源分类；注释说明需 client-side I/O，但仍是 shell 层启发式 |

## 发现列表（按严重级排序）

### Medium

#### M1. `providers add --api-key` 强制命令行传入，凭据泄漏风险
- **文件**: `interfaces/cli/src/commands/providers_cmd.rs:103`
- **相关定义**: `interfaces/cli/src/commands/cli_args.rs:583`
- **严重级**: Medium
- **问题描述**: `aleph providers add` 的 `--api-key` 是必填 `String` 参数，没有交互式提示或环境变量回退。API key 会进入 shell history（`~/.bash_history`）并在进程列表（`ps`）中可见，导致凭据泄漏。
- **建议修法**: 与 `aleph secret set` 对齐：省略 `--api-key` 时调用 `rpassword::prompt_password` 隐藏输入；或支持 `ALEPH_PROVIDER_API_KEY` 等环境变量，并将命令行参数改为可选。

#### M2. `plugin pack` 跟随符号链接，可能打包目录外文件
- **文件**: `interfaces/cli/src/commands/plugin_cmd.rs:681`
- **严重级**: Medium
- **问题描述**: `add_dir_to_zip` 使用 `path.is_dir()` 判断递归，该调用跟随符号链接。如果插件目录内含指向 `/home`、`/etc` 等外部目录的 symlink，打包结果会把外部文件包含进 `.aleph-plugin.zip`，造成信息泄漏或路径遍历。
- **建议修法**: 使用 `std::fs::symlink_metadata` 判断条目类型，跳过 symlink（或显式报错）；若需支持 symlink，应解析并校验最终路径仍在 `base` 目录内。

### Low

#### L1. 生产代码使用 `unwrap()`（style 红线）
- **文件**: `interfaces/cli/src/commands/plugin_cmd.rs:285`
- **严重级**: Low
- **问题描述**: `validate` 的 JSON 输出路径使用 `serde_json::to_string_pretty(&json).unwrap()`。虽然 `serde_json::Value` 序列化实际不会失败，但违反了项目“生产代码禁止 `unwrap()`/`expect()`”的风格红线。
- **建议修法**: 改为 `unwrap_or_default()` 或 `map_err` 后返回 `CliError`。

#### L2. 安全条件检查后仍使用 `unwrap()`
- **文件**: `interfaces/cli/src/commands/plugin_cmd.rs:533`
- **严重级**: Low
- **问题描述**: `check_plugin_dir_exists` 中 `plugin_dir` 已用 `is_some_and` 判断存在，随后仍在同一分支调用 `plugin_dir.unwrap().display()`。结果安全，但违反风格红线。
- **建议修法**: 用 `if let Some(dir) = plugin_dir` 替换 `unwrap()`。

#### L3. `doctor` 命令仍在 CLI 层组装 LLM repair prompt
- **文件**: `interfaces/cli/src/commands/doctor.rs:235-256`
- **严重级**: Low
- **问题描述**: `build_repair_brief` 在 shell 层拼接了一段完整的 LLM 指令（含工具名引用 `doctor`、`self_config`、`vault_store`），要求 agent 诊断并修复安装问题。代码注释将其归为 R9/R10，但按 R4（接口层纯 I/O）属于在 CLI 内做 prompt 工程。
- **建议修法**: 将 brief 模板迁移到 Core 的 `doctor` tool/system prompt 中，CLI 仅负责把失败的 check 列表原样转发给 `agent.run`。

#### L4. `main.rs` 仍用启发式决定 plugin install 的 client-side 路径
- **文件**: `interfaces/cli/src/commands/main.rs:585`
- **严重级**: Low
- **问题描述**: `PluginAction::Install` 用 `source.starts_with("github:") \|\| source.ends_with(".zip")` 决定走 client-side 下载还是直接转发 daemon。注释已说明理由（需本地 I/O），但仍是 shell 层业务启发式，与 R4 的“纯 I/O”存在张力。
- **建议修法**: 若所有 source 类型都可由 daemon 处理，则统一转发；若必须 client-side 处理 zip/github，考虑在协议层显式暴露 `plugin.installFromLocalZip`/`plugin.installFromGitHub` 方法，而非在 CLI 做字符串分类。

#### L5. 多个模块超过 500 行，职责可进一步拆分
- **文件**: 
  - `interfaces/cli/src/commands/plugin_cmd.rs`（912 行）
  - `interfaces/cli/src/commands/doctor.rs`（914 行）
  - `interfaces/cli/src/commands/main.rs`（1065 行）
  - `interfaces/cli/src/commands/cli_args.rs`（1102 行，虽为纯 clap 定义，仍偏大）
  - `interfaces/cli/src/output/exec_echo.rs`（846 行）
  - `interfaces/cli/src/output/markdown.rs`（582 行）
  - `interfaces/cli/src/commands/cron_cmd.rs`（576 行）
- **严重级**: Low
- **问题描述**: 多个文件超过 500 行。`plugin_cmd.rs` 混合了 init/validate/pack/doctor 四个独立子命令；`main.rs` 包含大量 dispatch 臂和测试；`exec_echo.rs` 单个渲染模块接近 850 行，不利于维护。
- **建议修法**: 
  - 将 `plugin_cmd.rs` 拆分为 `plugin_init.rs`、`plugin_validate.rs`、`plugin_pack.rs`、`plugin_doctor.rs`。
  - 将 `main.rs` 中的 dispatch 函数按子系统拆分到 `commands/dispatch_*.rs`。
  - `exec_echo.rs` 可按渲染对象（tool/scratchpad/summary/retry 等）拆分子模块。

#### L6. GitHub release 下载文件名未做路径校验
- **文件**: `interfaces/cli/src/commands/plugins_cmd.rs:147`
- **严重级**: Low
- **问题描述**: `install` 从 GitHub API 返回的 `browser_download_url` 提取最后一段作为临时文件名：`download_url.rsplit('/').next().unwrap_or("plugin.zip")`。若服务端（或未来支持的非 GitHub 源）返回包含 `..` 的 URL，可能将 zip 写到 `tmp_dir` 之外。
- **建议修法**: 使用 `std::path::Path::new(filename).file_name()` 校验，拒绝含路径分隔符或 `..` 的文件名；或将下载文件命名为固定随机名。

## 架构红线合规快照

| 红线 | 合规情况 | 说明 |
|------|----------|------|
| R1 core 不调用平台 API | N/A（CLI 层） | CLI 本身不实现平台能力，仅通过 RPC 与 core 交互 |
| R2 复杂业务 UI 在 Leptos/WASM | 合规 | CLI 为纯文本界面，无业务 UI |
| R3 core 极简 | N/A | 重依赖（clap、tokio、zip 等）集中在 CLI crate，未污染 core |
| R4 接口层纯 I/O | 基本合规，2 处 Low | `doctor` 的 repair brief 与 `main.rs` 的 install 源启发式略有越界 |
| R7 Rust Core 是唯一大脑 | 合规 | 所有状态变更 RPC 转发到 daemon |
| R8 LLM 负责意图/路由 | 合规 | CLI 无正则路由用户意图 |
| R9 可配置项暴露为工具 | 合规 | 配置通过 `config.*` RPC 处理 |
| R10 智能在 prompt 中 | 部分合规 | `doctor` brief 把智能 prompt 放在 CLI 层，而非 Core |

## 结论

- **Critical**：0
- **High**：0
- **Medium**：2
- **Low**：6

CLI 整体符合 R4“纯 I/O”红线，历史问题中的 TOML 注入已修复。主要风险集中在**凭据通过命令行参数传递**和**plugin pack 对符号链接的处理**两处，建议优先修复。
