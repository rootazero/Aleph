# Panel 连接韧性与错误路由 — 设计文档

> Spec date: 2026-06-24 · Scope: Panel WASM 共用层 + Lite shell 原生恢复 + 网关 token 轮换
> Status: Approved (brainstorming) → 待写 implementation plan

## 1. 背景与问题

继续优化 Panel 的连接加载性能、错误提示、失败提醒与回退机制。重点覆盖远程 server 变更
IP / token、网络故障等场景:找不到服务器、连得上但 token 失效/不匹配,分别引导用户
"检查网络"或"重新输入 token"。

### 现状摸底(已读完连接全链路)

三种形态共用同一份 Panel(WASM):**完整 App**(embedded core,loopback,免 token,恒
operator)、**Lite shell**(纯壳,指向远程 Gateway)、**浏览器**。

关键架构事实(`src/gateway/CLAUDE.md`):**token 不在 WS 握手层校验,而在 `connect` RPC
应用层**。因此两类失败天然走不同码路:

| 失败类型 | 当前路径 | 现状 |
|---------|---------|------|
| 网络/IP 不通 | WS 打不开 → `ConnectFailed` → reconnect 5 次 → `Failed` | 有 Boot/Service Gate 兜底,但只甩英文原始报错 + Retry,无"检查网络"引导 |
| Token 失效/不匹配 | WS 连上 → `connect` RPC 回 `needs_token=true` → 弹 `TokenWall` | 有登录墙,但只在首连触发,运行中轮换衔接生硬 |

### 已识别缺口

1. **错误信息是裸字符串**(`"WebSocket closed: code=1006"`),无分类、无 i18n、无可操作引导。
2. **WS 打开无超时**:半开连接让启动 spinner 无限转(最伤"加载性能感知")。
3. **reconnect 盲试 5 次**(1-16s 退避),且与 `ReconnectStrategy` 结构体重复造轮子,不看原因。
4. **Lite shell 运行中换 IP 无原生监测**:`supervise_daemon` 是 `#[cfg(embedded-core)]`,
   lite 完全没有它;panel 只能对死地址盲重连,ServiceBlockingGate 的 Retry 按钮对死地址无效,
   唯一出路是托盘菜单(不可发现)。远程源 panel **无法调 Tauri 命令、无法 mDNS、无法区分
   自己是 lite 还是浏览器**,故恢复只能由原生侧做。
5. **运行中 token 轮换不踢现有会话**:`reset_token()` 只让旧 token 失效,auth 只在握手时
   校验一次,活动会话一直有效 → 操作者"撤销"对在线远程 panel 无效(安全缺口,非纯 UX)。
6. handshake 收到 `needs_token` 仍返回 `Ok`,导致 `is_connected=true` 却又弹墙的不自洽状态。

## 2. 决策(brainstorming 已确认)

- **范围**:三项全做 —— Panel WASM 共用层 + Lite shell 原生恢复 + 网关 token 轮换推送。
- **重连策略**:按失败类型差异化(AuthRequired 立即停并弹墙;网络类走退避重试)。
- **Lite shell 恢复时机**:先等 panel 重连贩尽,原生再跳转 connect.html(瞬时抖动不扰用户,R5)。
- **中心抽象**:类型化失败分类(方案 A)—— 把"为什么失败"变成一个值,驱动文案/重试/交接。

## 3. 设计

### 3.1 失败模型 + 分类(shared 层,纯函数可测)

新增 `shared/ui_logic/src/connection/failure.rs`:

```rust
pub enum ConnectionFailure {
    Unreachable { detail: String },  // WS 打不开 / TCP 不通 / DNS 失败
    Timeout { detail: String },      // WS 通了但静默 / RPC 超时
    AuthRequired,                    // connect RPC 回 needs_token,或 token_rotated 关闭
    Dropped { detail: String },      // 曾健康后掉线 → 瞬断
    Unknown { detail: String },
}

pub enum FailureStage { BeforeOpen, AfterOpen, Handshake, RpcTimeout }

/// 纯分类:浏览器里 WS 失败几乎都是 code=1006,故主要靠"发生在哪个阶段"
/// + needs_token + 已知 close reason(如 token_rotated / code 4001)。
pub fn classify(stage: FailureStage, close_reason: Option<&str>, needs_token: bool) -> ConnectionFailure;
```

映射表(关键):

