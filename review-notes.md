# Review: desktop

## Summary
- crate 数:5 (shared / macos / linux / windows / shell)
- 文件总数:171 (rust:129, swift:42)
- 估计 P0 / P1 / P2 数量:0 / 1 / 1
- 整体评估:**健康**

文件分布(去掉 target 编译产物):
- `desktop/shared` 60 .rs
- `desktop/macos` 20 .rs + 41 .swift (含 Tests/)
- `desktop/linux` 15 .rs
- `desktop/windows` 11 .rs
- `desktop/shell` 23 .rs + 1 .swift (图标生成器)

整体观察:这是一个**安全工程实践相当扎实**的代码库。重复出现的几个工程模式——serde_json 兜底所有 JS 字符串注入、PowerShell 用 env 变量传输入、`-LiteralPath` + 精确匹配代替 `Filter`、0600 文件权限、原子写(tmp + rename)、TOFU + SHA-256 指纹 pin、`CallNextHookEx`/`CallNextHook` 显式穿透而不吞键——表明团队对常见陷阱有过反复踩坑的经验,被文档化在注释里。错误的处理面也分得很细:`DesktopError` 有 10+ 个变体覆盖了 PermissionDenied+Guide / BridgeTimeout / BridgeDisabled / BridgeBackoff / NotImplemented / PlatformError 等关键场景,IPC 错误传播没有把字符串 flatten 成 `BridgeFailed` 一锅煮(虽然 `screen.rs::call_input` 里有一处把 `BridgeFailed` 改写成 `InputFailed` 的合理重定向)。

## R1 红线验证

**R1 通过。** R1 的语义是"Core never calls platform APIs; platform impl via IPC"。检查结果:

- `desktop/shared/src/lib.rs:8-12` 明确写明:R1 是 brain-limb separation;`aleph-desktop` 持有 trait contracts + 共享实现;真正平台相关的实现在 `desktop-macos/linux/windows` 里;macOS 的 Swift helper bridge 是 R1 的具体形态。
- `desktop/shared/src/traits/` 完整定义了 8 个 capability trait(`ScreenCapability` / `AccessibilityCapability` / `PermissionCapability` / `PimCapability` / `AutomationCapability` / `SystemCapability` / `MediaCapability` / `PowerCapability`) + `EscapeAbort`,每个 trait 都有 `Send + Sync`、清晰的契约文档、显式的 typed-error(`Result<_, DesktopError>`)。
- macOS limb 在 platform 侧的工作分层干净:
  - 走 Swift bridge 的工作:AX(全部 5 个方法)/ PIM(全部)/ Media(全部)/ screen.capture / screen.list_displays / 全部 input.*(pid-targeted rail) / ScreenCaptureKit 截图 / 权限的 `check_permission`/`guide_permission`/`open_settings`。这是 macOS 上需要私有框架(Accessibility、Vision、ScreenCaptureKit、EventKit、Contacts)的真正平台特化部分。
  - 直接 objc2 / CoreFoundation 的工作:`CGEventSourceSecondsSinceLastEventType`(用户 idle 时间)、`NSWorkspace.runningApplications()`(运行 app 列表)、`NSProcessInfo`(系统信息)、`IOPMAssertion`(sleep inhibit)、TCC 探针(`AVCaptureDevice.authorizationStatusForMediaType` / `SFSpeechRecognizer.authorizationStatus` / `UNUserNotificationCenter` 等)、CGEventTap 全局 Escape 监听。这些是访问模式简单的同步 API,bridge 化只会增加往返成本而无安全收益,留 limb 内是合理的。
- Linux / Windows limb:用 AT-SPI2(D-Bus)/ xcap / enigo / Win32 `windows` crate / ConsentStore 注册表,符合"limb 直接调 OS API"的预期。
- `DesktopPlatform` aggregator trait 通过 `Option<&dyn XCapability>` 显式表达"平台可能不支持"——`None` 是诚实缺失,不会被静默 fallback 成"试试然后报错"。

