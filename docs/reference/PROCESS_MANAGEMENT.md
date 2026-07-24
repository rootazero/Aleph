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

## 日志轮转 (Log Rotation)

Aleph 有两条独立日志流，轮转策略不同：

- **结构化日志** `~/.aleph/logs/aleph-server.log.YYYY-MM-DD`：tracing 写入，**按天轮转 + 7 天保留**，每行带时间戳。排查优先 grep 这一份。
- **裸 stdout/stderr 流**（daemon 的 `--log-file`，或前台 shell 重定向的 `server.log`）：只装 banner / warnings / panics / 子进程输出。**非 TTY 时 tracing 的 console 层会被丢弃**，所以这条流里的行没有时间戳。

每次启动会向 stdout 打一行可 grep 的启动标记，用于在裸流里定位当前 boot（跨重启累积时不再误判历史行）：

```
ALEPH-BOOT ts=<RFC3339> pid=<pid> version=<ver>
grep ALEPH-BOOT ~/.aleph/server.log | tail -1   # 之后的行 = 本次启动
```

裸流的轮转分两种情形：

- **Daemon 模式**（`--daemon --log-file <path>`）：`daemonize()` 拥有该 fd，**启动时自轮转**——若文件来自更早的一天、或超过 ~5 MB，归档为 `<name>.YYYY-MM-DD`（用文件自身最后写入日作后缀），并沿用 7 天保留老化。同日重启追加到同一文件（靠 `ALEPH-BOOT` 标记区分 boot）。无进程内轮转：单次长跑期间 fd 不会中途轮转，但裸流平时几乎无输出，增长可忽略。
- **前台 / 手动 shell 重定向**：fd 属于 shell，Aleph 无法轮转，需系统级 logrotate。仓库提供现成配置 [`scripts/aleph-server.logrotate`](../../scripts/aleph-server.logrotate)（`copytruncate` + daily + 7 天），改掉里面的绝对路径后 `cp` 到 `/etc/logrotate.d/` 即可。
