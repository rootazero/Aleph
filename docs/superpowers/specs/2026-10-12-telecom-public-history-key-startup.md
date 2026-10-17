# Telecom public history: dedicated key startup / 电信公开历史：独立密钥启动

## Decision / 决策

Use an **operator-provided dedicated local key file**, independent of login,
owner/shared Gateway tokens, and the secrets vault. No inline secret is accepted
in `[telecom_public_history]`; only an absolute `key_file` path is configured.
Startup never generates a key, searches fallback locations, initializes a vault
for this feature, or deletes/rotates/resets a database to recover from a bad key.

使用**运营者提供的专用本地密钥文件**，与登录、owner／共享 Gateway token 和秘密 vault
完全独立。配置只接受绝对 `key_file` 路径，不接受内联密钥。启动不生成密钥、不查找后备位置、
不为此初始化 vault，也不通过删除、轮换或重置数据库来修复错误密钥。

This is a startup/configuration contract and implementation note, **not evidence
of production deployment, live SMS/PSTN testing, or successful test execution**.

本文记录启动配置契约与实现，不代表生产部署、真实 SMS/PSTN 验收或测试执行成功。

## Configuration / 配置

```toml
[telecom_public_history]
enabled = true
key_file = "/home/aleph/.local/keys/telecom-public-history.key"
ttl_secs = 86400
max_turns = 32
max_session_bytes = 65536
max_total_bytes = 8388608
```

| Setting / 配置项 | Default / 默认值 | Allowed / 范围 |
| --- | --- | --- |
| `enabled` | `false` | explicit opt-in / 显式启用 |
| `key_file` | absent / 无 | absolute operator-supplied path / 运营者指定绝对路径 |
| `ttl_secs` | `86400` | `0..=86400`; zero disables / 0 关闭 |
| `max_turns` | `32` | `1..=32` |
| `max_session_bytes` | `65536` | `1..=65536` |
| `max_total_bytes` | `8388608` | `1..=8388608` |

Absent section preserves the old public single-turn behavior. A present section
still defaults to disabled. Limits may only be lowered through the existing
`PublicHistoryConfig::with_limits` contract; invalid limits or a supplied relative
key path fail `Config::validate`, even if disabled. A missing key is deliberately
not a config-validation error: enabled startup warns and leaves persistent
history unavailable, without stopping ordinary public single-turn execution.

不写该节保持原公开单轮行为；写了但不设 enabled 仍关闭。限制只能下调，复用现有
`PublicHistoryConfig::with_limits`；非法限制或已提供的相对路径在 `Config::validate`
中失败，关闭时也如此。未指定／缺失密钥不属于配置验证失败：启用后的启动只告警并关闭
持久历史，不阻断普通公开单轮执行。

`enabled = false` or `ttl_secs = 0` skips key/DB/history-directory I/O completely.
The production hook also avoids resolving the data directory for these cases;
this does not prevent unrelated channel subsystems from doing their normal I/O.

`enabled = false` 或 `ttl_secs = 0` 完全跳过密钥、数据库、历史目录 I/O；此功能的生产
启动钩子也不解析数据目录。这不意味着其他通道子系统停止原有的正常 I/O。

## Key provisioning and checks / 密钥提供与检查

Provision **32 cryptographically random bytes**, encoded as exactly **64 ASCII
hex characters**, optionally followed by one LF (`\n`). Upper/lowercase hex
encodes the same key; startup canonicalizes it to lowercase for the String-based
`SecretsCrypto` API. Whitespace, CRLF, extra newlines, non-hex bytes, truncation,
and oversized input are refused. Entropy must come from the operator's secure
random generator; the loader cannot infer randomness from a valid hex string.

提供 **32 字节密码学随机数据**，编码为恰好 **64 个 ASCII 十六进制字符**，末尾可有
一个 LF（`\n`）。大小写编码同一密钥，加载器统一转为小写供 String 型 `SecretsCrypto`
接口使用。空白、CRLF、多行、非 hex 字节、截断及超长输入均拒绝；随机性由运营者的安全
随机源保证，加载器无法仅从合法 hex 内容推断熵。

On Unix the loader opens a read-only FD using `O_NOFOLLOW` and `O_NONBLOCK`,
then verifies **that FD's metadata**: regular file, effective-user ownership,
exact mode **0600** (including refusal of special bits). Reads are bounded to
66 bytes, detecting input longer than the 65-byte maximum. A final-component
symlink, group/world-accessible file, directory, or malformed/missing key cannot
cause history-directory creation. Raw key input is zeroized; the crypto engine
owns the resulting secret. Enabled non-Unix startup returns `Unsupported` rather
than assuming secure ACLs.

Unix 上只读 FD 使用 `O_NOFOLLOW` 与 `O_NONBLOCK`，随后核验**该 FD 的元数据**：普通
文件、euid 所有、权限恰为 **0600**（特殊位也拒绝）。读取最多 66 字节以检测超过合法
65 字节的输入。末级符号链接、组／其他用户可访问文件、目录、缺失／非法密钥均不会创建
历史目录。原始输入退出时清零，最终秘密由加密引擎持有。非 Unix 启用时返回 Unsupported，
不假定 ACL 安全。

