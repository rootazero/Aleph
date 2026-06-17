# 进程管理 (Process Management)

> 由根 `CLAUDE.md` 的「进程管理」一行指针指向本文。

Singleton 强制由 OS 级 `flock` 保证（Spec C, 2026-05-02 起改为结构化保护）：

- `aleph-server start` 在 `main()` 进入任何 DB/vault 操作之前先获取
  `~/.aleph/data/aleph.lock`。第二个 `start` 会立即以 exit 64 退出，
  并在 stderr 打印持锁进程的 PID。
- 所有 CLI 写子命令（`secret`、`hooks`、`plugins` 等）通过
  `with_policy` 分发：服务在跑时，写操作通过 `/v1/admin/*` IPC 转发；
  服务不在时，CLI 自己拿锁本地写入。两条路径都不会与服务竞争。
- OS 在进程退出（正常、panic、SIGKILL）时自动释放 `flock`。`kill -9 <pid>`
  之后**无需 sleep**，可立即 `aleph-server start`。
- 反向回归脚本 `scripts/spec_c_regression.sh` 锁住四条不变量：
  SQLite 走 `open_sqlite_safe`、vault/acp 走 `vault_io`/`atomic_io`、
  每个 CLI 子命令显式声明 policy、`acquire_instance_lock` 不再有遗留 caller。

如果看到 `Stale lock file detected (PID X not running)`，可以安全
`rm ~/.aleph/data/aleph.lock`（理论上不会出现，因为 flock 是 OS 管理的；
该诊断仅作防御性提示）。
