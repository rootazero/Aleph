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

## 浏览器引擎子进程 (Browser engine children)

Aleph 自己 spawn 浏览器（2026-09-05 起 Chromium，2026-09-06 起还有 obscura），而 `std::process::Child`
**不在 drop 时 kill**，驱动它的外部 CLI 又从来不是它的父进程。所以「谁负责关掉它」必须写下来：

- **sidecar 记录**：每一个被起起来的引擎在
  **`~/.aleph/data/browser/chromium/<engine>-<session-key>.json`** 留一条
  `EngineSidecar{engine, pid, http_url, data_dir, build}`。两处细节都不是笔误：
  - 目录叶名**冻结**在 `chromium`（`engine/process.rs::SIDECAR_REGISTRY_LEAF`，由
    `the_sidecar_registry_leaf_is_frozen` 按名钉住）。它现在装着两个引擎的记录，名字却只说一个——
    刻意的：改名要在**同一个提交**里迁移旧叶下的 `*.json`，否则每台升级的机器每个 profile 永久漏一个
    浏览器，而且**没有任何后续提交救得回来**，因为改名之后没人知道旧记录存在。一个读者一分钟的困惑
    vs 一个永久孤儿，选前者。
  - **引擎在文件名里，而且排在前面**：`<engine>-<key>` 是单射的（`Engine::as_str()` 的值不含 `-`
    且首字节不同），`<key>-<engine>` 不是（session key 允许 `-`，于是一个叫 `default-obscura`
    的 profile 会占掉 `default` 的 obscura 记录）。从 `<key>.json` 升级到这个形状是
    `switch_engine` 逼出来的：它让**同一个 session key 下同时跑着源引擎和目标引擎**，粗粒度的键
    于是让目标的记录在启动那一刻覆盖掉源的（整个迁移期间那个活 obscura 不可回收），然后源的
    `stop_launched` 又把同一个文件删掉（目标从此对清扫不可见）。**判据 §19**：**键必须和它所寻址的
    世界一样细**——这正是 §19 的旗舰实例（2026-09-06 新增的那个形状），**不是 §12**：这个键只有一个
    作者，数它得 1 而那个 1 是**对的**，坏的是粒度。⚠️ `engine/process.rs` 的模块 doc 在同一件事上
    仍写着「判据 §12」——那是本轮之前写下的，在 `src/` 里，本轮（文档轮）不改，记在
    [FEATURE_LOCATOR §3.12](FEATURE_LOCATOR.md) 第八轮 ㉑⑬。
  记录放在**一个注册表目录**而不是各自的 profile 目录里——一个 profile 可以把 `user_data_dir` /
  `storage_dir` 指到任何地方，记录跟着走的话开机清扫就只扫得到「派生出来的那个根」，配过目录的
  profile（本仓 QA 自己就配）永远扫不到。`engine` 字段带 `#[serde(default)]`（且是一个**具名**的
  `Engine::chromium_default`，不是 `Default`——产品默认是 obscura、兼容默认是 Chromium，是两个问题），
  所以 2026-09-05 那一轮写下的、没有这个字段的记录仍读作 Chromium。
- **孤儿回收**：开机时按记录里的 pid 读 argv 并**整 token 相等**地比对我们自己那个 flag——Chromium 是
  `--user-data-dir=<path>`，obscura 是 `--storage-dir=<path>`（`Engine::data_dir_flag`）。读的是
  `sysinfo::Process::cmd()` 那个 **argv 向量**，不是拼好的命令行：拼好的串上只表达得出 `contains`，
  而 `--user-data-dir=<root>/default` 是活着的 `--user-data-dir=<root>/default-2` 的子串，于是这条
  检查会杀掉**邻居 profile** 的浏览器。探测结果是三态 `ArgvProbe{Absent, Unreadable, Argv}`，四条臂：
  匹配 → 杀并删记录；`Argv` 但不匹配（pid 被回收）→ 不杀、删记录；`Absent`（含**僵尸**，已退出等父进程
  收尸）→ 删记录；**`Unreadable` → 什么都不做、记录留着**（Windows 上读不到 argv 是常态，而「读不出来」
  不是「不在了」，判据 §8）。清扫**按记录迭代**，所以它的作用域是注册表：一个从没被记下来的浏览器不会被
  检查，也就永远不是这条守卫的控制组。
- **退出时杀**：挂在**两个** daemon 退出点——有序停机（`start/mod.rs` 的 `run_until_shutdown` 返回处）与
  卡死兜底（`start/helpers.rs` 的 `std::process::exit(0)` 之前）。只挂前者的话，负载中的
  `aleph-server stop` 照样漏浏览器，而 `SHUTDOWN_FAILSAFE = 5 s` 正是为那种停机设的；有序那条路拿到的
  预算是 `ORDERLY_BROWSER_STOP_BUDGET = SHUTDOWN_FAILSAFE / 2`，取一半是因为停引擎只是有序拆卸的**一
  步**，后面每一件事还要在同一个 5 s 里跑完。这段代码是在和花掉 failsafe 的那个看门狗**赛跑**——一个早先
  的版本要了 35.5 s（它所处 failsafe 的七倍），于是进程在等待途中被强制退出，浏览器没停掉，它后面的
  projector flush / monitor / MCP / 端点清理 / `GatewayStop` 钩子也一起跳过了。**把 failsafe 调大不是
  出路**：5 s 是外部天花板（`aleph stop` 的 SIGTERM ≤5 s 然后 SIGKILL ≤2 s），越过它落下来的是监管者的
  SIGKILL 而不是我们自己的 `exit(0)`，连兜底那条路的回收也没了。一条**编译期** `assert!`
  （`ORDERLY_BROWSER_STOP_BUDGET * 2 <= SHUTDOWN_FAILSAFE`）钉住这个关系：动了其中一个常量而不动另一个，
  构建直接停。
- **规则不是预算：SIGKILL + 有界回收，绝不做优雅握手**——不 SIGTERM-然后-等，不发 CDP `Browser.close`，
  不向一个可能正是停机卡死原因的进程发起往返。`terminate()` 先 `child.kill()`，再在 `grace` 内**轮询**
  回收（不是 `wait()` 阻塞：kill 可能失败，而 `wait()` 会把这个 worker 挂到进程自己碰巧退出为止）。
  那个 `grace` 是**回收**的预算，不是给进程的缓刑。
- **代价，写下来而不是留给人发现**：被 SIGKILL 的 Chromium 一定在 profile 目录里留下 `SingletonLock` /
  `SingletonSocket` 与一个「did not shut down correctly」标记，下次启动可能弹恢复提示。本仓**没有**做宽限
  期，也**没有**清理陈旧 singleton 锁。

详见 [FEATURE_LOCATOR §3.12](FEATURE_LOCATOR.md) 第七轮 ⑯⑰ 与第八轮 ⑫。

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