**IPC 边界完整。** SwiftBridge 客户端(client.rs / supervisor.rs / inflight.rs / codec.rs)是一个自包含的子模块,有:
- 状态机式 supervisor(Backoff + RestartWindow,5/10min 阈值)
- line-delimited JSON codec,显式 newline 终止
- 显式握手协议版本(`BRIDGE_PROTOCOL_VERSION = 2`),版本不匹配直接 fail-fast
- spawn 时 `setpgid(0,0)`,helper 在自己的进程组里
- helper 侧 `ParentWatch`(kqueue `NOTE_EXIT` + 10s poll 兜底)保证父进程死亡时 helper 也退出
- 单 inflight id 序列号,write-failure 时换新 id 重试,避免旧 helper 的 EOF 路径误杀重试的 oneshot
- 全局 `disabled` latch + `SHARED_BRIDGE` OnceLock 共享,多个 platform handle 共享一个 helper 子进程

## Findings

### [P1] desktop/macos/bridge/Sources/AlephBridge/RPC/Handlers.swift:17 — 注释声称不存在的安全属性
注释写道:
```
// enumeration here only exposes the locally-registered IPC surface
// (already gated by the Unix-socket peer-uid + token check), so it
// does not widen the threat model.
```
**实际的 IPC 通道是 stdin/stdout(`Server.swift` 读 `FileHandle.standardInput.fileDescriptor`)**,不是 Unix socket,也没有任何 peer-uid 校验、token 校验、或 allow-list。`Server.swift` / `Codec.swift` / `main.swift` 全套都没有 socket listener,也没有任何 token / uid 处理路径。

**实际的安全模型** 完全不同:helper 由 `SwiftBridge::spawn_process` 通过 `tokio::process::Command` 创建,stdin/stdout/stderr 都是新建的 OS pipe,所有权只属于 aleph-server(parent)和 helper;helper 的安全完全建立在 **OS 强制的 pipe 所有权隔离** + **setpgid** + **ParentWatch**(helper 主动监听父进程死亡)之上,没有协议层的认证。

这是一个 **文档-实现不一致** 的 bug,而不是一个真实漏洞:
- 目前的威胁模型下没有实际的攻击面(没人在 stdin pipe 上跟 helper 通信,除了 parent)
- 但这条注释会误导后续开发者,如果哪天有人为了测试、调试或分布式部署给 helper 加一个 socket listener,他/她会以为"auth 已经在了"而省去必要的 peer 校验,导致 **真实的 auth bypass 漏洞**。

建议修复:要么删掉这句注释,要么把"Unix-socket peer-uid + token check"改成准确的描述——例如 "the helper is exposed only to its parent process via OS-private stdin/stdout pipes, plus setpgid + ParentWatch; no protocol-layer auth exists because no transport layer beyond stdio is reachable"。

### [P2] desktop/shell/src/perm_monitor.rs:128-176 — 权限轮询每次都 spawn 新 bridge 进程
`check_permission` 每 3 秒 (`POLL_INTERVAL = 3s`) 都会 `Command::new("aleph-bridge").args(["perm-check", kind_str]).output()` 起一个新进程,代码注释承认了这一点("spawns a short-lived aleph-bridge process in CLI mode")。

- 安全影响:无(`kind_str` 是 hardcoded 字面量,不接受外部输入)
- 资源影响:每次 fork+exec 一个 Swift 子进程加载 AVFoundation / Speech / UserNotifications frameworks,在 macOS 上冷启动数百 ms,后台常驻每 3s 一次相当于持续进程生成压力
- 设计上更合适的方式:复用 `SHARED_BRIDGE` 共享 client 跑一个 `perm.check` RPC(`AlephPermission` 已经有 `check_permission` trait 方法)

不是阻塞问题,但值得纳入后续优化 backlog。

## 违反红线的情况

**无。** R1-R10 全部合规:
- R1(平台 API 通过 trait):✅ shared 定义 traits,platform crates 实现
- R2(复杂业务 UI 在 Leptos/WASM):✅ shell 仅做 I/O 集成;`main.rs` 显式声明 shell 是 pure I/O 边界
- R3(Core minimalism):N/A(桌面模块非 core)
- R4(Interface 层 pure I/O):✅ shell 是 I/O,无业务逻辑
- R5(Menu bar first,window on demand):✅ shell 有 tray + 隐藏窗口 + `RevealGate` 防 cold-boot flicker
- R6(AI comes to you):N/A
- R7(One core, many shells):✅ 桌面是 limb 之一
- R8/LLM 处理 intent,regex 仅机器格式):✅ JSON 协议解析用 serde/JSONDecoder,无手写 regex
- R9(配置暴露为工具):N/A
- R10(Intelligence lives in the prompt):N/A

