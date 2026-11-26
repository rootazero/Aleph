# Linux 通用 stdin/EOF 准入验证 / Linux Generic stdin/EOF Eligibility

**结论 / Verdict: NO-GO（T1）。** 未找到既能开放任意解释器后续输入、又能保留现有文本硬规则及写入时授权复验的生产配置。不公开 stdin 或 EOF。 / No production configuration was found that enables arbitrary interpreter input while preserving the existing hard text rules and authorization at the write boundary. Neither stdin nor EOF is exposed.

本记录执行[已确认计划](../plans/2026-11-26-shell-capability-stdin.md)的 T1，仅核对现有实现。产品基线为 `854505aa6`，运行的归档 Core 测试二进制来自构建产品提交 `fdfa1077d`；两者与本分支在产品及测试源码、Cargo 清单和锁文件上相同。 / This records T1 of the confirmed plan against the existing implementation. The product baseline is `854505aa6`; the archived Core test binary was built from `fdfa1077d`. Both match this branch's product and test sources, Cargo manifest, and lockfile.

## 策略实效 / Policy Enforcement

选取约束最少的已挂载 `WorkspaceSandbox` 配置：关闭可调命令策略，operator 自定义 Shell 文本规则不启用。`src/sandbox/factory.rs:39–88` 把关闭可调策略的分支挂载为 `CommandPolicy::hardline_only()`；启用或回退到默认策略的分支也仍保留同一底线。安全内核 hook 在启用时另加 operator 的 blocked 规则，danger 规则仅告警。没有一个更宽松且仍保留当前硬规则的现有配置可以绕过此判断。 / Use the least restrictive mounted configuration: disable the tunable command policy and omit optional operator text rules. The factory mounts the hardline-only hook in that configuration; the enabled and default-fallback branches retain the same floor. The optional security-kernel hook adds operator blocks; danger matches are advisory. No less restrictive mounted configuration avoids this gate while preserving the existing rules.

`src/sandbox/workspace/mod.rs:235–277,365–385` 在调用 driver 前准备命令并运行钩子。`src/sandbox/command_policy/mod.rs:263–279,345–367,445–470` 的硬规则针对规范化命令文本；`command_text` 仅采集程序、参数和已准备的启动 stdin。`src/sandbox/security_kernel_hook.rs:31–80` 消费同一份启动文本。进程就绪后没有供这些钩子消费的输入，也没有在解释器解释操作前复用此判定的生产接线。审批只回答是否授权一次请求，不执行这些规则；即使内核阻止某个副作用，也不能等同于在执行前拒绝命中硬规则的文本。 / Workspace runs both hooks before the driver. Their input contains only the program, arguments and prepared startup stdin. There is no production route from post-readiness bytes or the interpreter's subsequent actions back to that decision. Human consent does not evaluate the rules, and denying a resulting OS effect is not equivalent to refusing the matched text before execution.

### 硬规则清单 / Hard Rule Inventory

以下是 `src/sandbox/command_policy/rules.rs:237–521` 的全部 16 条无条件 `Block` 规则；即使在 Linux 上，跨平台条目也属于同一文本匹配集合。验证条件对每行相同：仅把该类样本作为内存字符串交给规则判定器，绝不执行；匹配样本在启动文本中须拒绝，但仅出现于后续输入时，现有启动钩子根本没有收到该样本。此表记录源码证明，不声称逐项运行了新的写入试验。 / These are all 16 unconditional `Block` rules. Cross-platform entries are part of the same text matcher on Linux. The falsifier for every row is the same: use a matching fixture solely as an in-memory string, never as a command. The launch matcher must block it when present in launch text; when it exists only in later input, the launch hook receives no such bytes. This table records the source boundary, not a claim that 16 new live write tests ran.

| 规则 / Rule | 启动前须拒绝的形状 / Launch-time block obligation |
| --- | --- |
| `fork_bomb` | 自管道式后台递归函数形状 / Self-piping background recursion shape |
| `rm_no_preserve_root` | 显式关闭根目录删除保护的移除命令 / Removal explicitly disabling root preservation |
| `rm_rf_root` | 裸根目录递归删除 / Recursive removal of a bare root |
| `dd_to_block_device` | 写入原始块设备的复制命令 / Copy to a raw block device |
| `mkfs_device` | 在设备节点建立文件系统或交换区 / Filesystem or swap formatting at a device node |
| `redirect_to_block_device` | 覆盖原始块设备的重定向 / Raw block-device overwrite by redirection |
| `device_wipe_tools` | 原始块设备擦除或丢弃 / Raw block-device wiping or discard |
| `macos_disk_destruction` | macOS 整盘销毁或覆盖 / macOS whole-disk destruction or overwrite |
| `win_format_volume` | Windows 卷格式化 / Windows volume formatting |
| `win_recursive_root_delete` | Windows 根路径或注册表根递归删除 / Windows drive or registry root recursive deletion |
| `win_delete_shadow_copies` | Windows 卷影副本销毁 / Windows shadow-copy destruction |
| `win_backup_catalog_destruction` | Windows 备份目录或系统状态备份销毁 / Windows backup catalog or system-state backup destruction |
| `win_disk_wipe_tools` | Windows 原始磁盘擦除 / Windows raw disk wiping |
| `win_bcdedit_delete` | Windows 引导配置记录删除 / Windows boot-configuration entry deletion |
| `win_boot_recovery_disable` | 禁用 Windows 引导恢复 / Windows boot recovery disablement |
| `win_registry_hive_delete` | 强制删除 Windows 注册表根 hive / Forced Windows registry-root hive deletion |