| 变体 | 重试策略 | 用户文案/补救 |
|------|---------|--------------|
| `Unreachable` | 指数退避(最多 5 次) | "找不到服务器,请检查网络或服务器地址" + Retry;lite 贩尽后原生跳 connect.html |
| `Timeout` | 退避重试 | "服务器无响应(可能在重启)" + Retry |
| `AuthRequired` | **不重试**,直接弹墙 | TokenWall,清失效旧 token,提示重输 |
| `Dropped` | 静默退避重连 | 仅状态条 "Reconnecting N/5",不打扰 |
| `Unknown` | 退避重试 | 透传 detail + Retry |

`DashboardState` 新增 `connection_failure: RwSignal<Option<ConnectionFailure>>` 作为单一真相源,
现有裸 `connection_error: String` 的显示文案由它经 i18n 派生(不再两处独立赋值);
`ConnectionPhase::Failed` 携类型化原因。

### 3.2 WS 打开超时(性能)

在调用方 `interfaces/webchat/src/context.rs::connect` 用已 import 的 `TimeoutFuture` 给
"打开 WS"封顶(connector 本身不动,符合 P5):

```rust
const WS_OPEN_TIMEOUT_MS: u32 = 8_000;
match select(Box::pin(connector.connect(&url)), TimeoutFuture::new(WS_OPEN_TIMEOUT_MS)).await {
    Either::Left((Ok(()), _))  => { /* 继续 handshake */ }
    Either::Left((Err(e), _))  => { /* classify(BeforeOpen) → Unreachable */ }
    Either::Right(((), _))     => { /* 超时 → Unreachable{detail:"WS open timed out"} */ }
}
```

8s 经验值(局域网 <500ms,公网 <2s)。超时归 `Unreachable`,走"检查网络" + Retry。

### 3.3 差异化 reconnect + handshake 结果区分

**3.3a handshake 返回三态**(消除"已连接却弹墙"):

```rust
enum Handshake { Authorized, NeedsToken, Failed(ConnectionFailure) }
```

`connect()` 分流:
- `Authorized` → happy path(is_connected=true、清错、spawn 订阅)
- `NeedsToken` → **不**置 is_connected、**不** spawn 订阅;`needs_token.set(true)` + 清失效旧 token → TokenWall 接管。归 `AuthRequired`。
- `Failed(f)` → 置 `connection_failure`,走重连/Failed。

**3.3b reconnect 按类型分支**(替换无脑 5 次):
- `AuthRequired` → 不进循环(同一坏 token 重试是浪费),`needs_token.set(true)`,弹墙。
- 其它 → 复用 `reconnect.rs::ReconnectStrategy`(含 `MAX_DELAY_MS=30s` 封顶),**删掉 `context.rs`
  里手写的 `1000*2^n` 退避**(单一真相源,P6),加 ±10% 抖动避免齐刷重连。退避耗尽 → 置
  `Failed`(分类原因)→ Gate 用对应文案。

### 3.4 三个 Gate 的文案与补救(新增 i18n key,中英双语)

- **BootCheckGate**(首连前):`Unreachable`→"无法连接到核心 / 找不到服务器,请检查网络或地址端口" +
  重试;`Timeout`→"服务器无响应 / 可能正在重启,请稍候重试" + 重试。(AuthRequired 由 z-100 的
  TokenWall 自动覆盖。)
- **ServiceBlockingGate**(运行中掉线、重连贩尽后):同套分类文案。**lite shell 远程 + `Unreachable`**
  时文案改"正在为你重新连接服务器…",**去掉对死地址无效的 Retry**(原生即将接管跳转);保留"打开日志"。
  `Timeout`/`Dropped` 保留 Retry。
- **TokenWall**(AuthRequired):①弹墙时清 localStorage 失效旧 token,输入框从空开始;②靠
  `token_was_rejected` 信号区分"首次输入"与"令牌已失效,请重新输入"两种文案。
- `detail_text` 不再直甩 `Failed{reason}` 原始串,改经分类 → i18n,英文 detail 仅 `Unknown` 兜底透传。

### 3.5 Lite-shell 原生 supervisor(换 IP 恢复)

给 lite shell 加 Remote-only 常驻 supervisor,复用现有 `Supervisor` 状态机的 Remote 腿
(`new_remote` / `down_action()→ShowConnectionError` 已存在),探测用 `connect_setup::probe_reachable`
(`daemon::*` 在 lite 被编译掉):