## 建议但不阻塞(P3 级)

1. **Linux escape listener 用 sentinel 文件**:任何能写入 `~/.aleph/desktop-abort` 的进程都能 abort desktop control。攻击门槛 = 写用户 home 目录的能力,这个权限下能做的事已经远不止 abort。这是有意的设计选择(`escape_listener.rs` 注释明确解释了 Wayland/X11 没便携全局热键 API),但值得在权限文档里写一句"assume $HOME is owned only by the operator"。

2. **`SHARED_BRIDGE` 是全局 `OnceLock<Arc<SwiftBridge>>`**:在测试/多 tenant 场景里无法替换。当前没有测试场景需要替换,记录即可。

3. **macOS bridge `LineReader` 的 64MB 读缓冲上限**:`Server.swift:60-65` 注释解释了"防御 hostile caller",但实际 stdin 只来自 parent,没有外部 hostile caller 场景。过度防御,但不是错的——保留。

4. **`PermGuide.swift` IOHIDCheckAccess 调用是单向的**:注释提到了 `-1743` 错误码的判断逻辑;值得在 IOHID API 上补一个单元测试覆盖"Input Monitoring 在 sandboxed / non-bundled 二进制下返回 Unknown"的路径,目前看起来只在 main 路径有 happy-path 测试。

5. **`desktop/shared/src/lib.rs:78-84` 的 `WindowInfo::Default` 实现**:让 limb 可以 `..Default::default()` 部分填充,但要求所有 consumer 显式处理 `Option` None 含义("this platform doesn't tell us",不是 "false")。这是一条契约,值得在 module-level docstring 里再加一句明确警告,防止未来有人把 `Option<bool>` 解读成 "false if missing"。

6. **`external_link::route` 的 `data:` URL 拒绝路径**:已实现(`route_rejects_dangerous_pseudo_schemes` 测试覆盖),但 `data:` 的 "no, not internal" 也是写死的——如果未来 Panel 真的需要 data: URL(例如导出报表内嵌图像),需要更细的 origin 判定。当前保守方向正确。

7. **Windows escape listener 把 HHOOK 存成 `isize`**(`escape_listener.rs:51-58`)以保持 `Send + Sync`:有详细的 SAFETY 注释解释为什么这么做是安全的。但 `Drop` 里靠 PostThreadMessageW + join 来保证 ListenerState 在 hook 被 unhook 之后才释放,顺序敏感——值得在 `Drop` 上加一条 invariant 测试(目前 `lifecycle` 测试只覆盖 happy path)。

8. **macOS `screen.rs::is_degenerate` 退化截图 fallback**:对桥返回 degenerate 帧时 fallback 到 `NativeScreen` (xcap),但对 xcap 返回的 degenerate 帧不做二次 fallback。`screenshot_window` 完全没有 fallback(注释解释了对窗口截图没有 fallback 是有意)。逻辑是对的,但"`is_degenerate` 在 bridge 失败和 bridge 成功但退化两种情形下表现不同"这一点值得在 doc comment 里更明确地标记。

9. **`scripts/cargo check -p aleph-desktop` 编译负载很重**:memory note 在 AGENTS.md 已经标注(CARGO_BUILD_JOBS=2 + CARGO_PROFILE_DEV_DEBUG=1)。本次审查纯静态读源码,未触发编译。

10. **macOS bridge 的 `CocoaApp` 启动 / `[NSApplication sharedApplication]` 未在 main.swift 调用**:`InputHandlers` 里 `requireInputTrusted()` 调用 `AXIsProcessTrusted()`,这个函数不需要 NSApplication running;但如果未来加了 NSEvent-based 监控(类似注释提到的旧 macOS 实现),必须自己建 NSApp + run loop。文档里已经写过这个教训(`escape_listener.rs` 顶层 docstring),值得在 main.swift 也交叉引用。

---

**审查完成**:报告路径 `/home/zou/data/workspace/Aleph/.worktrees/review-desktop/review-notes.md`。