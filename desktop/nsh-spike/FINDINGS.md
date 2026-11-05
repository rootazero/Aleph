# NSH Spike Findings

## Task 1: Spike crate skeleton + NSH compile verification

- **状态**: ✅ cargo build exit 0
- **NSH commit**: `2ab3b6bc27f151afd5d932c63746a8b4b15a00e9` (describe: `v0.4.0-24-g2ab3b6bc`)
  - 注：本地 clone 超出精确 tag v0.4.0 24 commits。Phase 1 正式实施时需锁定精确 commit。

### API 偏差（设计文档 §4.3 需更新）

| 设计文档假设 | 实际 API | 影响 |
|---|---|---|
| `type Root = Box<dyn Fn() -> AnyElement>` | `Root` 是 struct，通过 `Root::of::<C>()` 构建（`nsh_core/src/element/component.rs:535`） | 低风险——编译器直接报错；改为 `Root::of::<App>()` 即可 |
| `AppOptions::default()` 可能需要显式字段 | `AppOptions::default()` 可直接使用，所有字段有默认值 | 无影响，简化了初始化 |
| `#[component]` 宏生成函数 | 实际生成类型（struct），不能 `App()` 调用，要用 `Root::of::<App>()` | 低风险——编译器报 E0618 明确指示 |

### 必要的结构修复

- **nested workspace**: spike crate 需要本地空 `[workspace]` 段，否则被 Aleph 根 workspace 拒绝（exit 101）。
  已修复：在 `Cargo.toml` 顶部加 `[workspace]`（空 members）。根 `Cargo.toml` 零修改。

### 依赖验证

- `nsh-engine` features: `[]`（空）——QuickJS/nsh-js 未引入 ✅
- 无 `nsh-js`、`rquickjs`、`quickjs` resolved 节点 ✅
- 根 `Cargo.toml`/`Cargo.lock` diff = 0 ✅
- Mach-O arm64 二进制含字符串 `NSH Spike: compiles OK` ✅

### objc2 版本冲突风险（Task 1 验证结果）

- NSH 依赖 `objc2 0.6.5`（workspace 锁定）
- Aleph `desktop/shell` 用 `objc2 =0.6.5`（精确钉版）
- Aleph `desktop/macos` 用 `objc2 0.6`
- 三者版本完全兼容，**无冲突** ✅（之前设计文档标为「高风险」可降级为「低风险」）

### tokio 版本（待 Task 3 验证）

- NSH 用 `tokio = "1"`（semver 宽松）
- Aleph 用 `tokio = "1.53"`
- 理论上兼容，Task 3 实测确认。

## Task 2: 最小 NSH 窗口运行验证

- **状态**: ✅ 窗口成功打开
- `cargo build`: exit 0 (9.95s)
- `cargo fmt --check`: exit 0
- `cargo clippy -- -D warnings`: exit 0

### AppOptions 字段验证

| 字段 | 值 | 编译 | 运行时 |
|---|---|---|---|
| `title: String` | `"Aleph NSH Spike".to_string()` | ✅ | ✅ 窗口标题正确 |
| `size` (整数元组字面量) | `(800, 600)` | ✅ | ✅ 外框 800×632（内容区未单独量）|
| `app_id: String` | `"com.aleph.nsh-spike".to_string()` | ✅ | 配置可启动；应用标识行为未单独探测 |
| `..AppOptions::default()` | 其余字段全部有默认值 | ✅ | ✅ |

### 运行时观察

- 进程 PID 74848，CoreGraphics 读到 1 个屏幕窗口（name=Aleph NSH Spike）
- 窗口外框：`{Width:800, Height:632, X:356, Y:87}`（height=632 含标题栏，内容区未单独量）
- 无 objc2 runtime crash
- 结束方式：SIGTERM（非自然退出），非验收失败

### 二进制归因

- 继承根 target，实际路径：`/Volumes/TBU4/Workspace/Aleph/target/debug/aleph-nsh-spike`
- SHA256: `f00af8e75f4daa7f6fa8982e08081f797ad0d69782f4bc46b2e389d2a88eed5d`
- 二进制含独有字符串：`NSH Spike: compiles OK`（Task 1 继承）、`Aleph NSH Spike`、`com.aleph.nsh-spike`（Task 2 新增）✅

### #[component] 宏在 edition 2021 下的行为

- 生成类型（struct），不是函数——`App()` 调用报 E0618
- 正确用法：`Root::of::<App>()`
- 在 Aleph 的 `edition = "2021"` 下展开正常 ✅

## Task 3: tokio 集成 + Gateway 连接验证

- **状态**: ✅ 真实往返验证通过 — 窗口显示 `aleph-server 26.9.1`
- `cargo build`: exit 0
- `cargo test --locked`: 2 passed, 0 failed
- `cargo clippy --all-targets -- -D warnings`: exit 0
- `cargo fmt --check`: exit 0

### tokio + NSH 线程模型

- NSH AppKit 事件循环在主线程
- tokio runtime 在独立线程上以 `Builder::new_multi_thread().enable_all().build()` 启动
- 后台线程通过 `tokio::sync::oneshot::channel` 把结果送回 NSH 引擎线程
- NSH 的 `spawn` 和 `signal` 完成响应式更新：App 挂载时启动 worker，oneshot 收到后更新 signal，文本随 signal 变化
- **无需全局 Mutex、无需固定 sleep**

### tokio 版本

- spike `Cargo.toml` 声明 `tokio = "1.53"`
- 解析为 `1.53.3`（与 Aleph 根 `Cargo.lock` 一致）
- 单一 tokio 版本，无重复

### GatewayClient API

- `GatewayClient::new()` → 默认 `ws://127.0.0.1:18790/ws`
- `client.call::<serde_json::Value>("system.info", None).await`
- `call` 内部自动先发 `connect` 帧（device_name + channel_kind），再发方法帧
- 服务端注册：`src/gateway/handlers/mod.rs:323` → `system_info::handle`
- 返回 JSON 含 `version` 字段（`system_info.rs:42`，`env!("ALEPH_VERSION")`）
- 访问级别：Open（`method_census.rs:407`）

### 真实往返验证

- 使用 `/Volumes/TBU4/Workspace/Aleph/target/debug/aleph-server`（11 月 2 日构建，759,461,248 字节，`--version=26.9.1`）
- 独立 `ALEPH_HOME`（tmp 目录，已清理），仅监听 127.0.0.1:18790
- server 在 1.64s ready
- spike 窗口显示 `aleph-server 26.9.1` ✅
- 截图：`/tmp/aleph-nsh-task3-qa-20261105/window-real-stable.png`
- 失败路径已验证：无 server 时显示 `error: Connection error: IO error: Connection refused (os error 61)`

### 测试覆盖

1. `worker_delivers_version_even_when_response_takes_longer_than_startup`：夹具延迟 200ms 后返回版本，验证不依赖启动时限
2. `worker_delivers_connection_error_instead_of_remaining_connecting`：连接失败时返回错误，不卡在 "connecting"

### 依赖树

- 单一 tokio 1.53.3 ✅
- NSH 0.4.0 ✅
- 无 QuickJS / nsh-js / rquickjs 节点 ✅
- `shared/client` 的 `native-tls` feature 编译链接通过 ✅
- `cargo metadata --offline` 因未缓存的 `aes` 依赖失败（非本机依赖），改用 `cargo tree` 核验

### Spike 范围排除

- 没有测试事件订阅、Session RPC、重连、Snapshot/Replay（Phase 1 范围）
- 没有测试 WSS/TLS 握手（spike 用 loopback ws://）
- 没有量化覆盖率（cargo-llvm-cov 未安装）