若启用 operator 自定义 `blocked` 规则，要求还覆盖实际配置中的文本语义；现有测试只证明它能检查同一 `SandboxCommand` 的预备 stdin。逐次审批、分块检查或 cgroup 配额不能作为该语义的等价证明。 / When optional operator blocks are enabled, the corresponding configured text semantics also apply. The existing regression establishes coverage of prepared stdin on the same launch command, not a subsequent stream. Per-write approval, chunk predicates, and cgroup quotas do not establish equivalence.

## 授权与原生门禁 / Authority and Native Gate

| 检查 / Check | 观察 / Observation |
| --- | --- |
| 有人值守的 Gateway Agent 配置 / Attended Gateway Agent route | `src/gateway/execution_engine/turn_permissions.rs:391–434` 读取全局权限并合并 Agent、频道值；`src/gateway/execution_engine/tool_service_builder.rs:173–207` 把结果注入每请求 scoped service。`src/tools/scoped/mod.rs:431–492` 设定路由与会话，`src/tools/scoped/dispatch.rs:564–585,1257,1429,1608` 消费现有策略并校验 Capability lease。这条路线未提供写入时可与撤权同步的授权。 / Per-turn policy and live lease checks exist, but do not create a grant synchronized with revocation at a later pipe effect. |
| 全局撤权 / Global policy mutation | `src/gateway/handlers/config.rs:861–892` 在共享配置写锁内持久化并替换全局策略。上面的 scoped service 使用解析后的值；当前没有覆盖同一写入操作的授权临界区。即使再读一次快照，也不能把撤权与效果排序。 / The RPC mutates global policy under the shared config write lock. The scoped service receives resolved values, with no shared authorization boundary around the hypothetical pipe effect; a second snapshot read alone cannot order them. |
| Linux 整组回收 / Linux group cleanup | 不继续做委派 scope 实验：策略和授权前置门已失败。`src/sandbox/platforms/linux/bwrap.rs:737–785` 的现有路径允许部分配置降级，并在启动前尝试归组；`src/sandbox/cgroup_v2.rs` 的清理并非整组退出的确认。第一阶段真实 Linux QA 只证明直接受管子进程回收，不作为本阶段的整组证据。 / The delegated experiment was skipped after the preceding gates failed. Existing attachment and cleanup are not verified whole-group settlement. Stage 1's real Linux evidence covered direct-child reaping only. |

## 验证与未交付项 / Verification and Not Delivered

在隔离 HOME 与 ALEPH_HOME 中用同一归档二进制串行复跑，共 158 项通过、零失败、零忽略。 / Serial rerun in isolated HOME and ALEPH_HOME: 158 passed, zero failed or ignored.

| 已有测试范围 / Existing filter | 通过 / Passed |
| --- | ---: |
| `sandbox::factory::tests::` | 4 |
| `sandbox::command_policy::` | 141 |
| `sandbox::security_kernel_hook::` | 7 |
| `tools::scoped::production_policy_tests::` | 6 |

覆盖关键回归 `build_sandbox_disabled_command_policy_still_blocks_hardline`、`hardline_rules_are_all_block_action`、`hook_denies_hardline_even_in_hardline_only_policy`、`custom_patterns_see_a_script_arriving_on_stdin`。这些结果验证既有判定，不证明新交互能力。 / These regressions substantiate the existing launch boundary, not a new interactive capability.

没有修改产品代码、公开 schema、权限或现行 reference 契约；未执行 T2–T5、原生续写/EOF 实验或整组回收验证。没有实现候选，因此未发起独立实现终审，未把此前设计自查算作终审。未改 `main`、未合并、未推送。 / No product code, schema, permissions or current reference contract changed. T2–T5, native write/EOF and whole-group cleanup verification did not run. There is no implementation candidate for independent final review; earlier design self-review was not counted. No changes to `main`, merge or push.