Operator example (manual provisioning, **not a startup action**):
运营者示例（人工提供，**非启动动作**）：

```sh
# Run as the service account; use a trusted, private parent directory.
umask 077
openssl rand -hex 32 > /home/aleph/.local/keys/telecom-public-history.key
chmod 600 /home/aleph/.local/keys/telecom-public-history.key
```

Create the private parent first. Do not run this over an existing key to “repair”
history: doing so replaces the key and makes old ciphertext unreadable. Do not
reuse a login/owner token or place the key in configuration, logs, Relay payloads,
or beside the database. Keep the key and database backups separately protected.

先准备可信的私有父目录。不要为“修复”历史而覆盖已有密钥，否则旧密文无法读取。不要复用
登录／owner token，不要将密钥放入配置、日志、Relay 载荷或数据库旁；分别保护密钥与
数据库备份。

## Storage and lifecycle / 存储与生命周期

The Core-local fixed path is `<data_dir>/telecom-public-history/public-history.db`.
`ensure_private_dir` creates/verifies the history directory as owner-only on Unix,
refusing unsafe ownership, permissions, or symlinks rather than repairing them.
The data-directory parent must already exist. The key is loaded before preparing
this directory. `open_configured_history(config, data_dir)` returns a local handle
and does not touch process globals; production installs that handle once before
channel startup. Duplicate installation produces an explicit static warning.
All startup-failure warnings omit key material, configured paths, caller text,
binding identifiers, and underlying error details.

Core 本地固定路径为 `<data_dir>/telecom-public-history/public-history.db`。
`ensure_private_dir` 在 Unix 上创建／核验所有者独占的历史目录；不安全所有权、权限或
符号链接会被拒绝，不自动修复。数据目录父级必须已存在；先核验密钥才准备历史目录。
非全局 `open_configured_history(config, data_dir)` 返回本地句柄，不污染进程级槽；生产
在通道启动前仅安装一次，重复安装明确静态告警。启动失败告警不包含密钥、配置路径、
调用者文本、binding 或底层错误详情。

Configuration/key changes require a **process restart**. Disabling persistence
is not a purge or a physical-erasure command. A wrong key against existing
ciphertext must make loading fail closed; startup must never auto-delete,
rotate/reset, or fall back to another key/store. Lost keys have no automatic
recovery: stop the service and restore the correct separately-protected key.
The existing history-store load behavior was changed separately; its no-purge
regression must be verified together with startup wiring before release.

配置／密钥变更需要**重启进程**。关闭持久化不等于清理或物理擦除。已有密文配错密钥时，
读取必须 fail-closed；启动不得自动删除、轮换／重置或使用其他密钥／存储回退。丢失密钥
没有自动恢复路径：停止服务并恢复正确的独立备份密钥。历史存储旧 load 行为已由另一项修改
处理，发布前必须与启动接线一起验证不自动删除的回归用例。

## Boundaries and verification / 边界与验证

- Public completed-turn history only; not owner session history, memory/profile,
  login auth, or an approval journal. / 仅公开完整轮次历史；非 owner 会话、记忆、登录认证或审批日志。
- Local euid, root, or compromised service-account access remains trusted; file
  mode checks cannot protect secrets from those actors. `O_NOFOLLOW` protects
  the final key component, not every parent. Protect parent directories against
  replacement; neither parent components nor SQLite sidecars are FD-pinned by
  this helper. / 本地 euid、root 或被攻陷服务账户仍处于信任边界内；权限检查无法防御他们。
  O_NOFOLLOW 保护末级密钥，不保护所有父级；需防止父目录被替换，本 helper 不用 FD 固定
  父级或 SQLite sidecar。
- Existing private-directory helper uses path-based creation/metadata checks;
  it is reused unchanged. No key-rotation/migration or live-reload mechanism is
  introduced. / 原目录 helper 仍为路径式创建／元数据检查，原样复用；未引入密钥轮换迁移或热重载。
- SQLite WAL/backups may retain deleted ciphertext; TTL is logical retention,
  not physical secure erasure. Byte caps count payloads, not full filesystem
  usage. / SQLite WAL／备份可能保留过期密文；TTL 非物理安全擦除，字节上限非总磁盘占用。
- Tests use explicit temporary directories and no global installation or config
  environment mutation. Coverage includes disabled/zero-TTL no-I/O, encrypted
  two-turn reopen/no plaintext, lowered limits, config validation, unsafe keys,
  owner-metadata refusal, unsafe directories, and wrong-key no-reset recovery.
  / 测试使用显式临时目录，不安装全局槽、不更改配置环境；覆盖关闭／零 TTL 不写盘、两轮
  加密重开／无明文、下调限制、配置验证、不安全密钥、所有权元数据、目录及错误密钥不重置恢复。
- **Cargo/compilation/tests are not run by this scoped implementation task**;
  parent sequential validation is required. Non-Unix runtime, privileged real
  foreign-owner/chown scenarios, and live telecom QA are not claimed verified.
  / **本实现任务未运行 Cargo／编译／测试**；需父任务串行验证。非 Unix 运行、真实提权
  chown／异主文件及真实电信 QA 不宣称已验证。