```rust
#[cfg(not(feature = "embedded-core"))]
async fn supervise_remote_lite(handle: AppHandle) {
    let mut sup = Supervisor::new_remote(/*初始可达*/ true);
    loop {
        sleep(HEALTH_POLL_INTERVAL).await;
        let ready = connect_setup::target_reachable(&connection::load_target()).await;
        if let ShowConnectionError = sup.tick(ready) {
            connect_setup::show_lite_connect_page(&handle); // connect.html: mDNS 重发现 + 手填
        }
    }
}
```

**与 panel 重连协调(节奏对齐)**:panel 重连预算 ≈ 1+2+4+8+16 ≈ **31s**;原生宣告 Down 时间 =
`HEALTH_POLL_INTERVAL × FAILURES_TO_DECLARE_DOWN`,**调参让原生 ≳ 35s 才跳**(如 poll 5s × 阈值 7),
给 panel 充分窗口。瞬时抖动(<31s 恢复)→ panel 自连成功、原生看到 ready 回升不跳,不打扰(R5)。
落地后用户重选/重填地址经已有的 `connect_to`(TCP 预探 + 持久化 + reroute)。本节主要是放开 cfg +
spawn lite 循环 + 调参,组件均已存在。

### 3.6 网关 token 轮换主动踢下线(安全边界 ⚠️)

复用现有 broadcast 总线 + `CloseFrame`(不新建 session 注册表):

```
rotate → reset_token() 后发布 `gateway.token.rotated` 信号到事件总线
       → 每连接循环收到后:
           · loopback / 免 token 会话 → 忽略(本机恒 operator,不受影响)
           · token 授权的远程会话 → Close(code=4001, reason="token_rotated")
```

panel `onclose` 已把 code+reason 送进流;`classify` 判 `token_rotated`/4001 → `AuthRequired` →
不重连、清旧 token、弹墙"令牌已更新,请重新输入"。

**红线遵守(`src/gateway/CLAUDE.md`)**:改授权行为**必须同步加测试**(关远程、不关 loopback);
保持最小,不引入新注册表。rotate handler 当前无总线句柄,**如何拿 publisher 是 planning 细节**;
若拿句柄须侵入式改 handler 签名(违 R4),本节降级为**独立后续 PR**,前 5 节不依赖它即可独立交付。

## 4. 测试

**host 纯单测(主力,符合 alephcore build memory-heavy 约束):**

| 测试 | 覆盖 |
|------|------|
| `classify()` 表驱动 | (阶段 × close_reason × needs_token) → 5 变体全覆盖;含 token_rotated / 1006 / 1008 |
| `ConnectionPhase` 携类型化原因 | 扩展现有,Failed 带 `ConnectionFailure` |
| `ReconnectStrategy` 抖动 | 抖动后延迟仍在 [base, MAX_DELAY];AuthRequired 不进循环 |
| `Supervisor` lite remote 腿 | 连续失败阈值 → `ShowConnectionError`;可达回升不误跳 |
| token-wall `token_was_rejected` 推导 | 有持久 token 被拒=失效文案;无=首次文案 |
| **网关轮换(红线必测)** | rotate → 远程收 4001/token_rotated;loopback **不**被关 |

**集成/手动 e2e(reactive 接线 + Tauri 导航,沿用现有 TBD smoke 惯例):**
- 拔网线 → Gate 显"检查网络" + Retry;恢复自动重连
- 远程换 IP → panel 重连贩尽 → 原生跳 connect.html → mDNS 重发现
- 运行中 rotate → 远程 panel 立刻弹墙重输;本机 App 无感

## 5. 受影响文件(预估)

- `shared/ui_logic/src/connection/failure.rs`(新增)、`mod.rs`、`reconnect.rs`(jitter)
- `interfaces/webchat/src/context.rs`(WS 超时、handshake 三态、reconnect 分支、清旧 token)
- `interfaces/webchat/src/state/connection.rs`(ConnectionPhase 携类型化原因)
- `interfaces/webchat/src/components/{boot_check_gate,service_blocking_gate,token_wall}.rs`(文案/补救)
- `interfaces/webchat/src/i18n/*`(新增连接错误 key,中英双语)
- `desktop/shell/src/main.rs`(放开 supervisor cfg + lite remote 循环)、`connect_setup.rs`(复用探测)
- `src/gateway/handlers/gateway_token.rs` + 连接循环(轮换踢下线)+ 授权测试

## 6. 非目标(YAGNI)

- 不引入 per-device 会话 / 新 session 注册表(复用 broadcast)。
- 不为浏览器形态做原生恢复(无原生侧,只能靠 panel 内提示)。
- 不改信任模型(网络边界 + 单层 Gateway token)。
