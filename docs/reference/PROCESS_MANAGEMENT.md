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

启动时任何 `Another Aleph instance holds the lock …` 都意味着**锁此刻确实
被某个活进程握着**（OS 报了争用；持有者一退出 OS 就释放）。三种措辞只是
持有者记录 `aleph.lock.pid` 说了什么：`PID N` = 能点名 → `kill N` 或
`aleph stop`；`holder record names PID N, which is not running` = 记录过期
（通常是 fork 后没改写 PID 的守护进程）；`holder record … missing or
unreadable` = 点不了名。**三种都不要 `rm aleph.lock`**：Unix 上删掉被锁的
文件再重建是新 inode，下一个启动者会拿到另一把锁、和持有者并排跑——正是
vault HMAC 丢数据的双实例条件。去进程列表里找到持有者停掉它。
持有者 PID 记在**未加锁**的 sidecar `aleph.lock.pid` 里：正常退出时随锁
一起删除（`InstanceLock::drop`），服务端的强制退出失效路径
（`SHUTDOWN_FAILSAFE` → `process::exit`）也会在退出前清掉它
（`remove_held_holder_records_before_exit`），只有崩溃 / SIGKILL 会留下
它——锁本身此时已被 OS 释放，下一次启动直接赢得锁并覆盖记录，**不会**报
上面那些话；`aleph doctor` 的 `core/instance-lock` 才是给这种残留用的，
`--fix` 会清掉——但它**先探锁**（`instance_lock::is_lock_held`），锁被握着时
只报「Holder record stale」、什么都不删；删除本身也是握着锁做的
（`remove_holder_record_if_lock_free`），且只删 sidecar、不动 `aleph.lock`。若文件系统本身拒绝加锁（无 lockd 的 NFS、不支持字节范围锁
的挂载），启动会直接报该 OS 错误——那种情况下 `rm` 同样没用，要把
`ALEPH_HOME` 指到本地文件系统。

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
