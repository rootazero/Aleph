# Browser Live View — Plan 1: Managed Launch-Chain Flip Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `BrowserDriver::Managed` 的启动链翻转过来——Aleph 自己 spawn Chromium（`--remote-debugging-port=0`），从 `<user_data_dir>/DevToolsActivePort` 读出 CDP 端点，`playwright-cli` 只以 `attach --cdp <http-url>` 接入（永不 `open`）；浏览器进程的生命周期归 Aleph，`playwright-cli close` 退化成断开，惰性 `open` 变成惰性 `attach`。同时把 Chromium 作为外部运行时供给（配置钉住 > 系统浏览器 > Playwright 自带），补 fail-closed 文案、doctor 哨兵与 R8 安装工具面，并给现有真机装置 `qa/browser_managed/run.sh` 加一个 `attach` 阶段。**本计划不做视图**：只交付 §3.2 的 `live_endpoint(profile)` 访问器，`src/browser/live/`、`qa/browser_live/`、`browser_control` 工具都不在范围内。

**Architecture:** `ChromiumLaunchSpec` → `ChromiumChild`（`std::process::Child` + `CdpEndpoint` + sidecar 文件）住在 `PlaywrightCliDriver` 的 per-session 映射里，与惰性启动咽喉、per-session 锁同处一地；`ProfileManager` 只多三个转发访问器（`live_endpoint` / `session_active` / `reap_idle` 的 Managed 臂）。二进制解析走 `chromium_resolve`：配置钉住 > `discovery::find_chromium_preferred` > 问 `playwright-cli install-browser <b> --dry-run` 要 `Install location:` 再在那一个目录里找可执行文件。运行时供给复用台账**已有的** `playwright-cli` post-install 动作（`install-browser chromium`），只加 `PLAYWRIGHT_DOWNLOAD_HOST` 透传；新增 doctor 哨兵 `browser/chromium-missing` 与 R8 工具 `runtime_manage{list,install}`。

**Tech Stack:** Rust (alephcore) · tokio · serde / schemars · `std::process::Command`（子进程，`NoWindow` 扩展）· `sysinfo`（经 `utils::process_alive::with_process_specifics` 与 `gateway::pty::foreground::fact_for_pid`，唯一 sysinfo 惯用法所有者）· bash + python3（`qa/browser_managed/`）。**零新 crate**。

**Spec:** docs/superpowers/specs/2026-09-05-browser-live-view-design.md (§3.1, §3.2 accessor only, §6.1–§6.5)

## Global Constraints

- MSRV = 1.95；工具链由 `rust-toolchain.toml` 钉住 `1.96.0`；不要 `rustup default`，不要 `cargo +<ver>`。
- 只有 tokio 一个 async runtime；只有 serde 一套序列化栈。
- 进程 spawn 不引新 crate：`std::process::Command` 已经跨平台，`Child::kill` 也是。**禁止**引入 `windows-rs` / `nix` / `libc` 新用法（R1 禁止 `src/` 直接依赖平台 API crate）。
- R10：`src/harness/` 零改动。
- 版本号一律 `env!("ALEPH_VERSION")`，禁止硬编码，禁止 `env!("CARGO_PKG_VERSION")`。
- 格式化只跑 `rustfmt <你改过的那个文件>`；⚠️ `rustfmt <file>` 会**递归进它声明的子模块**。**永远不要**跑 `cargo fmt -p alephcore`（它会重排约 100 个无关文件并扰动 harness 棘轮计数）。
- 最小可信验证集：`cargo test -p alephcore --lib --no-run` · `cargo test -p alephcore --bins` · `cargo test -p alephcore --features test-helpers --test '*' --no-run` · `cargo check -p aleph-desktop-{macos,windows,linux}`（仅当动了桌面 crate；本计划不动）· `just _stage-shell-placeholders` 之后 `cargo clippy --workspace --all-targets`。
- 定向跑测试用 `cargo test -p alephcore --lib browser::` / `--lib runtimes::` / `--lib diagnostics::` / `--lib executor::builtin_registry::`。
- commit message 用英文，格式 `<scope>: <description>`（例 `browser: launch chromium and attach playwright-cli over cdp`），每条 commit message 结尾附两行 trailer：
  ```
  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01TKV5PtutzoBvbT4yTpsyRY
  ```
- 单分支开发，直接在 main（执行者可以自己开 worktree；本计划不创建 worktree）。
- 代码叙述一律写进 `docs/reference/FEATURE_LOCATOR.md` §3.12 的新一轮条目（Task 10），**绝不写进 CLAUDE.md**；另在 `docs/reference/SECURITY.md` 加一段关于「调试口绑 loopback 但无认证」的说明（spec §3.5）。
- QA 需要真 `playwright-cli`（本机 PATH 上是 0.1.8）与真 Chromium；`aleph-server` 必须带假 API key 启动，否则 `tools.invoke` 只答 boot phase 2（`qa/busy_input/patch_config.py` 已经写 `api_key = "qa-dummy-not-a-real-key"`）。每一条断言都要有一个**能变红的控制组**。

---

## 前言：与提议结构的九处偏离（每一处都是代码逼出来的）

1. **`ProfileManager` 构造点不是 `builder/subsystems.rs`**，是 `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:509-513`。而「每个被服务的 manager 恰好跑一次」的 boot 钩子是 `spawn_idle_reaper`，唯一生产调用点在 `src/executor/builtin_registry/builder/constructor/mod.rs:639`。所以 boot 时的孤儿回收挂进 `spawn_idle_reaper`（Task 6），不挂在 builder 里——那一处的论证（manager.rs:202-207）逐字说了为什么它是那个钩子。
2. **macOS 的 Playwright 浏览器缓存不是 `~/.cache/ms-playwright/`**，是 `~/Library/Caches/ms-playwright/`；而且可执行文件不是 `Chromium.app/Contents/MacOS/Chromium`，本机实测是 `chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing`。**所以 `chromium_resolve` 不硬编码缓存路径**，改为问装它的那个 CLI：`playwright-cli install-browser chromium --dry-run` 逐行打印 `Install location:`（本机实测输出逐字抄在 Task 2 里）。同一个二进制既装它又说它在哪，这是判据 §1 要的单一推导；硬编码三条平台路径则是三份会腐烂的表述。
3. **`ChromiumChild` 的映射住在 `PlaywrightCliDriver`，不是 `ProfileManager`。** 惰性启动咽喉在 `playwright_cli.rs:197-221` 的 `run()`，而它已经在 per-session 锁（`:205-206`）下；把子进程映射放到别处等于把这把锁解决掉的竞态重新引进来。`ProfileManager` 保留 `live_endpoint` / `session_active` / `reap_idle` 三个**转发**访问器。
4. **`playwright-cli attach` 接受 `--config`——已实测，不是两个分支。** 本机 0.1.8 的 `attach --help` 逐字列出 `--config  path to the configuration file, defaults to .playwright/cli.config.json`。spec §9 那条未验证项就此关闭，Task 4 只写一个分支。
5. **不新增 `chromium` 台账 capability。** `RuntimeSpec` 的探测是 PATH-only（`src/runtimes/probe.rs:65-83` 只走 `find_on_path`），而 Playwright 的 Chromium 永远不在 PATH 上——加一条 spec 只会得到一个**永远 Missing** 的条目，每次 `ensure_capability` 都重装。而供给本来就已经在了：`src/runtimes/specs.rs:173-176` 的 `PostInstallAction::RunSubcommand { args: &["install-browser", "chromium"] }` 是 `playwright-cli` 的 post-install。所以 Task 7 做的是**给这条已有动作加 `PLAYWRIGHT_DOWNLOAD_HOST` 透传** + doctor 哨兵 + fail-closed 文案，Task 8 做 R8 工具面。这同时回答了 spec §9 的第二条未验证项：**台账已经会装 Chromium**，只是叫 `install-browser` 不叫 `install`（v0.1.14 改的名，specs.rs:166-172 记着）。
6. **提议的 Task 7 拆成两个任务（7 与 8）。** 注册一个新工具要动六处（`definitions.rs` 的条目 + `REGISTRY_SCHEMA_BASELINE` 行 + `standalone` 的 `=> None` 臂、`groups.rs`、`registry/struct_def.rs`、`builder/constructor/mod.rs`、`registry/tool_registry_impl.rs`、`builder/core_tools.rs`）外加一条描述字节棘轮（`CATALOG_DESCRIPTION_CEILING_BYTES = 113_719`）。塞进供给任务里会让「哪一步红了」不可归因。**全计划共 10 个任务。**
7. **三处代码在翻转后成为死码或恒真谓词，本计划 CUT 它们**：`playwright_launch::open_argv`（:220-234）、`playwright_launch::browser_flag_value`（:118-121，`attach` 不收 `--browser`/`--headed`，引擎选择改由 `chromium_resolve` 承担）、`manager::unhonored_managed_fields`（:580-591，它唯一的返回条件是 `browser_flag_value(..).is_none()`，删掉后它恒空——判据 §2「恒真的谓词等于没判」）。连同它们的测试一起删（P6：废弃代码删除不注释）。
8. **`session_active` 与 `idle_managed_profiles` 从 `TabRegistry::has_tabs` 改成子进程活性。** 现状两处都用 `has_tabs`，且 manager.rs:355-357 逐字承认那是「近似」。翻转后「有没有浏览器」有了一个确切答案（我们自己的 `Child`），留着近似就是同一个问题的第二个答案（判据 §1）。`reap_idle_tabs` 的候选筛选**继续**用 `has_tabs`——那问的是 tab 不是浏览器。
9. **现有 `open` 场景的预言机会失效，Task 9 负责搬家。** `qa/browser_managed/drive_browser.py` 用 `playwright-cli list` 打印的 `user-data-dir` 证明「Aleph 生成的 `--config` 真的到了浏览器」。`attach` 之后 CLI 不再拥有 profile 目录，那一行不可能再报出我们的 udd。新的预言机是 `<udd>/DevToolsActivePort` 存在 + `curl http://127.0.0.1:<port>/json/version` 200——**更强**，因为它证明的是浏览器确实被我们用那个 udd 起起来了，而不是 CLI 转述了一遍我们写给它的配置。

---

### Task 1: `ChromiumLaunchSpec` / `ChromiumChild` / `CdpEndpoint` —— Aleph 自己起 Chromium

**Files:**
- Create `src/browser/chromium_launch.rs`
- Modify `src/browser/mod.rs:1-21`（模块声明；现有 21 行全部读过）
- Modify `src/browser/error.rs:5-6`（`LaunchFailed(String)` → 带 `stage` 的结构变体）与它现存的四个构造点 `src/browser/chrome_mcp.rs:135, 578, 603, 609`、一个测试点 `src/diagnostics/checks/browser_runtime.rs:480`

**Interfaces:**
- Consumes: `crate::utils::no_window::NoWindow`（`src/utils/no_window.rs:32-51`，`std::process::Command` 与 `tokio::process::Command` 都实现了）· `crate::security::secret_env::is_secret_env`（`playwright_cli.rs:19` 已在用）· `crate::utils::process_alive::with_process_specifics`（`src/utils/process_alive.rs:126-131`，`pub(crate)`）· `crate::gateway::pty::foreground::fact_for_pid`（`src/gateway/pty/foreground.rs:263`，`pub`，返回带 `cmdline: Option<String>` 的 `ForegroundFact`）
- Produces:
  ```rust
  pub(crate) const DEVTOOLS_PORT_DEADLINE: std::time::Duration;
  pub(crate) const SIDECAR_FILE: &str;                       // "aleph-chromium.json"
  pub(crate) struct ChromiumLaunchSpec { pub binary: PathBuf, pub user_data_dir: PathBuf,
                                          pub headless: bool, pub proxy: Option<String>,
                                          pub extra_args: Vec<String> }
  impl ChromiumLaunchSpec { pub(crate) fn argv(&self) -> Vec<String>; }
  pub(crate) struct CdpEndpoint { pub http_url: String, pub ws_url: String, pub pid: u32 }
  pub(crate) fn parse_devtools_active_port(text: &str) -> Option<(u16, String)>;
  pub(crate) fn endpoint_from_port_file(text: &str, pid: u32) -> Option<CdpEndpoint>;
  pub(crate) struct ChromiumSidecar { pub pid: u32, pub http_url: String, pub aleph_version: String }
  pub(crate) struct ChromiumChild;
  impl ChromiumChild {
      pub(crate) async fn spawn(spec: &ChromiumLaunchSpec, deadline: Duration) -> Result<Self, BrowserError>;
      pub(crate) const fn endpoint(&self) -> &CdpEndpoint;
      pub(crate) fn alive(&mut self) -> bool;
      pub(crate) fn shutdown(self);
  }
  pub(crate) fn reap_orphans(root: &Path,
                             argv_of: &dyn Fn(u32) -> Option<String>,
                             kill: &dyn Fn(u32)) -> usize;
  pub(crate) fn reap_orphans_now(root: &Path) -> usize;
  // error.rs
  BrowserError::LaunchFailed { stage: &'static str, detail: String }
  ```

#### Steps

- [ ] **写失败测试。** 新建 `src/browser/chromium_launch.rs`，先只放 `#[cfg(test)] mod tests`（测试引用的项还不存在，所以这一步必然编译失败——这就是 RED）。测试全文：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ChromiumLaunchSpec {
        ChromiumLaunchSpec {
            binary: PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            user_data_dir: PathBuf::from("/tmp/udd"),
            headless: true,
            proxy: Some("socks5://127.0.0.1:1080".into()),
            extra_args: vec!["--disable-gpu".into()],
        }
    }

    /// Golden vector. The ORDER is the contract, not decoration: Chrome takes
    /// the LAST occurrence of a duplicated switch, which is exactly how
    /// Playwright's own `--remote-debugging-port` beat a user-supplied `=0` in
    /// the spike. So the operator's `extra_args` go FIRST and every switch this
    /// launch depends on goes after them, where a duplicate cannot displace it.
    #[test]
    fn argv_puts_the_contract_switches_after_the_operator_args() {
        assert_eq!(
            spec().argv(),
            vec![
                "--disable-gpu",
                "--no-first-run",
                "--no-default-browser-check",
                "--headless=new",
                "--proxy-server=socks5://127.0.0.1:1080",
                "--user-data-dir=/tmp/udd",
                "--remote-debugging-port=0",
                "about:blank",
            ]
        );
    }

    #[test]
    fn a_headed_launch_omits_the_headless_switch_and_a_proxyless_one_the_proxy() {
        let argv = ChromiumLaunchSpec {
            headless: false,
            proxy: None,
            extra_args: Vec::new(),
            ..spec()
        }
        .argv();
        assert!(!argv.iter().any(|a| a.starts_with("--headless")));
        assert!(!argv.iter().any(|a| a.starts_with("--proxy-server")));
        assert_eq!(argv.last().map(String::as_str), Some("about:blank"));
        assert!(argv.contains(&"--remote-debugging-port=0".to_string()));
    }

    /// The real two-line file, verbatim from the Chrome spike
    /// (`docs/superpowers/specs/2026-09-05-browser-live-view-evidence/chrome-spike-findings.md`
    /// STEP 1): a port on line 1, the browser path on line 2.
    #[test]
    fn the_real_port_file_parses_into_a_port_and_a_browser_path() {
        let text = "58363\n/devtools/browser/ac5f508a-1111-2222-3333-444455556666\n";
        assert_eq!(
            parse_devtools_active_port(text),
            Some((
                58363,
                "/devtools/browser/ac5f508a-1111-2222-3333-444455556666".to_string()
            ))
        );
        let ep = endpoint_from_port_file(text, 4242).expect("endpoint");
        assert_eq!(ep.http_url, "http://127.0.0.1:58363");
        assert_eq!(
            ep.ws_url,
            "ws://127.0.0.1:58363/devtools/browser/ac5f508a-1111-2222-3333-444455556666"
        );
        assert_eq!(ep.pid, 4242);
    }

    /// A half-written file is the NORMAL state during the poll — Chrome creates
    /// it and fills it in. Every partial shape must read as "not yet", never as
    /// an endpoint: `Option::None` here is the "I do not know yet" answer the
    /// poll loop is allowed to spend, and a `Some` built from half a file would
    /// hand `attach --cdp` a URL that cannot connect.
    #[test]
    fn every_partial_or_malformed_port_file_reads_as_not_yet() {
        for bad in [
            "",
            "\n",
            "58363",                      // port written, path not yet
            "58363\n",                    // ditto, with the newline
            "58363\ndevtools/browser/x",  // path must be absolute
            "notaport\n/devtools/browser/x",
            "0\n/devtools/browser/x",     // port 0 is never a listening port
            "99999999\n/devtools/browser/x",
        ] {
            assert_eq!(parse_devtools_active_port(bad), None, "accepted {bad:?}");
            assert!(endpoint_from_port_file(bad, 1).is_none(), "accepted {bad:?}");
        }
    }

    #[test]
    fn the_sidecar_round_trips_and_carries_the_running_version() {
        let json = serde_json::to_string(&ChromiumSidecar {
            pid: 4242,
            http_url: "http://127.0.0.1:58363".into(),
            aleph_version: env!("ALEPH_VERSION").to_string(),
        })
        .expect("serialize");
        let back: ChromiumSidecar = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.pid, 4242);
        assert_eq!(back.http_url, "http://127.0.0.1:58363");
        assert_eq!(back.aleph_version, env!("ALEPH_VERSION"));
    }

    /// The whole point of reading argv before killing: a pid recorded hours ago
    /// may belong to somebody else's process now. "The sidecar named this pid"
    /// is not evidence; "the process still carries OUR user-data-dir" is.
    #[test]
    fn reap_orphans_kills_the_match_and_spares_a_recycled_pid() {
        let root = tempfile::tempdir().expect("tempdir");
        let ours = root.path().join("default");
        let recycled = root.path().join("other");
        for (dir, pid) in [(&ours, 111_u32), (&recycled, 222_u32)] {
            std::fs::create_dir_all(dir).expect("mkdir");
            std::fs::write(
                dir.join(SIDECAR_FILE),
                serde_json::to_string(&ChromiumSidecar {
                    pid,
                    http_url: "http://127.0.0.1:1".into(),
                    aleph_version: env!("ALEPH_VERSION").to_string(),
                })
                .expect("serialize"),
            )
            .expect("write");
        }
        let ours_flag = format!("--user-data-dir={}", ours.display());
        let killed = std::cell::RefCell::new(Vec::new());
        let n = reap_orphans(
            root.path(),
            &|pid| match pid {
                111 => Some(format!("/x/chrome {ours_flag} --headless=new")),
                // A recycled pid: alive, but it is somebody else's process.
                222 => Some("/usr/bin/vim notes.txt".to_string()),
                _ => None,
            },
            &|pid| killed.borrow_mut().push(pid),
        );
        assert_eq!(n, 1, "exactly the matching pid is reaped");
        assert_eq!(*killed.borrow(), vec![111]);
        // Both sidecars are cleared: a stale one that survives would be read
        // again on every boot forever.
        assert!(!ours.join(SIDECAR_FILE).exists());
        assert!(!recycled.join(SIDECAR_FILE).exists());
    }

    /// A dead pid answers `None`, which means "no such process" — nothing to
    /// kill, and the sidecar is stale. It must not be spent as "it matched".
    #[test]
    fn reap_orphans_treats_an_unreadable_process_as_nothing_to_kill() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("default");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join(SIDECAR_FILE),
            serde_json::to_string(&ChromiumSidecar {
                pid: 333,
                http_url: "http://127.0.0.1:1".into(),
                aleph_version: env!("ALEPH_VERSION").to_string(),
            })
            .expect("serialize"),
        )
        .expect("write");
        let killed = std::cell::RefCell::new(Vec::new());
        let n = reap_orphans(root.path(), &|_| None, &|pid| killed.borrow_mut().push(pid));
        assert_eq!(n, 0);
        assert!(killed.borrow().is_empty());
        assert!(!dir.join(SIDECAR_FILE).exists());
    }
}
```

- [ ] **跑它，看红。** `cargo test -p alephcore --lib browser::chromium_launch` → 期望 `error[E0433]: failed to resolve: use of undeclared type ChromiumLaunchSpec`（以及同类的 `parse_devtools_active_port` / `reap_orphans` / `SIDECAR_FILE` 未解析）。⚠️ 在把 `pub(crate) mod chromium_launch;` 加进 `src/browser/mod.rs` 之前，这个文件根本不参与编译，`cargo test` 会直接绿——所以**先**在 `src/browser/mod.rs` 第 3 行之后（按字母序）插入一行：
  ```rust
  pub(crate) mod chromium_launch;
  ```
  再跑一次，红才有意义。这正是判据 §3「没有 `mod` 声明的测试文件」那一条。

- [ ] **改 `src/browser/error.rs`：给 `LaunchFailed` 加 `stage`。** 把 `error.rs:5-6` 的
  ```rust
      #[error("Failed to launch browser: {0}")]
      LaunchFailed(String),
  ```
  改成
  ```rust
      /// A launch that did not reach a usable browser, tagged with **which
      /// step** failed. The stage is not decoration: "the binary would not
      /// spawn", "the process died before it published a port" and "the port
      /// file never appeared" are three different operator problems, and a
      /// single opaque string made the tool answer identical for all three.
      /// `stage` values in use: `"spawn"`, `"chromium-exit"`, `"devtools-port"`
      /// (this module's Chromium launch) and `"chrome-mcp"` (the existing-session
      /// driver's Chrome launch).
      #[error("Failed to launch browser at stage '{stage}': {detail}")]
      LaunchFailed { stage: &'static str, detail: String },
  ```
  然后逐个改它的五个引用点（`grep -rn "LaunchFailed" src/` 逐字核对过，只有这些）：
  - `src/browser/chrome_mcp.rs:135` / `:578` / `:603` / `:609` → `BrowserError::LaunchFailed { stage: "chrome-mcp", detail: format!(...) }`（把原来的 `format!` 参数原样搬进 `detail`）。
  - `src/diagnostics/checks/browser_runtime.rs:480` → `BrowserError::LaunchFailed { stage: "chrome-mcp", detail: "io".into() }`。
  另外，`error.rs:20-21` 的 `ChromiumNotFound` 保持原样（它是 `discovery::find_chromium` 的「系统上没有 Chromium 系浏览器」，doctor 的 `classify_chromium` 按变体名匹配它，见 `browser_runtime.rs:81-89`），在它下面新增一个 managed 专用变体：
  ```rust
      /// The **managed** driver has no browser to launch: the pin (if any) is
      /// gone, no system Chromium-family browser was found, and Playwright's own
      /// Chromium is not installed either. Distinct from [`Self::ChromiumNotFound`]
      /// on purpose — that one answers "is there a system browser?", which the
      /// doctor and the existing-session driver ask and this driver does not.
      /// The message names the command that fixes it, because a fail-closed
      /// answer that does not say how to open the gate is fail-dead (判据 §14).
      #[error("No Chromium for the managed browser driver ({tried}). \
               Run `playwright-cli install-browser chromium`, ask me to run \
               `runtime_manage{{action:\"install\", capability:\"chromium\"}}`, \
               or pin one with [browser.runtime] binary_path.")]
      ChromiumUnavailable { tried: String },
  ```

- [ ] **最小实现。** 在 `src/browser/chromium_launch.rs` 的 `#[cfg(test)] mod tests` **之前**写入：

```rust
//! Aleph launches Chromium; `playwright-cli` only attaches to what it finds.
//!
//! Why this module exists at all is a measurement, not a preference. The Chrome
//! spike (`docs/superpowers/specs/2026-09-05-browser-live-view-evidence/`)
//! established that a CLI-launched Chrome *does* open a debug port — and that
//! the port is useless as a contract: it is random per launch, a user-supplied
//! `--remote-debugging-port` loses to Playwright's own (Chrome takes the last
//! occurrence), no `DevToolsActivePort` file is written into Playwright's
//! profile dir, and `playwright-cli list` prints no endpoint. The only
//! discovery route left was scraping `ps`. Launching it ourselves replaces all
//! of that with a file Chrome writes on purpose.
//!
//! The second consequence is ownership: under `attach --cdp`, `playwright-cli
//! close` disconnects and leaves the browser running (measured: 9 Chrome
//! processes before and after, endpoint still serving, page still on its URL).
//! So the browser's life is ours to end, which is what [`ChromiumChild`] and
//! [`reap_orphans`] are for.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::utils::no_window::NoWindow;

use super::error::BrowserError;

/// How long the `DevToolsActivePort` file may take to appear before the launch
/// is called failed.
///
/// A cold Chrome on a loaded machine is slow, and the spike never measured this
/// window (it read the file after the fact) — so the number is chosen to match
/// the repo's existing answer to "how long may bringing up a browser take":
/// `playwright_cli::SESSION_START_TIMEOUT_SECS` and `chrome_mcp`'s
/// `create_session` both say 60 s. Half of it is the budget for the *port*,
/// which appears well before the browser is usable.
pub(crate) const DEVTOOLS_PORT_DEADLINE: Duration = Duration::from_secs(30);

/// How often the port file is polled while waiting.
const PORT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Chrome's own file, written into the user-data-dir. Name fixed by Chrome.
const DEVTOOLS_PORT_FILE: &str = "DevToolsActivePort";

/// Our sidecar next to it: what Aleph needs to recognise its own orphan after a
/// crash. Chrome does not remove `DevToolsActivePort` on exit, so that file
/// cannot answer "is this browser mine and still running".
pub(crate) const SIDECAR_FILE: &str = "aleph-chromium.json";

/// Everything the Chromium process needs at launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChromiumLaunchSpec {
    pub binary: PathBuf,
    pub user_data_dir: PathBuf,
    pub headless: bool,
    pub proxy: Option<String>,
    pub extra_args: Vec<String>,
}

impl ChromiumLaunchSpec {
    /// The full argv, operator args first.
    ///
    /// Order is the contract. Chrome resolves a duplicated switch to its LAST
    /// occurrence — that is precisely how Playwright's own
    /// `--remote-debugging-port=58419` beat a caller-supplied `=0` in the spike.
    /// So `extra_args` lead and every switch this launch depends on follows
    /// them, where an operator's duplicate cannot displace it. The URL is last
    /// because it is positional.
    pub(crate) fn argv(&self) -> Vec<String> {
        let mut argv = self.extra_args.clone();
        argv.push("--no-first-run".to_string());
        argv.push("--no-default-browser-check".to_string());
        if self.headless {
            argv.push("--headless=new".to_string());
        }
        if let Some(proxy) = &self.proxy {
            argv.push(format!("--proxy-server={proxy}"));
        }
        argv.push(format!("--user-data-dir={}", self.user_data_dir.display()));
        argv.push("--remote-debugging-port=0".to_string());
        // `about:blank` keeps the launch out of the SSRF guard's way; the
        // caller navigates afterwards through the guarded path. Same reasoning
        // the deleted `open_argv` carried.
        argv.push("about:blank".to_string());
        argv
    }
}

/// A live CDP endpoint on loopback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CdpEndpoint {
    /// `http://127.0.0.1:<port>` — the form `playwright-cli attach --cdp` takes
    /// (both this and the ws form were accepted in the spike; the http one is
    /// shorter and does not embed a browser id that changes per launch).
    pub http_url: String,
    /// `ws://127.0.0.1:<port>/devtools/browser/<id>` — what a raw CDP client
    /// (the live view, Plan 2) connects to.
    pub ws_url: String,
    /// The Chromium process we launched.
    pub pid: u32,
}

/// Parse Chrome's two-line `DevToolsActivePort`: the port, then the browser
/// websocket path.
///
/// Returns `None` for every shape that is not both lines — which is the normal
/// state while Chrome is still writing the file. That `None` means "not yet",
/// and the poll loop is the only thing allowed to spend it; nothing may read it
/// as "failed" (判据 §8).
pub(crate) fn parse_devtools_active_port(text: &str) -> Option<(u16, String)> {
    let mut lines = text.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    if port == 0 {
        return None;
    }
    let path = lines.next()?.trim();
    if !path.starts_with('/') {
        return None;
    }
    Some((port, path.to_string()))
}

/// [`parse_devtools_active_port`] plus the pid, as one endpoint.
pub(crate) fn endpoint_from_port_file(text: &str, pid: u32) -> Option<CdpEndpoint> {
    let (port, path) = parse_devtools_active_port(text)?;
    Some(CdpEndpoint {
        http_url: format!("http://127.0.0.1:{port}"),
        ws_url: format!("ws://127.0.0.1:{port}{path}"),
        pid,
    })
}

/// What Aleph writes beside the port file so it can recognise its own orphan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChromiumSidecar {
    pub pid: u32,
    pub http_url: String,
    /// The build that launched it. Not used as a gate — recorded because an
    /// orphan from a different version is exactly the case a reader will want
    /// named when this goes wrong.
    pub aleph_version: String,
}

/// Where the sidecar lives for a given user-data-dir.
pub(crate) fn sidecar_path(user_data_dir: &Path) -> PathBuf {
    user_data_dir.join(SIDECAR_FILE)
}

/// One Chromium process owned by this Aleph.
pub(crate) struct ChromiumChild {
    child: Child,
    endpoint: CdpEndpoint,
    user_data_dir: PathBuf,
}

impl ChromiumChild {
    /// Launch Chromium and wait for it to publish its debug port.
    pub(crate) async fn spawn(
        spec: &ChromiumLaunchSpec,
        deadline: Duration,
    ) -> Result<Self, BrowserError> {
        tokio::fs::create_dir_all(&spec.user_data_dir)
            .await
            .map_err(|e| BrowserError::LaunchFailed {
                stage: "spawn",
                detail: format!(
                    "cannot create the chromium user-data-dir {}: {e}",
                    spec.user_data_dir.display()
                ),
            })?;
        // A leftover file from the PREVIOUS launch would be read as this one's
        // endpoint — a port that is either closed or, worse, somebody else's.
        let port_file = spec.user_data_dir.join(DEVTOOLS_PORT_FILE);
        let _ = tokio::fs::remove_file(&port_file).await;

        let mut cmd = Command::new(&spec.binary);
        cmd.args(spec.argv())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Same discipline as the CLI child (`playwright_cli::spawn`): the
        // browser never needs the parent's credentials, and over-stripping is
        // safe.
        for (name, _) in std::env::vars() {
            if crate::security::secret_env::is_secret_env(&name) {
                cmd.env_remove(&name);
            }
        }
        let mut child = cmd.no_window().spawn().map_err(|e| BrowserError::LaunchFailed {
            stage: "spawn",
            detail: format!("{}: {e}", spec.binary.display()),
        })?;
        let pid = child.id();

        let started = Instant::now();
        loop {
            if let Ok(text) = tokio::fs::read_to_string(&port_file).await {
                if let Some(endpoint) = endpoint_from_port_file(&text, pid) {
                    let me = Self {
                        child,
                        endpoint,
                        user_data_dir: spec.user_data_dir.clone(),
                    };
                    me.write_sidecar().await;
                    tracing::info!(pid, endpoint = %me.endpoint.http_url, "chromium launched");
                    return Ok(me);
                }
            }
            // Chrome died before publishing: a different fact from "the file is
            // late", and the operator's fix is different too (a missing shared
            // library, a bad `--user-data-dir`, a crashed sandbox).
            if let Ok(Some(status)) = child.try_wait() {
                return Err(BrowserError::LaunchFailed {
                    stage: "chromium-exit",
                    detail: format!(
                        "{} exited with {status} before writing {DEVTOOLS_PORT_FILE}",
                        spec.binary.display()
                    ),
                });
            }
            if started.elapsed() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(BrowserError::LaunchFailed {
                    stage: "devtools-port",
                    detail: format!(
                        "no {DEVTOOLS_PORT_FILE} under {} after {}s",
                        spec.user_data_dir.display(),
                        deadline.as_secs()
                    ),
                });
            }
            tokio::time::sleep(PORT_POLL_INTERVAL).await;
        }
    }

    pub(crate) const fn endpoint(&self) -> &CdpEndpoint {
        &self.endpoint
    }

    /// Whether the browser is still running.
    ///
    /// `Err` from `try_wait` is answered **`true`**, deliberately. "I could not
    /// tell" is not "it is dead", and killing on an unknown would orphan a live
    /// browser. The attach that follows settles it for free: a dead endpoint
    /// answers `ECONNREFUSED` and the driver's retry path forgets the child.
    pub(crate) fn alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) | Err(_) => true,
            Ok(Some(_)) => false,
        }
    }

    /// Kill the browser and clear the sidecar.
    pub(crate) fn shutdown(mut self) {
        let pid = self.endpoint.pid;
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(sidecar_path(&self.user_data_dir));
        tracing::info!(pid, "chromium shut down");
    }

    async fn write_sidecar(&self) {
        let body = match serde_json::to_string(&ChromiumSidecar {
            pid: self.endpoint.pid,
            http_url: self.endpoint.http_url.clone(),
            aleph_version: env!("ALEPH_VERSION").to_string(),
        }) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "cannot serialize the chromium sidecar");
                return;
            }
        };
        if let Err(e) = tokio::fs::write(sidecar_path(&self.user_data_dir), body).await {
            // Best-effort: a missing sidecar costs an orphan across a crash, it
            // does not break this launch. Saying so is the point — a silent
            // failure here is invisible until the next boot leaks a browser.
            tracing::warn!(error = %e, "cannot write the chromium sidecar");
        }
    }
}

/// Kill Chromium processes left behind by a previous Aleph.
///
/// `root` is the parent of the per-profile user-data-dirs. For each sidecar
/// found, the recorded pid is killed **only if the process still carries our
/// `--user-data-dir`** — a pid recorded before a crash may belong to somebody
/// else's program now, and "the sidecar named this pid" is not evidence that
/// the process is ours. The sidecar is removed either way: a stale one that
/// survives would be re-read on every boot forever.
///
/// The two effects are injected so the decision is testable without a browser;
/// [`reap_orphans_now`] is the production wiring.
pub(crate) fn reap_orphans(
    root: &Path,
    argv_of: &dyn Fn(u32) -> Option<String>,
    kill: &dyn Fn(u32),
) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        // The dir not existing is the normal first-boot state, not a failure.
        return 0;
    };
    let mut reaped = 0;
    for entry in entries.flatten() {
        let dir = entry.path();
        let sidecar = sidecar_path(&dir);
        let Ok(body) = std::fs::read_to_string(&sidecar) else {
            continue;
        };
        if let Ok(rec) = serde_json::from_str::<ChromiumSidecar>(&body) {
            let flag = format!("--user-data-dir={}", dir.display());
            if argv_of(rec.pid).is_some_and(|argv| argv.contains(&flag)) {
                tracing::info!(pid = rec.pid, dir = %dir.display(), "reaping orphaned chromium");
                kill(rec.pid);
                reaped += 1;
            }
        }
        let _ = std::fs::remove_file(&sidecar);
    }
    reaped
}

/// [`reap_orphans`] wired to the real process table.
///
/// Reads argv through `gateway::pty::foreground::fact_for_pid`, the one place
/// in this repo that owns the single-pid `sysinfo` idiom — a second copy of
/// that dance would be the same fact written twice, and the two would drift on
/// which fields get refreshed.
///
/// ⚠️ On macOS `sysinfo` can bleed environment variables into the reported
/// argv. The predicate here is *containment of `--user-data-dir=<dir>`*, and a
/// bleed can only ADD text, so the failure mode would need an env var holding
/// that exact flag-and-value string. Recorded rather than assumed away.
pub(crate) fn reap_orphans_now(root: &Path) -> usize {
    reap_orphans(
        root,
        &|pid| crate::gateway::pty::foreground::fact_for_pid(pid).and_then(|f| f.cmdline),
        &|pid| {
            let killed = crate::utils::process_alive::with_process_specifics(
                pid,
                sysinfo::ProcessRefreshKind::nothing(),
                sysinfo::Process::kill,
            );
            if killed != Some(true) {
                tracing::warn!(pid, "orphaned chromium did not accept the kill");
            }
        },
    )
}
```

- [ ] **跑到绿。** `cargo test -p alephcore --lib browser::chromium_launch` → 9 个测试全过。若 `sysinfo::Process::kill` 的签名在 0.39 上不是 `fn kill(&self) -> bool`，按编译器提示改闭包（`|p| p.kill()`），不要改判据。
- [ ] **证伪一次守卫。** 把 `parse_devtools_active_port` 里 `if !path.starts_with('/') { return None; }` 注释掉，重跑 → `every_partial_or_malformed_port_file_reads_as_not_yet` 必须变红（`58363\ndevtools/browser/x` 那一条）。再把 `reap_orphans` 里的 `argv_of(...).is_some_and(...)` 改成 `true`，重跑 → `reap_orphans_kills_the_match_and_spares_a_recycled_pid` 必须变红。两次都确认之后**恢复原样**。
- [ ] `rustfmt src/browser/chromium_launch.rs src/browser/error.rs src/browser/chrome_mcp.rs src/diagnostics/checks/browser_runtime.rs`
- [ ] `cargo test -p alephcore --lib browser:: diagnostics::checks::browser_runtime` 全绿。
- [ ] **提交。**
  ```
  git add src/browser/chromium_launch.rs src/browser/mod.rs src/browser/error.rs \
          src/browser/chrome_mcp.rs src/diagnostics/checks/browser_runtime.rs
  git commit -m "browser: launch chromium and read its DevToolsActivePort

  Aleph spawns Chromium with --remote-debugging-port=0, polls the port file
  with a deadline, and writes a sidecar so a crashed daemon's orphan can be
  recognised by argv rather than by pid alone. LaunchFailed gains a stage so
  \"would not spawn\", \"died early\" and \"no port file\" stop reading alike.

  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01TKV5PtutzoBvbT4yTpsyRY"
  ```

---

### Task 2: `[browser.runtime]` 配置段（`BrowserRuntimeConfig`）

> **顺序偏离**：提议里这是 Task 3、`chromium_resolve` 是 Task 2。对调了，因为解析器**消费**这个类型——反过来写会让 Task 2 先编不过再补回来，红的原因就不是它自己的了。

**Files:**
- Modify `src/browser/profile.rs`：在 `PlaywrightCliConfig`（:147-164）之后、`ChromeMcpConfig`（:186）之前插入新类型；在 `BrowserSystemConfig`（:244-261）里加一个字段；扩充 `tests::test_browser_system_config_toml_deserialization`（:279-330）

**Interfaces:**
- Consumes: `super::network_policy::default_true`（经 `profile.rs:141-143` 的本地包装 `const fn default_true()`）
- Produces:
  ```rust
  pub struct BrowserRuntimeConfig {
      pub binary_path: Option<String>,
      pub prefer_system_browser: bool,   // default true
      pub download_host: Option<String>,
  }
  impl BrowserRuntimeConfig {
      pub fn pinned_binary(&self) -> Option<&str>;
      pub fn download_host(&self) -> Option<&str>;
  }
  // BrowserSystemConfig gains: pub runtime: BrowserRuntimeConfig
  ```

#### Steps

- [ ] **写失败测试。** 在 `src/browser/profile.rs` 的 `mod tests` 里追加：

```rust
    /// The three `[browser.runtime]` keys, and the one property that matters
    /// about all of them: an EMPTY string is not a value.
    ///
    /// `download_host = ""` is what the spec's own config sample shows, and
    /// what a Panel form posts when the operator clears the field. Handing that
    /// to the installer as `PLAYWRIGHT_DOWNLOAD_HOST=` is not "no mirror", it is
    /// "the mirror is the empty host" — every download then fails with a URL
    /// error that names nothing. Same for a `binary_path` cleared to "".
    #[test]
    fn browser_runtime_reads_its_three_keys_and_treats_empty_as_unset() {
        let cfg: BrowserSystemConfig = toml::from_str(
            r#"
[runtime]
binary_path = "/opt/chromium/chrome"
prefer_system_browser = false
download_host = "https://npmmirror.com/mirrors/playwright"
"#,
        )
        .expect("parse");
        assert_eq!(cfg.runtime.pinned_binary(), Some("/opt/chromium/chrome"));
        assert!(!cfg.runtime.prefer_system_browser);
        assert_eq!(
            cfg.runtime.download_host(),
            Some("https://npmmirror.com/mirrors/playwright")
        );

        let cleared: BrowserSystemConfig = toml::from_str(
            r#"
[runtime]
binary_path = ""
download_host = "   "
"#,
        )
        .expect("parse");
        assert_eq!(cleared.runtime.pinned_binary(), None, "empty pin is unset");
        assert_eq!(cleared.runtime.download_host(), None, "blank host is unset");
        assert!(
            cleared.runtime.prefer_system_browser,
            "a system browser is preferred unless the operator says otherwise: \
             Windows almost always has Edge and macOS usually has Chrome, so the \
             download is for clean Linux servers"
        );
    }

    /// A config with no `[runtime]` table at all must still produce the
    /// defaults — this section is new, and every config file on every existing
    /// install predates it.
    #[test]
    fn a_config_without_the_runtime_table_still_gets_the_defaults() {
        let cfg: BrowserSystemConfig = toml::from_str("[policy]\nblock_private = true\n")
            .expect("parse");
        assert!(cfg.runtime.prefer_system_browser);
        assert_eq!(cfg.runtime.pinned_binary(), None);
        assert_eq!(cfg.runtime.download_host(), None);
        assert_eq!(
            BrowserRuntimeConfig::default().prefer_system_browser,
            cfg.runtime.prefer_system_browser,
            "serde's default and Default::default must agree — two answers to \
             one question is how a default drifts"
        );
    }
```

- [ ] **跑它，看红。** `cargo test -p alephcore --lib browser::profile` → 期望 `error[E0433]: failed to resolve: use of undeclared type BrowserRuntimeConfig` 与 `error[E0609]: no field runtime on type BrowserSystemConfig`。

- [ ] **最小实现。** 在 `src/browser/profile.rs` 的 `PlaywrightCliConfig` 的 `impl Default`（:173-182）之后插入：

```rust
/// External-runtime settings for the managed driver's browser.
///
/// Chromium is deliberately NOT in any Aleph installer (D4): all three
/// artifacts stay Chromium-free and the browser is supplied at runtime, the
/// same way `playwright-cli` already is.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserRuntimeConfig {
    /// Absolute path to a Chromium-family binary, pinned by the operator.
    /// Highest precedence — a pin that does not exist is a hard failure, not a
    /// fallback, because silently launching a different browser than the one
    /// named is worse than refusing.
    #[serde(default)]
    pub binary_path: Option<String>,

    /// Use a system-installed Chromium-family browser (via
    /// `discovery::find_chromium_preferred`) before Playwright's own.
    ///
    /// Default `true`: Windows almost always has Edge and macOS usually has
    /// Chrome, so the ~150 MB download is only for a clean Linux host. The
    /// Chrome spike ran system Chrome 152 against playwright-core 1.60 with no
    /// trouble, so the cross-version mixing this permits is measured, not hoped.
    #[serde(default = "default_true")]
    pub prefer_system_browser: bool,

    /// `PLAYWRIGHT_DOWNLOAD_HOST` for the install. Playwright's CDN is blocked
    /// on some networks exactly as GitHub release assets are; npmmirror carries
    /// a mirror. A config key rather than "go export a variable", because the
    /// installer runs inside the daemon.
    #[serde(default)]
    pub download_host: Option<String>,
}

impl BrowserRuntimeConfig {
    /// The pinned binary, or `None` when unset **or blank**.
    ///
    /// A cleared form field posts `""`, and `Some("")` would be spent as a path
    /// — resolving to the current directory and failing with a message that
    /// names nothing.
    #[must_use]
    pub fn pinned_binary(&self) -> Option<&str> {
        self.binary_path.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }

    /// The download mirror, or `None` when unset or blank. See
    /// [`Self::pinned_binary`] for why blank is not a value.
    #[must_use]
    pub fn download_host(&self) -> Option<&str> {
        self.download_host.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }
}

impl Default for BrowserRuntimeConfig {
    fn default() -> Self {
        Self {
            binary_path: None,
            prefer_system_browser: true,
            download_host: None,
        }
    }
}
```

  然后在 `BrowserSystemConfig`（:244-261）的 `chrome_mcp` 字段之后加：

```rust
    /// External-runtime supply for the managed driver's Chromium.
    #[serde(default)]
    pub runtime: BrowserRuntimeConfig,
```

  ⚠️ `BrowserSystemConfig` derives `Default`（:244），新字段有 `Default` 实现，无需改那一行。

- [ ] **跑到绿。** `cargo test -p alephcore --lib browser::profile`
- [ ] **证伪一次。** 把 `pinned_binary` 的 `.filter(|s| !s.is_empty())` 去掉 → `browser_runtime_reads_its_three_keys_and_treats_empty_as_unset` 必须变红。恢复。
- [ ] `rustfmt src/browser/profile.rs`
- [ ] `cargo test -p alephcore --lib browser::` 全绿；`cargo test -p alephcore --lib config::` 全绿（`GeneralConfig` 里嵌着 `BrowserSystemConfig`，schemars 派生要跟着走）。
- [ ] **提交。**
  ```
  git add src/browser/profile.rs
  git commit -m "browser: add the [browser.runtime] section for chromium supply

  binary_path pins a browser, prefer_system_browser (default true) decides
  whether a system Chromium-family browser beats Playwright's own, and
  download_host carries PLAYWRIGHT_DOWNLOAD_HOST. All three read a blank
  string as unset: a cleared form field must not become an empty mirror host.

  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01TKV5PtutzoBvbT4yTpsyRY"
  ```

---

### Task 3: `chromium_resolve` —— 钉住 > 系统 > Playwright 自带

**Files:**
- Create `src/browser/chromium_resolve.rs`
- Modify `src/browser/mod.rs`（模块声明，紧跟 Task 1 加的那一行）
- Modify `src/browser/discovery.rs:125`（`find_chromium_preferred` 现在被本模块调用；`mod.rs:4` 的 `mod discovery;` 是私有的，同 crate 内 `super::discovery::` 可达，**无需**改可见性——先确认，若编译器要求再改成 `pub(super)`）

**Interfaces:**
- Consumes: `super::profile::{BrowserRuntimeConfig, BrowserType}`（Task 2）· `super::discovery::find_chromium_preferred`（`discovery.rs:125`）· `super::error::BrowserError::{ChromiumUnavailable, PlaywrightCliError}`（Task 1）· `crate::utils::no_window::NoWindow`
- Produces:
  ```rust
  pub(crate) enum ChromiumSource { Pinned, System, PlaywrightManaged }
  pub(crate) fn parse_install_location(dry_run_stdout: &str) -> Option<PathBuf>;
  pub(crate) fn executable_among(files: &[PathBuf]) -> Option<PathBuf>;
  pub(crate) async fn resolve_binary(runtime: &BrowserRuntimeConfig,
                                     browser: &BrowserType,
                                     cli_binary: &Path)
      -> Result<(PathBuf, ChromiumSource), BrowserError>;
  ```

**本机实测的两条读数（本任务的全部依据，`playwright-cli` 0.1.8 / macOS arm64）：**

```
$ playwright-cli install-browser chromium --dry-run
Chrome for Testing 147.0.7727.49 (playwright chromium v1219)
  Install location:    /Users/zouguojun/Library/Caches/ms-playwright/chromium-1219
  Download url:        https://cdn.playwright.dev/builds/cft/147.0.7727.49/mac-arm64/chrome-mac-arm64.zip

FFmpeg (playwright ffmpeg v1011)
  Install location:    /Users/zouguojun/Library/Caches/ms-playwright/ffmpeg-1011
  ...
Chrome Headless Shell 147.0.7727.49 (playwright chromium-headless-shell v1219)
  Install location:    /Users/zouguojun/Library/Caches/ms-playwright/chromium_headless_shell-1219
  ...
```

```
$ ls "$HOME/Library/Caches/ms-playwright/chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/"
Google Chrome for Testing
```

两条合起来说明：① 缓存根在 macOS 上是 `~/Library/Caches/ms-playwright/`（**不是** `~/.cache/`），② 可执行文件叫 `Google Chrome for Testing`（**不是** `Chromium`），③ 但**都不用硬编码**——装它的那个 CLI 会说出安装目录，我们只在那一个目录里找可执行文件。Linux 的 `chrome-linux/chrome` 与 Windows 的 `chrome-win/chrome.exe` 本机**未验证**，作为候选名一并列入（找不到就 fail-closed，不会认错）。

#### Steps

- [ ] **写失败测试。** 新建 `src/browser/chromium_resolve.rs`，先只写 `#[cfg(test)] mod tests`，并在 `src/browser/mod.rs` 里加 `pub(crate) mod chromium_resolve;`（否则它不参与编译，红是假的）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// The real transcript, verbatim. Three `Install location:` lines appear;
    /// only the FIRST section is the browser — the other two are ffmpeg and the
    /// headless shell. A parser that took "the first Install location" would be
    /// right today by luck; this one anchors on the section header, which is
    /// the thing that says which product the block is about.
    const DRY_RUN: &str = "\
Chrome for Testing 147.0.7727.49 (playwright chromium v1219)
  Install location:    /Users/x/Library/Caches/ms-playwright/chromium-1219
  Download url:        https://cdn.playwright.dev/builds/cft/147.0.7727.49/mac-arm64/chrome-mac-arm64.zip

FFmpeg (playwright ffmpeg v1011)
  Install location:    /Users/x/Library/Caches/ms-playwright/ffmpeg-1011
  Download url:        https://cdn.playwright.dev/dbazure/download/playwright/builds/ffmpeg/1011/ffmpeg-mac-arm64.zip

Chrome Headless Shell 147.0.7727.49 (playwright chromium-headless-shell v1219)
  Install location:    /Users/x/Library/Caches/ms-playwright/chromium_headless_shell-1219
  Download url:        https://cdn.playwright.dev/builds/cft/147.0.7727.49/mac-arm64/chrome-headless-shell-mac-arm64.zip
";

    #[test]
    fn the_install_location_comes_from_the_chromium_block_not_the_first_line() {
        assert_eq!(
            parse_install_location(DRY_RUN),
            Some(PathBuf::from(
                "/Users/x/Library/Caches/ms-playwright/chromium-1219"
            ))
        );
    }

    /// `chromium-headless-shell` starts with `chromium` — a substring match on
    /// the product name would pick the shell's directory, which has no browser
    /// in it. The anchor carries the closing `v` on purpose.
    #[test]
    fn the_headless_shell_block_is_not_mistaken_for_the_browser() {
        let shell_only = "\
Chrome Headless Shell 147.0.7727.49 (playwright chromium-headless-shell v1219)
  Install location:    /Users/x/Library/Caches/ms-playwright/chromium_headless_shell-1219
";
        assert_eq!(parse_install_location(shell_only), None);
    }

    #[test]
    fn unparseable_output_answers_i_do_not_know_rather_than_a_path() {
        for bad in [
            "",
            "playwright-cli: unknown option --dry-run",
            "Chrome for Testing 147 (playwright chromium v1219)\n", // header, no location
            "  Install location:    /tmp/x\n",                      // location, no header
        ] {
            assert_eq!(parse_install_location(bad), None, "accepted {bad:?}");
        }
    }

    /// The macOS layout is the one this machine actually has; the Linux and
    /// Windows leaves are documented-but-unverified, so they are listed as
    /// candidates and nothing more. Whatever the layout, the answer must be a
    /// FILE inside the directory the CLI named — never a guess assembled from
    /// a platform constant.
    #[test]
    fn the_executable_is_found_in_each_known_layout() {
        let mac = vec![
            PathBuf::from("/c/chromium-1219/chrome-mac-arm64/Google Chrome for Testing.app/Contents/Info.plist"),
            PathBuf::from("/c/chromium-1219/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"),
        ];
        assert_eq!(
            executable_among(&mac),
            Some(PathBuf::from("/c/chromium-1219/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"))
        );

        let linux = vec![
            PathBuf::from("/c/chromium-1219/chrome-linux/icudtl.dat"),
            PathBuf::from("/c/chromium-1219/chrome-linux/chrome"),
        ];
        assert_eq!(
            executable_among(&linux),
            Some(PathBuf::from("/c/chromium-1219/chrome-linux/chrome"))
        );

        let windows = vec![
            PathBuf::from(r"C:\c\chromium-1219\chrome-win\chrome.dll"),
            PathBuf::from(r"C:\c\chromium-1219\chrome-win\chrome.exe"),
        ];
        assert_eq!(
            executable_among(&windows),
            Some(PathBuf::from(r"C:\c\chromium-1219\chrome-win\chrome.exe"))
        );
    }

    /// An install directory that exists but holds no browser (a half-extracted
    /// download, a layout this list does not know) must answer `None`, so the
    /// caller fails closed with the install command rather than handing
    /// `Command::new` a directory.
    #[test]
    fn an_unrecognised_layout_answers_none_rather_than_a_wrong_file() {
        let files = vec![
            PathBuf::from("/c/chromium-1219/INSTALLATION_COMPLETE"),
            PathBuf::from("/c/chromium-1219/DEPENDENCIES_VALIDATED"),
            PathBuf::from("/c/chromium-1219/chrome-mac-arm64/Google Chrome for Testing.app/Contents/Info.plist"),
        ];
        assert_eq!(executable_among(&files), None);
    }

    /// `chrome-headless-shell` is a real Chromium binary, but it is only ever
    /// the DEGRADED option (§6.1: no-root Linux). It must never win over a full
    /// browser that is sitting in the same listing.
    #[test]
    fn a_full_browser_beats_the_headless_shell_in_the_same_directory() {
        let files = vec![
            PathBuf::from("/c/x/chrome-headless-shell-linux/chrome-headless-shell"),
            PathBuf::from("/c/x/chrome-linux/chrome"),
        ];
        assert_eq!(
            executable_among(&files),
            Some(PathBuf::from("/c/x/chrome-linux/chrome"))
        );
    }
}
```

- [ ] **跑它，看红。** `cargo test -p alephcore --lib browser::chromium_resolve` → `parse_install_location` / `executable_among` 未解析。

- [ ] **最小实现。** 在同文件的 `mod tests` 之前写入：

```rust
//! Which Chromium the managed driver launches, and where it came from.
//!
//! Order (spec §6.1): the operator's pin > a system Chromium-family browser >
//! Playwright's own. A system browser wins by default because Windows almost
//! always has Edge and macOS usually has Chrome — the ~150 MB download is for
//! a clean Linux host. The Chrome spike ran system Chrome 152 against
//! playwright-core 1.60, so mixing versions across that boundary is measured.
//!
//! # Why the Playwright path asks the CLI instead of globbing a cache
//!
//! The cache root is `~/Library/Caches/ms-playwright` on macOS, `~/.cache/…`
//! on Linux, `%LOCALAPPDATA%\ms-playwright` on Windows; the revision in the
//! directory name changes with every playwright-core release; and the
//! executable inside is `Google Chrome for Testing.app/Contents/MacOS/Google
//! Chrome for Testing` on macOS, not `Chromium`. Hard-coding that is three
//! platform tables and a revision guess — four facts that rot independently of
//! the installer that produces them. `playwright-cli install-browser <b>
//! --dry-run` prints the install location for the exact build THIS CLI would
//! use, so the same binary that installs it is the one that says where it is
//! (判据 §1: one derivation).

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;

use crate::utils::no_window::NoWindow;

use super::error::BrowserError;
use super::profile::{BrowserRuntimeConfig, BrowserType};

/// How long the `--dry-run` probe may take. It performs no download; a slower
/// answer than this means something is wrong with the CLI, not with the network.
const DRY_RUN_TIMEOUT: Duration = Duration::from_secs(20);

/// The header anchor of the browser block in `--dry-run` output.
///
/// The trailing `v` matters: `chromium-headless-shell` starts with `chromium`,
/// and its block names a directory with no browser in it.
const CHROMIUM_BLOCK: &str = "(playwright chromium v";

/// Executable leaf names, best first.
///
/// macOS is verified on this machine; the Linux and Windows leaves come from
/// Playwright's published layout and are **not** verified here. An unknown
/// layout therefore yields `None` and a fail-closed error naming the install
/// command — never a wrong file.
///
/// `chrome-headless-shell` is last on purpose: it is a real Chromium binary but
/// only ever the degraded, headless-only option (§6.1), so it must not win over
/// a full browser sitting in the same tree.
const EXECUTABLE_LEAVES: &[&str] = &[
    "Google Chrome for Testing",
    "Chromium",
    "chrome",
    "chrome.exe",
    "chrome-headless-shell",
    "chrome-headless-shell.exe",
];

/// How deep the install directory is walked looking for the executable.
/// macOS needs four (`chrome-mac-arm64/X.app/Contents/MacOS/X`); the others two.
const WALK_MAX_DEPTH: usize = 5;

/// Where the resolved binary came from. Carried so the log line and the doctor
/// finding can say which of the three answers won, instead of only that one did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChromiumSource {
    Pinned,
    System,
    PlaywrightManaged,
}

impl ChromiumSource {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Pinned => "pinned by [browser.runtime] binary_path",
            Self::System => "a system Chromium-family browser",
            Self::PlaywrightManaged => "Playwright's managed Chromium",
        }
    }
}

/// The browser block's `Install location:` out of `--dry-run` output.
pub(crate) fn parse_install_location(dry_run_stdout: &str) -> Option<PathBuf> {
    let mut in_block = false;
    for line in dry_run_stdout.lines() {
        if line.contains(CHROMIUM_BLOCK) {
            in_block = true;
            continue;
        }
        if !in_block {
            continue;
        }
        if let Some(rest) = line.trim_start().strip_prefix("Install location:") {
            let path = rest.trim();
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
        // A blank line ends the block; anything else non-indented starts the
        // next product. Either way, this block had no location.
        if !line.starts_with(' ') {
            in_block = false;
        }
    }
    None
}

/// The best executable among a directory listing, by [`EXECUTABLE_LEAVES`] order.
pub(crate) fn executable_among(files: &[PathBuf]) -> Option<PathBuf> {
    EXECUTABLE_LEAVES.iter().find_map(|leaf| {
        files
            .iter()
            .find(|p| p.file_name().is_some_and(|n| n == std::ffi::OsStr::new(leaf)))
            .cloned()
    })
}

/// Every file under `dir`, to a bounded depth. Bounded because this walks a
/// browser distribution: hundreds of files, and an unbounded walk over a
/// symlink loop would not return.
fn files_under(dir: &Path, depth: usize) -> Vec<PathBuf> {
    if depth == 0 {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => out.extend(files_under(&path, depth - 1)),
            Ok(_) => out.push(path),
            Err(_) => {}
        }
    }
    out
}

/// Resolve the binary the managed driver should launch for `browser`.
///
/// # Errors
///
/// [`BrowserError::ChromiumUnavailable`] when no route produced a file — its
/// message names the install command and the pin, because a closed gate that
/// does not say how to open it is fail-dead (判据 §14). A pin that does not
/// exist is that error too, and deliberately not a fallback: launching a
/// different browser than the one the operator named would be a silent
/// substitution of exactly the thing they pinned to prevent.
pub(crate) async fn resolve_binary(
    runtime: &BrowserRuntimeConfig,
    browser: &BrowserType,
    cli_binary: &Path,
) -> Result<(PathBuf, ChromiumSource), BrowserError> {
    let mut tried: Vec<String> = Vec::new();

    if let Some(pin) = runtime.pinned_binary() {
        let path = PathBuf::from(pin);
        if path.is_file() {
            return Ok((path, ChromiumSource::Pinned));
        }
        return Err(BrowserError::ChromiumUnavailable {
            tried: format!("[browser.runtime] binary_path = {pin:?} does not exist"),
        });
    }

    if runtime.prefer_system_browser {
        match super::discovery::find_chromium_preferred(browser) {
            Ok(path) => return Ok((path, ChromiumSource::System)),
            Err(e) => tried.push(format!("no system browser ({e})")),
        }
    } else {
        tried.push("system browsers skipped (prefer_system_browser = false)".to_string());
    }

    match playwright_managed(cli_binary).await {
        Ok(path) => {
            if *browser != BrowserType::Chromium {
                // Naming the substitution rather than performing it silently:
                // Playwright manages Chromium and nothing else, so a profile
                // asking for Brave gets Chromium here.
                tracing::warn!(
                    requested = ?browser,
                    "no system browser for the requested engine; falling back to \
                     Playwright's managed Chromium"
                );
            }
            Ok((path, ChromiumSource::PlaywrightManaged))
        }
        Err(why) => {
            tried.push(why);
            Err(BrowserError::ChromiumUnavailable {
                tried: tried.join("; "),
            })
        }
    }
}

/// Ask the CLI where its own Chromium lives, then find the executable there.
///
/// The `Err` is a sentence for [`BrowserError::ChromiumUnavailable`]'s `tried`
/// field, not an error to propagate: "the CLI would not answer" and "the CLI
/// answered a directory that is not there yet" are both just "this route did
/// not produce a browser".
async fn playwright_managed(cli_binary: &Path) -> Result<PathBuf, String> {
    let mut cmd = Command::new(cli_binary);
    cmd.args(["install-browser", "chromium", "--dry-run"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(DRY_RUN_TIMEOUT, cmd.no_window().output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("playwright-cli install-browser --dry-run: {e}")),
        Err(_) => {
            return Err(format!(
                "playwright-cli install-browser --dry-run did not answer in {}s",
                DRY_RUN_TIMEOUT.as_secs()
            ))
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(dir) = parse_install_location(&stdout) else {
        return Err(
            "playwright-cli did not report an install location for chromium".to_string()
        );
    };
    executable_among(&files_under(&dir, WALK_MAX_DEPTH))
        .ok_or_else(|| format!("no chromium executable under {}", dir.display()))
}
```

- [ ] **跑到绿。** `cargo test -p alephcore --lib browser::chromium_resolve` → 6 个测试全过。
- [ ] **手工核对解析器打在真输出上。** 本机跑一次 `playwright-cli install-browser chromium --dry-run`，把 stdout 存进 `/tmp/dry.txt`，然后确认第一个 `Install location:` 属于 `(playwright chromium v` 那一块（肉眼即可；上面 `DRY_RUN` 常量就是本机这一次的逐字副本）。**如果本机 playwright-core 版本换了、块头措辞变了**，把 `CHROMIUM_BLOCK` 与 `DRY_RUN` 一起更新，并在 Task 10 的 FEATURE_LOCATOR 条目里记下新旧措辞——这正是判据 §5「列举法只覆盖立法当天的世界」的那一类。
- [ ] **证伪一次。** 把 `CHROMIUM_BLOCK` 改成 `"(playwright chromium"`（去掉尾部的 ` v`）→ `the_headless_shell_block_is_not_mistaken_for_the_browser` 必须变红。恢复。
- [ ] `rustfmt src/browser/chromium_resolve.rs`
- [ ] `cargo test -p alephcore --lib browser::` 全绿。
- [ ] **提交。**
  ```
  git add src/browser/chromium_resolve.rs src/browser/mod.rs
  git commit -m "browser: resolve chromium as pin > system > playwright-managed

  The playwright-managed route asks the CLI that installs it
  (install-browser chromium --dry-run prints Install location:) instead of
  hard-coding three platform cache paths and a revision guess. On macOS the
  cache is ~/Library/Caches/ms-playwright and the binary is 'Google Chrome for
  Testing', neither of which the guessed layout would have found.

  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01TKV5PtutzoBvbT4yTpsyRY"
  ```

---

### Task 4: `playwright_launch` —— `open_argv` → `attach_argv`，配置只剩两键

**Files:**
- Modify `src/browser/playwright_launch.rs`：模块 doc（:1-21）· 删 `browser_flag_value`（:110-121）· `launch_config_json`（:123-200）改签名 · 删 `open_argv`（:207-234）· 新增 `attach_argv` · `write_launch_config`（:294-329）改签名 · 测试（:331-495）增删
- Modify `src/browser/manager.rs`：删 `unhonored_managed_fields`（:580-591）与 `ProfileManager::new` 里调用它的循环（:153-163），以及它的测试（`:838-875` 所在的那个 `#[test]`）
- Modify `src/browser/profile.rs:36-42`（`ProfileConfig::browser` 的 doc 现在引用一个不再存在的机制）
- Modify `src/browser/playwright_cli_backend.rs:170-173`（注释引用 `playwright_launch::open_argv`）

**`attach --config` 已实测被接受**（本机 playwright-cli 0.1.8，逐字）：

```
$ playwright-cli attach --help
Options:
  --cdp        connect to an existing browser via cdp endpoint url.
  --endpoint   playwright browser server endpoint to attach to.
  --extension  connect to browser extension, optionally specify browser name (e.g. --extension=chrome)
  --config     path to the configuration file, defaults to .playwright/cli.config.json
  --session    session name (defaults to bound browser name or "default")
```

所以 `outputDir` 与 `allowUnrestrictedFileAccess` 仍然拿得到，spec §9 的第一条未验证项关闭。⚠️ `attach` 的选项里**没有** `--headed`、也**没有** `--browser`——它们只属于 `open`（`open_argv` 的 doc 逐字记着 `tab-new --headed` 被 `Unknown option` 拒绝过），传过去就是硬失败。headless 与引擎选择从此由 Aleph 自己的 Chrome argv 与 `chromium_resolve` 承担。

**Interfaces:**
- Consumes: `super::chromium_launch::CdpEndpoint`（Task 1）
- Produces:
  ```rust
  pub fn launch_config_json(output_dir: &Path) -> Value;            // was (launch, output_dir)
  pub fn attach_argv(endpoint: &CdpEndpoint, config_path: &Path) -> Vec<String>;
  pub async fn write_launch_config(session_key: &str) -> Result<PathBuf, BrowserError>;  // was (key, launch)
  // DELETED: browser_flag_value, open_argv
  ```
- `SessionLaunch`（:34-41）与 `LaunchPolicy`（:71-101）**不变**：前者仍是 Chrome argv 五个字段的来源，后者仍是「这次调用有没有资格造一个浏览器」。

#### Steps

- [ ] **写失败测试。** 在 `src/browser/playwright_launch.rs` 的 `mod tests` 里，**删除** 这四个（它们测的东西本任务正在删）：`headed_puts_the_flag_on_open_where_the_cli_accepts_it`（:389-400）、`headless_omits_the_headed_flag`（:400-408）、`browser_flag_only_carries_values_the_cli_accepts`（:426-457），并把 `every_launch_carries_an_explicit_config`（:407-425）改写成 attach 版；同时把 `a_configuring_launch_maps_onto_the_documented_schema`（:335-366）、`a_default_launch_still_produces_a_config_to_displace_the_ambient_one`（:365-379）、`launch_options_is_omitted_rather_than_emitted_empty`（:379-388）三条合并成一条新的键集测试。新增/改写后的测试：

```rust
    /// The config file's key set, exactly. Everything that used to live under
    /// `browser` moved onto the Chrome argv Aleph now builds itself
    /// (`chromium_launch::ChromiumLaunchSpec::argv`), so a `userDataDir` or
    /// `launchOptions` surviving here would be a SECOND answer to where the
    /// profile directory and the proxy come from — and the CLI's copy would be
    /// the one nothing honours, because it no longer launches anything.
    #[test]
    fn the_attach_config_carries_exactly_the_two_keys_the_cli_still_owns() {
        let json = launch_config_json(Path::new("/tmp/out"));
        assert_eq!(
            json,
            json!({
                "outputDir": "/tmp/out",
                // Not decoration: naming `outputDir` without this makes the CLI
                // refuse every caller-supplied path outside it — see the fn doc.
                "allowUnrestrictedFileAccess": true,
            })
        );
        let obj = json.as_object().expect("object");
        for gone in ["browser", "userDataDir", "launchOptions", "cdpEndpoint"] {
            assert!(!obj.contains_key(gone), "{gone} must not be in the attach config");
        }
    }

    /// `--config` rides on the attach for the same reason it rode on the open:
    /// passing it is what DISPLACES the ambient `.playwright/cli.config.json`
    /// the CLI would otherwise load from the process cwd — a file that can
    /// carry `initScript` and `cdpEndpoint`, i.e. a directory Aleph happens to
    /// run in could instrument or redirect the agent's browser.
    #[test]
    fn attach_argv_names_the_endpoint_and_always_carries_an_explicit_config() {
        let endpoint = crate::browser::chromium_launch::CdpEndpoint {
            http_url: "http://127.0.0.1:58363".into(),
            ws_url: "ws://127.0.0.1:58363/devtools/browser/abc".into(),
            pid: 4242,
        };
        let argv = attach_argv(&endpoint, Path::new("/tmp/c.json"));
        assert_eq!(
            argv,
            vec!["attach", "--cdp", "http://127.0.0.1:58363", "--config", "/tmp/c.json"]
        );
    }

    /// `open` is destructive to a browser handed over this way: it issues
    /// `page.goto('about:blank')` on the page it reuses, silently clobbering
    /// whatever was displayed (measured, spike STEP 3). `attach` left the page
    /// untouched. Nothing in this module may emit the verb again.
    #[test]
    fn the_launch_verb_is_attach_and_never_open() {
        let endpoint = crate::browser::chromium_launch::CdpEndpoint {
            http_url: "http://127.0.0.1:1".into(),
            ws_url: "ws://127.0.0.1:1/devtools/browser/x".into(),
            pid: 1,
        };
        let argv = attach_argv(&endpoint, Path::new("/tmp/c.json"));
        assert_eq!(argv.first().map(String::as_str), Some("attach"));
        assert!(!argv.iter().any(|a| a == "open"));
        // Neither flag belongs to `attach`; passing one is `Unknown option`,
        // exit 1 — a hard failure on every call, which is how `--headed` broke
        // `tab-new` before.
        assert!(!argv.iter().any(|a| a == "--headed" || a == "--browser"));
    }
```

- [ ] **跑它，看红。** `cargo test -p alephcore --lib browser::playwright_launch` → 期望 `error[E0425]: cannot find function attach_argv` 与 `error[E0061]: this function takes 1 argument but 2 arguments were supplied`（旧的 `launch_config_json` 调用点）。

- [ ] **最小实现（playwright_launch.rs）。**
  1. 把模块 doc（:1-21）第 3-21 行整段替换成：

```rust
//! What it takes to hand one `playwright-cli` session a browser Aleph launched.
//!
//! The managed driver used to launch the browser through `playwright-cli open`.
//! It no longer does: Aleph spawns Chromium itself (`chromium_launch`) and the
//! CLI joins it with `attach --cdp <http-url>`. Two measurements forced the
//! change and one forbids going back:
//!
//! 1. **A CLI-launched Chrome's debug port is not a contract.** It is random
//!    per launch, a caller's own `--remote-debugging-port` loses to
//!    Playwright's (Chrome takes the last duplicate), no `DevToolsActivePort`
//!    file is written, and `playwright-cli list` prints no endpoint.
//! 2. **`close` under `cdpEndpoint` only disconnects** — nine Chrome processes
//!    before and after, endpoint still serving, page state intact. So the
//!    browser's lifetime is Aleph's to manage, which is the whole point.
//! 3. **`open` clobbers the page it reuses**, issuing `goto('about:blank')` on
//!    it; `attach` does not. Never emit `open` against a handed-over browser.
//!
//! `--config` is accepted by `attach` (verified against playwright-cli 0.1.8's
//! own `--help`), so the containment this module set up on the `open` path —
//! `outputDir` plus `allowUnrestrictedFileAccess` — survives unchanged. Every
//! other key it used to write moved onto the Chrome argv Aleph now builds.
```

  2. **删除** `browser_flag_value` 及其上方 doc（:103-121 整块）。
  3. `launch_config_json`（:123-200）：doc 中删掉 `proxy`/`extra_args`/`userDataDir` 那两段（它们描述的键不再存在），保留 `outputDir` 与 `allowUnrestrictedFileAccess` 两段；函数体改为：

```rust
#[must_use]
pub fn launch_config_json(output_dir: &Path) -> Value {
    json!({
        "outputDir": output_dir.to_string_lossy(),
        "allowUnrestrictedFileAccess": true,
    })
}
```

  4. **删除** `open_argv` 及其 doc（:207-234 整块），在原位写入：

```rust
/// The `attach` argv (after the `-s=<session>` flag the driver always prepends).
///
/// `--cdp` takes the http form of the endpoint. Both `http://…` and
/// `ws://…/devtools/browser/<id>` were accepted in the spike; the http one is
/// chosen because it does not embed a browser id that changes on every launch,
/// so a retry does not have to re-read the port file to rebuild the URL.
///
/// No `--headed` and no `--browser`: neither is an option of `attach`
/// (verified against the CLI's own `--help`), and headedness and engine choice
/// are now properties of the Chrome argv Aleph builds — see
/// [`super::chromium_launch::ChromiumLaunchSpec::argv`] and
/// [`super::chromium_resolve::resolve_binary`].
#[must_use]
pub fn attach_argv(endpoint: &super::chromium_launch::CdpEndpoint, config_path: &Path) -> Vec<String> {
    vec![
        "attach".to_string(),
        "--cdp".to_string(),
        endpoint.http_url.clone(),
        "--config".to_string(),
        config_path.to_string_lossy().into_owned(),
    ]
}
```

  5. `write_launch_config`（:294-329）：签名去掉 `launch` 参数，doc 的最后一段（「Rewritten on every launch rather than written once: the profile's proxy / user-data-dir / extra args can change…」）替换成：

```rust
/// Rewritten on every attach rather than written once: `outputDir` is derived
/// from the session key and the home dir, both of which a restart can move, and
/// the file is only read at attach time — so the cheapest correct thing is to
/// make it a pure function of the current session.
```

  函数体里删掉 `let body = launch_config_json(launch, &output_dir).to_string();` 的 `launch` 实参，改为 `launch_config_json(&output_dir)`。

- [ ] **最小实现（三个引用点）。**
  - `src/browser/manager.rs`：删掉 `:153-163` 的 `for (name, p) in &profiles { for field in unhonored_managed_fields(&p.config) { tracing::warn!(...) } }` 整块（保留它前后的代码），删掉 `:575-591` 的 `unhonored_managed_fields` 函数及其 doc，删掉测试模块里那个断言它的 `#[test]`（包含 `:845` 的 `assert_eq!(unhonored_managed_fields(&cfg), vec!["browser"]);` 的整个函数）。**理由写进删除处不留注释**——函数删了就没地方留注释了，理由进 Task 10 的 FEATURE_LOCATOR 条目：它唯一的返回条件是 `browser_flag_value(..).is_none()`，而 `browser_flag_value` 没了；留着它就是一个恒空的清单，判据 §2 的第二张脸（恒绿）。
  - `src/browser/profile.rs:36-42`：`ProfileConfig::browser` 的 doc 改为：
    ```rust
    /// Which browser engine to use.
    ///
    /// Honored by both drivers. The managed driver no longer passes the engine
    /// to `playwright-cli` (it launches the browser itself); the value steers
    /// `discovery::find_chromium_preferred`, so a profile asking for Brave gets
    /// Brave when Brave is installed. When no system browser matches, the
    /// fallback is Playwright's Chromium and the substitution is logged by
    /// `chromium_resolve::resolve_binary` rather than left silent.
    ```
  - `src/browser/playwright_cli_backend.rs:170-173`：把注释里的 `playwright_launch::open_argv` 改成 `chromium_launch::ChromiumLaunchSpec::argv`（headedness 现在住在那里）。

- [ ] **跑到绿。** `cargo test -p alephcore --lib browser::playwright_launch browser::manager` — 此时 `playwright_cli.rs:24` 还 `use ... open_argv` 会编译失败，这是 Task 5 的入口；**允许**本步以 `playwright_cli.rs` 的两个编译错结束，Task 5 第一步就修它。若希望每一步都能编译，把 Task 4 与 Task 5 合成一次提交——推荐**不要**，两者的红各自可归因。
- [ ] `rustfmt src/browser/playwright_launch.rs src/browser/manager.rs src/browser/profile.rs src/browser/playwright_cli_backend.rs`
- [ ] **提交（与 Task 5 一起，见 Task 5 的最后一步）。** 本任务单独不产生可编译状态，所以不单独提交——这一点写在这里而不是留给执行者发现。

---

### Task 5: 惰性 `open` → 惰性 `attach`（`PlaywrightCliDriver` 的翻转）

**Files:**
- Modify `src/browser/playwright_cli.rs`：imports（:6-25）· `PlaywrightCliDriver` 结构（:51-56）与 `new`（:59-67）· `run`（:177-221）· `open_session`（:223-249）→ `attach_session` · 新增 `ensure_chromium` / `forget_chromium` / `endpoint` / `chromium_alive` / `shutdown_chromium` · `classify_failure`（:331-389）加锚点 · 测试模块（文件尾）

**Interfaces:**
- Consumes: `super::chromium_launch::{ChromiumChild, ChromiumLaunchSpec, CdpEndpoint, DEVTOOLS_PORT_DEADLINE}`（Task 1）· `super::chromium_resolve::resolve_binary`（Task 3）· `super::playwright_launch::{attach_argv, write_launch_config, browser_state_dir, sanitize_session_key}`（Task 4；后两个是 `pub(super)`，同 `browser` 模块内可达）· `super::profile::BrowserRuntimeConfig`（Task 2）
- Produces:
  ```rust
  impl PlaywrightCliDriver {
      pub fn new(config: PlaywrightCliConfig, runtime: BrowserRuntimeConfig) -> Self;   // was (config)
      pub(crate) fn endpoint(&self, session_key: &str) -> Option<CdpEndpoint>;
      pub(crate) fn chromium_alive(&self, session_key: &str) -> bool;
      pub(crate) fn shutdown_chromium(&self, session_key: &str) -> bool;
  }
  pub(crate) fn chromium_user_data_dir(launch: &SessionLaunch, session_key: &str)
      -> Result<PathBuf, BrowserError>;
  ```

**真实的 attach 失败转录**（本机，scratch HOME，`playwright-cli -s=alephplanprobe attach --cdp http://127.0.0.1:1`）：

```
EXIT=1
--- STDOUT ---            (empty)
--- STDERR ---
/Users/…/playwright-core/lib/tools/cli-client/session.js:185
          reject(new Error(message));
                 ^

Error: connect ECONNREFUSED 127.0.0.1:1
Call log:
  - <ws preparing> retrieving websocket url from http://127.0.0.1:1
    at Socket.<anonymous> (…/session.js:185:18)
    …
Node.js v24.14.1
```

三点要记住：① 退出码是 **1**，stdout 是**空的**——与「浏览器没开」那两句都不同（D.9.13 的两个锚点一个在 stdout 一个在 stderr）；② 文本里**不含**任何现有锚点（`is not open` / `no session` / timeout 措辞），所以加新锚点不会与旧的冲突；③ 它是一句需要**重启浏览器再试**的失败，不是一句需要报给模型的失败。

#### Steps

- [ ] **写失败测试。** 在 `src/browser/playwright_cli.rs` 文件尾的测试模块里追加（若该文件尚无 `#[cfg(test)] mod tests`，新建一个）：

```rust
#[cfg(test)]
mod attach_tests {
    use super::*;

    /// The verbatim stderr of a real `attach --cdp` against a dead port
    /// (playwright-cli 0.1.8 / node 24.14.1), trimmed of the stack frames that
    /// carry absolute paths.
    const ATTACH_REFUSED: &str = "\
Error: connect ECONNREFUSED 127.0.0.1:1
Call log:
  - <ws preparing> retrieving websocket url from http://127.0.0.1:1
";

    /// A refused attach is its own outcome. It is NOT `NoSession` (that would
    /// loop straight back into another attach against the same dead endpoint)
    /// and it is NOT a generic CLI error (that would surface to the model as
    /// "exit 1: <node stack trace>" for a browser that merely needs
    /// relaunching).
    #[test]
    fn a_refused_attach_classifies_as_attach_failed() {
        let err = classify_failure("", ATTACH_REFUSED, 1, "default", 10_000);
        assert!(
            matches!(err, BrowserError::AttachFailed(_)),
            "expected AttachFailed, got {err:?}"
        );
    }

    /// The two "not open" phrasings (appendix D.9.13) must keep producing
    /// `NoSession` — that is what makes the lazy attach fire at all. Adding the
    /// attach anchors must not shadow either of them.
    #[test]
    fn both_not_open_phrasings_still_produce_no_session() {
        for (stdout, stderr) in [
            ("The browser 'default' is not open, please run open first", ""),
            ("", "Error: Browser 'default' is not open. Run open to start the browser session"),
        ] {
            assert!(
                matches!(
                    classify_failure(stdout, stderr, 1, "default", 10_000),
                    BrowserError::NoSession(_)
                ),
                "lost the lazy-attach trigger for {stdout:?}/{stderr:?}"
            );
        }
    }

    /// Page text must not be able to talk the classifier into a relaunch. The
    /// anchors are the CLI's own phrasings, and a page echoing them under
    /// `### Result` reaches this function only via `parse_error_section`, which
    /// already requires `### Error` to be the FIRST section.
    #[test]
    fn an_unrelated_failure_is_not_read_as_a_refused_attach() {
        let err = classify_failure("", "Error: strict mode violation: locator resolved to 3 elements", 1, "default", 10_000);
        assert!(
            !matches!(err, BrowserError::AttachFailed(_)),
            "over-broad attach anchor: {err:?}"
        );
    }

    /// The user-data-dir is where `DevToolsActivePort` lands, so the managed
    /// driver can no longer keep a browser "in memory": a profile that
    /// configures none gets one derived under `~/.aleph/data/browser`. The
    /// containment property is the same one `config_path_for` has — one
    /// component under the state dir, whatever the session key looks like.
    #[test]
    fn every_profile_gets_a_user_data_dir_and_it_cannot_escape() {
        let configured = chromium_user_data_dir(
            &SessionLaunch {
                user_data_dir: Some("/tmp/explicit".into()),
                ..SessionLaunch::headless_default()
            },
            "default",
        )
        .expect("home resolves");
        assert_eq!(configured, std::path::PathBuf::from("/tmp/explicit"));

        let derived = chromium_user_data_dir(&SessionLaunch::headless_default(), "default")
            .expect("home resolves");
        let dir = derived.parent().expect("has a parent").to_path_buf();
        for hostile in ["../../etc", "/etc", "..", "", "a/b"] {
            let p = chromium_user_data_dir(&SessionLaunch::headless_default(), hostile)
                .expect("home resolves");
            assert_eq!(p.parent(), Some(dir.as_path()), "escaped with {hostile:?}");
            assert_eq!(
                p.components().count(),
                dir.components().count() + 1,
                "not a single component for {hostile:?}"
            );
        }
    }

    /// The sealed test twin must stay sealed: a unit test may not install a
    /// runtime, and now it may not launch a Chromium either.
    #[tokio::test]
    async fn an_unconfigured_driver_still_refuses_to_reach_outside_the_process() {
        let driver = PlaywrightCliDriver::new(
            PlaywrightCliConfig::default(),
            crate::browser::profile::BrowserRuntimeConfig::default(),
        );
        assert!(matches!(
            driver.resolve_binary().await,
            Err(BrowserError::PlaywrightCliNotInstalled)
        ));
        assert!(driver.endpoint("default").is_none());
        assert!(!driver.chromium_alive("default"));
        assert!(!driver.shutdown_chromium("default"));
    }
}
```

- [ ] **跑它，看红。** `cargo test -p alephcore --lib browser::playwright_cli` → 先是 Task 4 遗留的 `open_argv` 未解析，修掉 import 之后是 `chromium_user_data_dir` 未解析、`PlaywrightCliDriver::new` 参数数目不符、`AttachFailed` 分类断言失败。

- [ ] **最小实现。**
  1. imports（:24-25）改为：
```rust
use super::chromium_launch::{ChromiumChild, ChromiumLaunchSpec, CdpEndpoint, DEVTOOLS_PORT_DEADLINE};
use super::playwright_launch::{attach_argv, write_launch_config, LaunchPolicy, SessionLaunch};
use super::profile::{BrowserRuntimeConfig, PlaywrightCliConfig};
```
  并在 `use std::collections::HashMap;`（:6）之后确认 `use std::path::{Path, PathBuf};`（:7）已在。

  2. 结构与构造（:51-67）：
```rust
pub struct PlaywrightCliDriver {
    binary_path: RwLock<Option<PathBuf>>,
    config: PlaywrightCliConfig,
    runtime: BrowserRuntimeConfig,
    per_session_locks: RwLock<HashMap<String, Arc<Mutex<()>>>>,
    binary_resolve_lock: tokio::sync::Mutex<()>,
    /// The Chromium this driver launched, per session key.
    ///
    /// It lives HERE and not on `ProfileManager` because the lazy attach — the
    /// only place a browser comes into existence — runs under
    /// `per_session_locks`, three lines away. A map on the manager would have
    /// to re-derive that serialization, and two callers racing into an unopened
    /// session would each spawn a Chromium.
    ///
    /// A `std` mutex, never held across an `await`: the resolve-and-spawn
    /// happens outside it, and the per-session lock is what keeps that safe.
    chromium: crate::sync_primitives::Mutex<HashMap<String, ChromiumChild>>,
}

impl PlaywrightCliDriver {
    #[must_use]
    pub fn new(config: PlaywrightCliConfig, runtime: BrowserRuntimeConfig) -> Self {
        Self {
            binary_path: RwLock::new(None),
            config,
            runtime,
            per_session_locks: RwLock::new(HashMap::new()),
            binary_resolve_lock: tokio::sync::Mutex::new(()),
            chromium: crate::sync_primitives::Mutex::new(HashMap::new()),
        }
    }
```

  3. `run`（:197-221）的 `NoSession` 臂改为调 `attach_session`：
```rust
        match self.spawn(&bin, session_key, args, timeout).await {
            Err(BrowserError::NoSession(key)) => {
                let Some(launch) = policy.launch() else {
                    return Err(BrowserError::NoSession(key));
                };
                self.attach_session(&bin, session_key, launch).await?;
                // One retry only. If the session still reports "not open"
                // after a successful attach, that is a real failure and must
                // surface rather than loop.
                self.spawn(&bin, session_key, args, timeout).await
            }
            other => other,
        }
```
  同时把 `run` 的 doc（:177-196）里 "**Why lazily, off the CLI's own refusal, rather than eagerly at construction:**" 那一段替换为：
```rust
    /// **Why lazily, off the CLI's own refusal, rather than eagerly:** the CLI
    /// is not the thing that owns the browser any more, but it is still the
    /// only thing that knows whether *this session* is attached. Attaching only
    /// when it says it is not makes a redundant attach structurally impossible,
    /// whereas an eager attach would have to be right about every path that
    /// obtains a backend. (Under `open` the same shape was load-bearing for a
    /// harder reason: a second `open` relaunched the browser and dropped every
    /// tab. A second `attach` is merely wasteful — but the browser it would
    /// attach to is now Aleph's, and re-deriving "is it alive" per call is how
    /// two answers to that question get created.)
```

  4. 把 `open_session`（:223-249）整块替换成：
```rust
    /// Give this session a browser: make sure Aleph's Chromium for the profile
    /// is alive, then `attach --cdp` to it.
    ///
    /// Calls [`Self::spawn`] directly rather than [`Self::run`]: the lock is
    /// already held by the caller, and going through `run` would make the
    /// attach-on-`NoSession` path re-entrant.
    ///
    /// One retry, and only for [`BrowserError::AttachFailed`]. That is the
    /// answer to the one race this design has: the liveness check said the
    /// child was there (or could not tell — see `ChromiumChild::alive`) and the
    /// endpoint refused the connection a moment later. Forgetting the child and
    /// attaching once more relaunches it. Bounded at one so a genuinely
    /// unreachable endpoint surfaces instead of looping.
    async fn attach_session(
        &self,
        bin: &Path,
        session_key: &str,
        launch: &SessionLaunch,
    ) -> Result<(), BrowserError> {
        match self.attach_once(bin, session_key, launch).await {
            Err(BrowserError::AttachFailed(detail)) => {
                tracing::warn!(session = %session_key, %detail, "attach refused; relaunching chromium");
                self.forget_chromium(session_key);
                self.attach_once(bin, session_key, launch).await
            }
            other => other,
        }
    }

    async fn attach_once(
        &self,
        bin: &Path,
        session_key: &str,
        launch: &SessionLaunch,
    ) -> Result<(), BrowserError> {
        let endpoint = self.ensure_chromium(session_key, launch).await?;
        let config_path = write_launch_config(session_key).await?;
        let argv = attach_argv(&endpoint, &config_path);
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        // Attaching is not a navigation and must not borrow the navigation
        // budget. Same reasoning, and the same number, the `open` path used.
        let timeout =
            Duration::from_secs(self.config.nav_timeout_secs.max(SESSION_START_TIMEOUT_SECS));
        self.spawn(bin, session_key, &args, timeout).await?;
        tracing::info!(
            session = %session_key,
            endpoint = %endpoint.http_url,
            "playwright-cli attached to Aleph's chromium"
        );
        Ok(())
    }

    /// The endpoint of this session's Chromium, launching one if there is none
    /// (or the one there is has exited).
    ///
    /// Safe without its own lock because every caller holds the per-session
    /// lock from [`Self::run`]. The `chromium` mutex is taken twice, briefly,
    /// and never across the `await`s in between.
    async fn ensure_chromium(
        &self,
        session_key: &str,
        launch: &SessionLaunch,
    ) -> Result<CdpEndpoint, BrowserError> {
        {
            let mut map = self.chromium.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(child) = map.get_mut(session_key) {
                if child.alive() {
                    return Ok(child.endpoint().clone());
                }
                // Exited. Drop the record here; `shutdown` reaps it and clears
                // the sidecar so the next boot does not try to kill this pid.
                if let Some(dead) = map.remove(session_key) {
                    dead.shutdown();
                }
            }
        }

        let bin = self.resolve_binary().await?;
        let (binary, source) =
            super::chromium_resolve::resolve_binary(&self.runtime, &launch.browser, &bin).await?;
        let user_data_dir = chromium_user_data_dir(launch, session_key)?;
        let spec = ChromiumLaunchSpec {
            binary,
            user_data_dir,
            headless: launch.headless,
            proxy: launch.proxy.clone(),
            extra_args: launch.extra_args.clone(),
        };
        tracing::info!(
            session = %session_key,
            binary = %spec.binary.display(),
            source = source.label(),
            "launching chromium for the managed profile"
        );
        let child = ChromiumChild::spawn(&spec, DEVTOOLS_PORT_DEADLINE).await?;
        let endpoint = child.endpoint().clone();
        let mut map = self.chromium.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(previous) = map.insert(session_key.to_string(), child) {
            // Cannot happen while the per-session lock is held; if it ever
            // does, the previous browser is leaked unless it is killed here.
            previous.shutdown();
        }
        Ok(endpoint)
    }

    /// Kill and forget this session's Chromium. Returns whether there was one.
    fn forget_chromium(&self, session_key: &str) -> bool {
        let taken = self
            .chromium
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session_key);
        match taken {
            Some(child) => {
                child.shutdown();
                true
            }
            None => false,
        }
    }

    /// This session's live CDP endpoint, if it has one. The accessor spec §3.2
    /// asks for; nothing in this plan consumes it beyond a test, and the live
    /// view (Plan 2) is its first real caller.
    pub(crate) fn endpoint(&self, session_key: &str) -> Option<CdpEndpoint> {
        self.chromium
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_key)
            .map(|c| c.endpoint().clone())
    }

    /// Whether this session's Chromium is running. The authoritative answer to
    /// "does this managed profile have a browser" now that Aleph owns it.
    pub(crate) fn chromium_alive(&self, session_key: &str) -> bool {
        self.chromium
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(session_key)
            .is_some_and(ChromiumChild::alive)
    }

    /// Public face of [`Self::forget_chromium`], for the idle reaper.
    pub(crate) fn shutdown_chromium(&self, session_key: &str) -> bool {
        self.forget_chromium(session_key)
    }
```

  5. 在 `classify_failure`（:361-389）的第一个 `if` **之前**插入 attach 分支，并在函数 doc 末尾补一段：
```rust
    // A refused attach, before the not-open anchors: it is neither "no browser
    // for this session" (which would attach again against the same dead
    // endpoint) nor a page-level failure. Anchored on the two phrases the real
    // transcript carries — the node error and playwright's own call log — so a
    // future rewording of one still matches the other.
    if s.contains("econnrefused") || s.contains("retrieving websocket url from") {
        return BrowserError::AttachFailed(detail.trim().to_string());
    }
```
  doc 追加：
```rust
/// A **third** phrasing joined the two "not open" ones when the driver stopped
/// launching browsers: a refused `attach --cdp`. Measured, not guessed —
/// `playwright-cli -s=x attach --cdp http://127.0.0.1:1` exits 1 with an EMPTY
/// stdout and a node exception on stderr (`Error: connect ECONNREFUSED …` plus
/// a `Call log:` line `- <ws preparing> retrieving websocket url from …`). It
/// shares no substring with either not-open phrase, so the anchors do not
/// interact; both of its own phrases are kept for the reason the not-open ones
/// were (a fourth wording will resemble one of them rather than a prediction).
```
  ⚠️ 因为新分支在函数最前面 `return`，原有的 `if s.contains(...) { ... } else if ...` 链**不动**。

  6. 在文件末尾（`parse_error_section` 之后、测试模块之前）加：
```rust
/// Where this session's Chromium keeps its profile.
///
/// The profile's own `user_data_dir` when it has one; otherwise a directory
/// derived under `~/.aleph/data/browser/chromium-udd/<key>`.
///
/// **A managed profile can no longer be "in memory".** `DevToolsActivePort` is
/// written into the user-data-dir, so a browser with no profile directory has
/// no discoverable endpoint — the file IS the contract. That is a behaviour
/// change for a default profile and it is stated here rather than left to be
/// discovered: browsing state (cookies, localStorage) now survives a restart
/// for every managed profile, not only for the ones that asked.
pub(crate) fn chromium_user_data_dir(
    launch: &SessionLaunch,
    session_key: &str,
) -> Result<PathBuf, BrowserError> {
    if let Some(dir) = &launch.user_data_dir {
        return Ok(PathBuf::from(dir));
    }
    Ok(super::playwright_launch::browser_state_dir("chromium-udd")?
        .join(super::playwright_launch::sanitize_session_key(session_key)))
}
```

- [ ] **跑到绿。** `cargo test -p alephcore --lib browser::` — 此时 `manager.rs` 的 `PlaywrightCliDriver::new(...)` 调用点（:96-97）少一个参数，Task 6 修它；本步允许以那一个编译错结束，或者直接把 Task 6 的第一处改动一起做掉再跑。**推荐**：先只把 `manager.rs:96-97` 改成 `PlaywrightCliDriver::new(config.playwright_cli.clone(), config.runtime.clone())`，其余留给 Task 6。
- [ ] **证伪两次。** ① 把新加的 attach 分支注释掉 → `a_refused_attach_classifies_as_attach_failed` 必须变红。② 把它的条件改成 `s.contains("error")` → `an_unrelated_failure_is_not_read_as_a_refused_attach` 与 `both_not_open_phrasings_still_produce_no_session` 必须变红（第二条会红是因为 not-open 的 stderr 那句以 `Error:` 开头——这正是为什么锚点必须窄）。两次都恢复。
- [ ] `rustfmt src/browser/playwright_cli.rs`
- [ ] `cargo test -p alephcore --lib browser::` 全绿；`cargo test -p alephcore --lib --no-run` 编译通过。
- [ ] **提交（Task 4 + Task 5 一起）。**
  ```
  git add src/browser/playwright_launch.rs src/browser/playwright_cli.rs \
          src/browser/playwright_cli_backend.rs src/browser/manager.rs src/browser/profile.rs
  git commit -m "browser: attach playwright-cli to Aleph's chromium over cdp

  The lazy launch now ensures Aleph's own Chromium is alive and runs
  'attach --cdp <http-url>' instead of 'open'. open is never emitted again: it
  issues goto('about:blank') on the page it reuses. The attach config keeps
  only outputDir and allowUnrestrictedFileAccess; every other key moved onto
  the Chrome argv. A refused attach is classified from the real transcript
  (ECONNREFUSED + 'retrieving websocket url from') and relaunches once.
  browser_flag_value, open_argv and unhonored_managed_fields are cut: with no
  --browser flag left to pass, the last one could only ever return empty.

  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01TKV5PtutzoBvbT4yTpsyRY"
  ```

---

### Task 6: `ProfileManager` —— `live_endpoint`、收割器杀浏览器、boot 收孤儿

**Files:**
- Modify `src/browser/manager.rs`：`new`（:96-97，Task 5 已改一行）· `spawn_idle_reaper`（:208-263）· `reap_idle`（:306-350）· `idle_managed_profiles`（:352-370）· `session_active`（:372-384）· 新增 `live_endpoint` · 测试模块
- Modify `src/browser/mod.rs:20-21`（把 `CdpEndpoint` 挂进 crate 内的重导出，`live_endpoint` 的返回类型要能被 `src/gateway/` 里的未来调用者命名）

**Interfaces:**
- Consumes: `PlaywrightCliDriver::{endpoint, chromium_alive, shutdown_chromium}`（Task 5）· `chromium_launch::reap_orphans_now`（Task 1）· `playwright_launch::browser_state_dir`（`playwright_launch.rs:256`）
- Produces:
  ```rust
  impl ProfileManager {
      pub(crate) fn live_endpoint(&self, profile: &str) -> Option<CdpEndpoint>;
  }
  ```

#### Steps

- [ ] **写失败测试。** 在 `src/browser/manager.rs` 的 `mod tests` 里追加：

```rust
    /// The view accessor spec §3.2 asks for, and the one property that makes it
    /// honest: an `ExistingSession` profile has no Aleph-owned browser, so it
    /// must answer `None` rather than somebody else's endpoint. The live view is
    /// Managed-only on purpose — a user's own Chrome is already visible to them.
    #[tokio::test]
    async fn live_endpoint_is_none_without_a_browser_and_never_answers_for_existing_session() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        // Both auto-injected profiles exist (`ProfileManager::new`): `default`
        // is Managed, `user` is ExistingSession.
        assert!(manager.live_endpoint("default").is_none());
        assert!(manager.live_endpoint("user").is_none());
        assert!(manager.live_endpoint("no-such-profile").is_none());
    }

    /// `session_active` used to answer from the tab registry, which its own doc
    /// called an approximation. Now that Aleph owns the process there is an
    /// exact answer, and the approximation must be GONE rather than kept beside
    /// it — two answers to "does this profile have a browser" is how they drift.
    /// Concretely: tracking a tab must no longer make a browserless profile
    /// report itself active.
    #[tokio::test]
    async fn a_tracked_tab_no_longer_fakes_a_live_managed_session() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        manager.touch_tab("default", "tab-1");
        assert!(
            manager.has_tracked_tabs("default"),
            "precondition: the registry did record the tab"
        );
        assert!(
            !manager.session_active("default"),
            "no chromium was ever launched, so the profile is not active"
        );
    }

    /// The reaper's Managed arm has two halves now, and the second one is the
    /// point: under `attach`, `playwright-cli close` only DISCONNECTS (measured
    /// — nine Chrome processes before and after). A reaper that stopped at
    /// `close` would report a reaped profile and leave the browser running
    /// forever. With no browser to begin with, the sweep must be a no-op and
    /// must not invent one.
    #[tokio::test]
    async fn the_reaper_does_not_launch_a_browser_in_order_to_reap_one() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "default".into(),
            ProfileConfig {
                driver: BrowserDriver::Managed,
                idle_timeout_secs: 0,
                ..Default::default()
            },
        );
        let manager = ProfileManager::new(config);
        assert_eq!(manager.reap_idle().await, 0);
        assert!(manager.live_endpoint("default").is_none());
    }
```

  ⚠️ `AlephHomeEnvGuard` 的真实路径是 `crate::utils::paths::AlephHomeEnvGuard`（`src/tasks/cron/mod.rs:819`、`src/config/save.rs:17` 都这样引用，它本身是 `#[cfg(test)]` 的）。`manager.rs` 的测试模块里加 `use crate::utils::paths::AlephHomeEnvGuard;`。

- [ ] **跑它，看红。** `cargo test -p alephcore --lib browser::manager` → `live_endpoint` 未解析；`a_tracked_tab_no_longer_fakes_a_live_managed_session` 断言失败（现状 `session_active` 就是 `has_tabs`）。

- [ ] **最小实现。**
  1. `spawn_idle_reaper`（:208-263）：在 `*slot = Some(Arc::downgrade(self));`（:243）**之前**插入 boot 孤儿回收：
```rust
        // Boot hook, and the only one that runs exactly once per SERVED
        // manager (a `ProfileManager` built by a test or a CLI never claims the
        // slot above). Anything Aleph launched before a crash is still running:
        // Chrome does not exit when its parent does, and under `attach` the CLI
        // was never its parent anyway.
        match super::playwright_launch::browser_state_dir("chromium-udd") {
            Ok(root) => {
                let reaped = super::chromium_launch::reap_orphans_now(&root);
                if reaped > 0 {
                    tracing::info!("reaped {reaped} orphaned chromium process(es) from a previous run");
                }
            }
            // Cannot resolve the home dir: say so. Silently skipping would leak
            // a browser per crash with nothing in the log to point at it.
            Err(e) => tracing::warn!(error = %e, "cannot sweep orphaned chromium processes"),
        }
```
  并在该函数 doc 的第一段之后补一句：
```rust
    /// It is also where the previous run's orphaned Chromium processes are
    /// reaped — same argument as the live-config handle below: "the manager
    /// whose reaper runs" is precisely "the manager the daemon serves from".
```

  2. `reap_idle`（:320-350）的 Managed 循环，在 `self.tab_registry.clear_profile(&name);` 之前加一行，并改它上方的 doc：
```rust
            // `close` under `attach --cdp` is a DISCONNECT: the browser, its
            // pages and their state all survive it (measured). So the reaper's
            // second half is the one that actually reclaims anything.
            self.playwright_cli_driver.shutdown_chromium(&name);
            self.tab_registry.clear_profile(&name);
```
  doc（:306-319）里 `- `Managed` → `playwright-cli close`.` 那一段替换为：
```rust
    /// - `Managed` → `playwright-cli close` (which now only disconnects the CLI
    ///   session) **and then** killing Aleph's own Chromium. Under the previous
    ///   arrangement `close` destroyed the browser the CLI had launched; under
    ///   `attach --cdp` it leaves it running, so stopping at `close` would have
    ///   reported a reaped profile over a browser that never went away.
```

  3. `idle_managed_profiles`（:352-370）：把 `&& self.tab_registry.has_tabs(name)` 换成 `&& self.playwright_cli_driver.chromium_alive(name)`，并把它的 doc（:352-357）改为：
```rust
    /// `Managed` profiles idle past their timeout that still have a browser.
    ///
    /// Exact, not approximate: the browser is Aleph's own child process, so
    /// "does one exist" is `ChromiumChild::alive`. The `close` below still
    /// tolerates `NoSession` because the CLI's session and the browser are now
    /// two different things — the browser can be alive with no session attached.
```

  4. `session_active`（:372-384）：Managed 臂改为 `Some(BrowserDriver::Managed) => self.playwright_cli_driver.chromium_alive(name),`，doc 的第三条改为：
```rust
    /// - `Managed` → Aleph's Chromium for the profile is running. Exact since
    ///   the launch-chain flip; it used to be "the tab registry has tabs",
    ///   which its own doc called an approximation.
```

  5. 在 `get_config`（:499-502）之后插入：
```rust
    /// The live CDP endpoint of a `Managed` profile's browser, if it has one.
    ///
    /// The accessor spec §3.2 asks for. `ExistingSession` answers `None` by
    /// construction: that browser is the user's own, Aleph never launched it,
    /// and the live view is deliberately Managed-only — a Chrome the user
    /// started is already on their screen.
    pub(crate) fn live_endpoint(&self, profile: &str) -> Option<super::chromium_launch::CdpEndpoint> {
        match self.get_driver(profile) {
            Some(BrowserDriver::Managed) => self.playwright_cli_driver.endpoint(profile),
            Some(BrowserDriver::ExistingSession) | None => None,
        }
    }
```

  6. `src/browser/mod.rs`：在 `pub use error::BrowserError;`（:21）之后加
```rust
pub(crate) use chromium_launch::CdpEndpoint;
```

- [ ] **跑到绿。** `cargo test -p alephcore --lib browser::manager`
- [ ] **证伪两次。** ① 把 `session_active` 的 Managed 臂改回 `self.tab_registry.has_tabs(name)` → `a_tracked_tab_no_longer_fakes_a_live_managed_session` 必须变红。② 把 `live_endpoint` 的 `ExistingSession` 臂改成也去问 driver → `live_endpoint_is_none_without_a_browser_and_never_answers_for_existing_session` 仍会绿（因为没有浏览器）——**这说明第二条守卫此刻是空的**。把它补强：在那条测试里额外断言 `manager.get_driver("user") == Some(BrowserDriver::ExistingSession)`，让「问的是哪个 driver」成为断言的一部分，然后重做这次变异并确认变红。（判据 §3：一条没被证伪过的守卫不算守卫。）
- [ ] `rustfmt src/browser/manager.rs`
- [ ] `cargo test -p alephcore --lib browser::` 与 `cargo test -p alephcore --lib gateway::handlers::browser_config` 全绿。
- [ ] **提交。**
  ```
  git add src/browser/manager.rs src/browser/mod.rs
  git commit -m "browser: the manager owns chromium's lifetime, not the CLI

  reap_idle kills Aleph's Chromium after the close that now only disconnects;
  session_active and the reap candidates ask the child process instead of the
  tab registry's self-described approximation; spawn_idle_reaper sweeps the
  previous run's orphans by argv; live_endpoint is the accessor the live view
  will consume, and answers None for existing-session profiles by construction.

  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01TKV5PtutzoBvbT4yTpsyRY"
  ```

---

### Task 7: Chromium 供给（`PLAYWRIGHT_DOWNLOAD_HOST` 透传）+ doctor 哨兵 `browser/chromium-missing`

> **偏离（前言 §5）**：**不新增 `chromium` 台账 capability**。`src/runtimes/probe.rs:65-83` 的探测只走 PATH，而 Playwright 的 Chromium 永远不在 PATH 上——加一条 spec 得到的是一个恒 `Missing` 的条目（判据 §2 的第四张脸：没装上）。供给已经存在：`src/runtimes/specs.rs:173-176` 的 `PostInstallAction::RunSubcommand { args: &["install-browser", "chromium"], target_dir: None }` 是 `playwright-cli` 的 post-install。本任务给它加镜像透传，并把「没有 Chromium」变成一件看得见的事。

**Files:**
- Modify `src/runtimes/specs.rs:47-59`（`PostInstallAction`）与 `:173-176`（playwright-cli 的 post_install）· `:460-490` 附近的那条断言 post-install 形状的测试（先 `grep -n "install-browser" src/runtimes/specs.rs` 定位，本计划读到的是 `:464` 与 `:480-485`）
- Modify `src/runtimes/post_install.rs:47-59`（`run` 的分派）· `:80-107`（`run_subcommand`）· 新增 `config_env` / `config_env_from` 与测试
- Create `src/diagnostics/checks/chromium_missing.rs`
- Modify `src/diagnostics/checks/mod.rs:17-52`（`pub mod` + `pub use`）
- Modify `src/diagnostics/mod.rs:80-95`（`default_registry` 的 `checks` 向量）

**Interfaces:**
- Consumes: `crate::browser::profile::BrowserRuntimeConfig`（Task 2）· `crate::browser::chromium_resolve::resolve_binary`（Task 3）· `crate::config::Config::load`（`src/config/load.rs:316`）· `crate::tools::probes::browser::managed_cli_path`（`browser_runtime.rs:232` 已在用）· `crate::diagnostics::check::{settle_probe, unknown_finding, HealthCheck, Posture}` · `crate::diagnostics::finding::{Finding, Severity}`
- Produces:
  ```rust
  // specs.rs
  pub enum EnvFromConfig { PlaywrightDownloadHost }
  PostInstallAction::RunSubcommand { args, target_dir, env: &'static [EnvFromConfig] }
  // post_install.rs
  pub fn config_env_from(vars: &[EnvFromConfig], runtime: &BrowserRuntimeConfig)
      -> Vec<(&'static str, String)>;
  pub fn config_env(vars: &[EnvFromConfig]) -> Vec<(&'static str, String)>;
  // diagnostics
  pub struct ChromiumMissingCheck;   // id = "browser/chromium-missing"
  ```

#### Steps

- [ ] **写失败测试（供给侧）。** 在 `src/runtimes/post_install.rs` 尾部新建测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserRuntimeConfig;

    /// The mirror is a config key rather than "go export a variable" because
    /// the installer runs inside the daemon — there is no shell for an operator
    /// to export into. Playwright's CDN is blocked on the same networks that
    /// block GitHub release assets, so without this the install just hangs and
    /// then fails with a network error naming a host nobody chose.
    #[test]
    fn the_download_host_reaches_the_installer_as_playwright_download_host() {
        let runtime = BrowserRuntimeConfig {
            download_host: Some("https://npmmirror.com/mirrors/playwright".into()),
            ..BrowserRuntimeConfig::default()
        };
        assert_eq!(
            config_env_from(&[EnvFromConfig::PlaywrightDownloadHost], &runtime),
            vec![(
                "PLAYWRIGHT_DOWNLOAD_HOST",
                "https://npmmirror.com/mirrors/playwright".to_string()
            )]
        );
    }

    /// An unset or blank mirror must produce NO variable, not an empty one.
    /// `PLAYWRIGHT_DOWNLOAD_HOST=` is not "use the default host"; it is "the
    /// host is the empty string", and every download then fails on a malformed
    /// URL. This is the fail-closed reading of a blank field (判据 §8).
    #[test]
    fn a_blank_download_host_sets_no_variable_at_all() {
        for host in [None, Some(String::new()), Some("   ".to_string())] {
            let runtime = BrowserRuntimeConfig {
                download_host: host.clone(),
                ..BrowserRuntimeConfig::default()
            };
            assert!(
                config_env_from(&[EnvFromConfig::PlaywrightDownloadHost], &runtime).is_empty(),
                "blank host {host:?} produced a variable"
            );
        }
    }

    /// An action that declares no env gets none — the passthrough must not
    /// leak onto every other post-install subcommand just because it is cheap.
    #[test]
    fn an_action_that_declares_no_env_gets_none() {
        let runtime = BrowserRuntimeConfig {
            download_host: Some("https://mirror.example".into()),
            ..BrowserRuntimeConfig::default()
        };
        assert!(config_env_from(&[], &runtime).is_empty());
    }
}
```

- [ ] **跑它，看红。** `cargo test -p alephcore --lib runtimes::post_install` → `config_env_from` / `EnvFromConfig` 未解析。

- [ ] **最小实现（供给侧）。**
  1. `src/runtimes/specs.rs`：在 `PostInstallAction`（:47-59）之前加：
```rust
/// A process-environment variable a post-install action needs, named by the
/// config key that supplies it.
///
/// An enum rather than a `(&str, &str)` pair because the value is not static:
/// it comes out of the running config, and the resolver
/// (`post_install::config_env_from`) is the single place that maps a variant to
/// a key. Both consumers — the post-install runner and the R8 install tool —
/// go through it, so the mirror cannot be honoured on one path and dropped on
/// the other (判据 §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvFromConfig {
    /// `PLAYWRIGHT_DOWNLOAD_HOST` ← `[browser.runtime] download_host`.
    PlaywrightDownloadHost,
}
```
  并给 `RunSubcommand` 加字段：
```rust
    RunSubcommand {
        args: &'static [&'static str],
        target_dir: Option<&'static str>,
        /// Environment this subcommand needs from the running config. Empty for
        /// every action but the Chromium download.
        env: &'static [EnvFromConfig],
    },
```
  2. `src/runtimes/specs.rs:173-176`：
```rust
        post_install: &[PostInstallAction::RunSubcommand {
            args: &["install-browser", "chromium"],
            target_dir: None,
            env: &[EnvFromConfig::PlaywrightDownloadHost],
        }],
```
  3. `src/runtimes/post_install.rs`：`run`（:68-78）的 `RunSubcommand` 臂改为传 `env`；`run_subcommand`（:80-107）增加参数并应用：
```rust
async fn run_subcommand(
    bin_path: &PathBuf,
    args: &[&str],
    target_dir: Option<&str>,
    env: &[super::specs::EnvFromConfig],
) -> Result<(), PostInstallError> {
    let mut cmd = Command::new(bin_path);
    cmd.args(args);
    for (key, value) in config_env(env) {
        cmd.env(key, value);
    }
    // … existing target_dir block and run_cmd_with_timeout unchanged …
```
  4. 同文件加两个函数：
```rust
/// The environment `vars` ask for, read out of the running config.
///
/// Split from [`config_env_from`] so the mapping is testable without a config
/// file: this half performs the global read, that half is a total function of
/// its inputs.
pub fn config_env(vars: &[super::specs::EnvFromConfig]) -> Vec<(&'static str, String)> {
    if vars.is_empty() {
        return Vec::new();
    }
    match crate::config::Config::load() {
        Ok(cfg) => config_env_from(vars, &cfg.general.browser.runtime),
        Err(e) => {
            // "I could not read the config" is not "the operator set no
            // mirror": say so, then proceed with the default host, which is the
            // only thing left to do.
            warn!("cannot read config for post-install environment: {e}");
            Vec::new()
        }
    }
}

/// [`config_env`]'s pure half.
#[must_use]
pub fn config_env_from(
    vars: &[super::specs::EnvFromConfig],
    runtime: &crate::browser::profile::BrowserRuntimeConfig,
) -> Vec<(&'static str, String)> {
    vars.iter()
        .filter_map(|v| match v {
            super::specs::EnvFromConfig::PlaywrightDownloadHost => runtime
                .download_host()
                .map(|h| ("PLAYWRIGHT_DOWNLOAD_HOST", h.to_string())),
        })
        .collect()
}
```
  5. 修 `src/runtimes/specs.rs` 里断言 post-install 形状的那条测试（`grep -n "install-browser" src/runtimes/specs.rs` 找到的第二处，本计划读到 `:480-485` 有 `&["install-browser", "chromium"]` 与「takes no appended target dir」的断言）：给它补一条断言而不是只改到编译通过：
```rust
        assert_eq!(
            env,
            &[EnvFromConfig::PlaywrightDownloadHost],
            "the chromium download is the one post-install action a mirror applies to"
        );
```
  ⚠️ `cfg.general.browser` 的字段路径要先核对：`config/types/general.rs` 里 `browser` 的类型是 `BrowserSystemConfig`（`add_browser_config.py` 的 doc 逐字说 sections 是 `[general.browser.*]`）。若字段名不同，按编译器改，不要猜。

- [ ] **跑到绿（供给侧）。** `cargo test -p alephcore --lib runtimes::`

- [ ] **写失败测试（doctor 侧）。** 新建 `src/diagnostics/checks/chromium_missing.rs`，先只写测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// The fail-closed contract: when there is no browser, the finding must name
    /// the command that fixes it. A gate that closes without saying how to open
    /// it is fail-dead (判据 §14), and this is the one surface an operator
    /// reaches for when the managed driver answers "no Chromium".
    #[test]
    fn the_missing_finding_names_every_way_out() {
        let f = missing_finding("no system browser; playwright's chromium is not installed");
        assert_eq!(f.check_id, ID);
        let text = format!("{} {}", f.detail, f.fix_hint.clone().unwrap_or_default());
        assert!(text.contains("playwright-cli install-browser chromium"), "{text}");
        assert!(text.contains("runtime_manage"), "{text}");
        assert!(text.contains("binary_path"), "{text}");
        // Info, not Error: the browser subsystem is optional and a
        // managed-browser-less host must not turn `aleph-server doctor`'s exit
        // code into a constant. Same argument `browser/runtime` states.
        assert_eq!(f.severity, crate::diagnostics::finding::Severity::Info);
    }

    /// A found browser says WHICH of the three routes answered. "Chromium is
    /// available" without the source is the finding an operator cannot act on:
    /// pinning, installing and the system browser are three different fixes.
    #[test]
    fn the_ok_finding_names_the_source_and_the_path() {
        let f = found_finding(
            std::path::Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            crate::browser::chromium_resolve::ChromiumSource::System,
        );
        assert_eq!(f.check_id, ID);
        assert!(f.detail.contains("Google Chrome"), "{}", f.detail);
        assert!(f.detail.contains("system Chromium-family browser"), "{}", f.detail);
    }
}
```

- [ ] **跑它，看红。** 先把 `pub mod chromium_missing;` 与 `pub use chromium_missing::ChromiumMissingCheck;` 加进 `src/diagnostics/checks/mod.rs`（按字母序，`browser_runtime` 之后、`config_parse` 之前），否则文件不参与编译。`cargo test -p alephcore --lib diagnostics::checks::chromium_missing` → `ID` / `missing_finding` / `found_finding` 未解析。

- [ ] **最小实现（doctor 侧）。** 在同文件测试之前写入：

```rust
//! `browser/chromium-missing` — can the MANAGED driver launch a browser?
//!
//! Distinct from `browser/runtime`, which asks three prerequisite questions
//! (system browser / Node for the existing-session driver / is `playwright-cli`
//! provisioned) and answers them from read-only lookups. This one asks the
//! single question the launch-chain flip created — *is there a Chromium for
//! Aleph to spawn* — and it answers it by running **the same resolver the
//! launch path runs** (`browser::chromium_resolve::resolve_binary`). A doctor
//! that re-derived the search order would be a second answer to a question the
//! driver already answers, and the two would disagree exactly when it matters
//! (判据 §1, §9).
//!
//! That means this check DOES spawn a process (`playwright-cli install-browser
//! chromium --dry-run`), unlike its sibling. It is bounded by
//! [`RESOLVE_TIMEOUT`] and a probe that does not answer in time produces the
//! `unknown` finding, never "not installed".

use async_trait::async_trait;

use crate::browser::chromium_resolve::{resolve_binary, ChromiumSource};
use crate::browser::profile::{BrowserRuntimeConfig, BrowserType};
use crate::diagnostics::check::{unknown_finding, HealthCheck, Posture};
use crate::diagnostics::finding::Finding;

const ID: &str = "browser/chromium-missing";
const SUBJECT: &str = "Managed browser";

/// The resolver spawns one short-lived CLI probe. Twenty seconds is its own
/// internal budget; this is the outer bound so a wedged CLI cannot hold
/// `aleph doctor` open.
const RESOLVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(25);

/// The finding for "there is no browser", spelled once so the doctor, the tool
/// error and the QA fixture can all be checked against the same sentence.
fn missing_finding(tried: impl std::fmt::Display) -> Finding {
    Finding::ok(
        ID,
        "No Chromium for the managed browser driver",
        format!(
            "The managed driver launches Chromium itself and could not find one ({tried}). \
             Browser tools will refuse until this is fixed; the existing-session driver \
             (attach to your own Chrome) is unaffected."
        ),
    )
    .with_fix_hint(
        "Run `playwright-cli install-browser chromium`, ask Aleph to run \
         `runtime_manage{action:\"install\", capability:\"chromium\"}`, or pin an \
         installed browser with [browser.runtime] binary_path. On a network that \
         blocks Playwright's CDN, set [browser.runtime] download_host to a mirror first.",
    )
}

/// The finding for "there is one", naming which of the three routes answered.
fn found_finding(path: &std::path::Path, source: ChromiumSource) -> Finding {
    Finding::ok(
        ID,
        "Managed browser available",
        format!("{} — {}.", path.display(), source.label()),
    )
}

#[derive(Default)]
pub struct ChromiumMissingCheck;

impl ChromiumMissingCheck {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl HealthCheck for ChromiumMissingCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Managed browser (Chromium)"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        // The CLI is the resolver's third route AND the thing that would run
        // the install. Without it there is nothing to ask and nothing to fix
        // here — `browser/runtime`'s managed probe owns that sentence, so this
        // check defers to it rather than printing a second copy.
        let Some(cli) = crate::tools::probes::browser::managed_cli_path() else {
            return vec![Finding::ok(
                ID,
                "Managed browser not checked (no playwright-cli)",
                "The managed driver's CLI is not provisioned, so there is nothing to \
                 attach a browser to yet. See the `browser/runtime` finding for that.",
            )];
        };
        let runtime = match crate::config::Config::load() {
            Ok(cfg) => cfg.general.browser.runtime.clone(),
            // A config we cannot read is not a config with default settings: a
            // pinned binary_path we failed to see would make every answer below
            // wrong. Say "I could not look".
            Err(e) => {
                return vec![unknown_finding(
                    ID,
                    SUBJECT,
                    format!("the config could not be read, so the browser pin is unknown: {e}"),
                )]
            }
        };
        let probe = tokio::time::timeout(
            RESOLVE_TIMEOUT,
            resolve_binary(&runtime, &BrowserType::default(), &cli),
        )
        .await;
        vec![match probe {
            Ok(Ok((path, source))) => found_finding(&path, source),
            Ok(Err(crate::browser::BrowserError::ChromiumUnavailable { tried })) => {
                missing_finding(tried)
            }
            // Any other error is the resolver failing to look, not a verdict.
            Ok(Err(e)) => unknown_finding(ID, SUBJECT, format!("the lookup failed: {e}")),
            Err(_) => unknown_finding(
                ID,
                SUBJECT,
                format!("the lookup did not answer within {}s", RESOLVE_TIMEOUT.as_secs()),
            ),
        }]
    }
}
```

  然后在 `src/diagnostics/mod.rs:80-94` 的 `checks` 向量里，`Arc::new(checks::BrowserRuntimeCheck::new()),`（:91）之后插入：
```rust
            Arc::new(checks::ChromiumMissingCheck::new()),
```
  `BrowserRuntimeConfig` 的 `use` 若被 clippy 判为未使用则删掉——上面的 `run` 用的是 `cfg.general.browser.runtime.clone()`，类型由推断得出。

- [ ] **跑到绿。** `cargo test -p alephcore --lib diagnostics::` 与 `cargo test -p alephcore --lib runtimes::`
- [ ] **证伪一次。** 把 `missing_finding` 的 `fix_hint` 删掉 → `the_missing_finding_names_every_way_out` 必须变红。恢复。
- [ ] **手工跑一次真 doctor。** `cargo run --bin aleph-server -- doctor 2>&1 | grep -A 3 -i chromium`。本机预期：`playwright-cli` 在 PATH 上、系统 Chrome 在 `/Applications` 下 ⇒ `Managed browser available … a system Chromium-family browser`。把这一行原样贴进 Task 10 的 FEATURE_LOCATOR 条目。⚠️ `aleph-server doctor` 是冷进程，`default_registry` 里任何恒红的检查都会把它的退出码变成常数——本检查的两条 Info 与一条 Warning（unknown）里，只有 unknown 是 Warning，且它只在「读不到配置 / 探针不答」时出现。**跑一次确认退出码仍是 0。**
- [ ] `rustfmt src/runtimes/specs.rs src/runtimes/post_install.rs src/diagnostics/checks/chromium_missing.rs src/diagnostics/checks/mod.rs src/diagnostics/mod.rs`
- [ ] `cargo test -p alephcore --lib --no-run` 与 `cargo test -p alephcore --bins` 全绿。
- [ ] **提交。**
  ```
  git add src/runtimes/specs.rs src/runtimes/post_install.rs \
          src/diagnostics/checks/chromium_missing.rs src/diagnostics/checks/mod.rs src/diagnostics/mod.rs
  git commit -m "runtimes: carry the download mirror into the chromium install; doctor sees a missing browser

  The ledger already ran 'playwright-cli install-browser chromium' as
  playwright-cli's post-install; it now carries PLAYWRIGHT_DOWNLOAD_HOST from
  [browser.runtime] download_host, with a blank value read as no mirror rather
  than an empty host. browser/chromium-missing runs the same resolver the
  launch path runs, so the doctor and the driver cannot disagree, and its
  fix hint names all three ways out.

  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01TKV5PtutzoBvbT4yTpsyRY"
  ```

---

### Task 8: R8 工具面 `runtime_manage{list,install}`

> **偏离（前言 §6）**：提议把这个折进 Task 7。拆开，因为注册一个工具要动六处 + 一条字节棘轮，混在供给任务里会让「哪一步红了」不可归因。
> **命名偏离**：spec §6.1/§7 写的是 `runtime_install{chromium}`。本计划做 `runtime_manage{action, capability}`，理由有二：① 本仓的工具命名惯例压倒性是 `<noun>_manage` + action 枚举（`agent_manage` / `channel_manage` / `skill_manage` / `plugin_manage` / `hooks_manage` / `cron_manage` …）；② `runtimes.*` RPC 家族已经有三个动词（list / refresh / install），只暴露 install 会造出一个比 RPC 家族窄的第二张脸（判据 §9「一个动词有几张脸，判据就要在每张脸上用同一个推导」）。`capability: "chromium"` 保留 spec 的调用形状。

**Files:**
- Create `src/builtin_tools/runtime_manage.rs`
- Modify `src/builtin_tools/mod.rs`（`pub mod` 按字母序插在 `remember` 与 `scratchpad` 之间；若同文件有对应的 `pub use` 区块，一并加）
- Modify `src/executor/builtin_registry/definitions.rs`：`BUILTIN_TOOL_DEFINITIONS` 加条目（照 `:248-252` 的 `list_models` 形状）· `standalone` 的 `=> None` 臂区（`:1170-1186` 一带）· `REGISTRY_SCHEMA_BASELINE`（`:3027+`）加一行 · 必要时抬 `CATALOG_DESCRIPTION_CEILING_BYTES`（`:2603`）与 `REGISTRY_SCHEMA_CEILING_BYTES`（`:2999`）
- Modify `src/executor/builtin_registry/groups.rs`（`:100-137` 的自管理组，`"doctor"` 附近）
- Modify `src/executor/builtin_registry/registry/struct_def.rs`（`:72-82` 一带）
- Modify `src/executor/builtin_registry/builder/constructor/mod.rs`（照 `:200-203` 的 `list_models_tool` 构造形状；注册结构体字面量里也要加一行，照 `:1157`）
- Modify `src/executor/builtin_registry/registry/tool_registry_impl.rs`（照 `:175-177` 的分派臂）
- Modify `src/executor/builtin_registry/builder/core_tools.rs`（照 `:185-189` 的 `reg(...)`）

**Interfaces:**
- Consumes: `crate::runtimes::{ensure_capability, find_spec, supported_on_current_os, SPECS}`（`src/gateway/handlers/runtimes.rs:12` 已在用同一组）· `crate::runtimes::ledger::{CapabilityLedger, CapabilityStatus}` · `crate::runtimes::post_install::config_env`（Task 7）· `crate::runtimes::specs::EnvFromConfig`（Task 7）· `crate::tools::probes::browser::managed_cli_path`
- Produces:
  ```rust
  pub struct RuntimeManageTool;
  pub struct RuntimeManageArgs { pub action: RuntimeAction, pub capability: Option<String> }
  pub enum RuntimeAction { List, Install }
  pub struct RuntimeManageOutput { pub ok: bool, pub message: String, pub runtimes: Vec<RuntimeRow> }
  ```

#### Steps

- [ ] **写失败测试。** 新建 `src/builtin_tools/runtime_manage.rs`，先只写测试（并把 `pub mod runtime_manage;` 加进 `src/builtin_tools/mod.rs`，否则不参与编译）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::AlephTool;

    /// `install` without a capability is not "install everything" — it is a
    /// malformed call, and answering it with a guess would install whichever
    /// spec happens to be first in the table.
    #[tokio::test]
    async fn install_without_a_capability_refuses_instead_of_guessing() {
        let out = RuntimeManageTool::new()
            .call(RuntimeManageArgs {
                action: RuntimeAction::Install,
                capability: None,
            })
            .await
            .expect("tool answers");
        assert!(!out.ok);
        assert!(out.message.contains("capability"), "{}", out.message);
    }

    /// An unknown capability must name what IS installable. "unknown capability:
    /// chrmium" that does not list the alternatives costs the model a whole turn
    /// discovering them.
    #[tokio::test]
    async fn an_unknown_capability_lists_the_ones_that_exist() {
        let out = RuntimeManageTool::new()
            .call(RuntimeManageArgs {
                action: RuntimeAction::Install,
                capability: Some("chrmium".into()),
            })
            .await
            .expect("tool answers");
        assert!(!out.ok);
        assert!(out.message.contains("chromium"), "{}", out.message);
        assert!(out.message.contains("playwright-cli"), "{}", out.message);
    }

    /// `chromium` is installable through this tool even though it is NOT a
    /// ledger spec — the ledger probes PATH, and Playwright's browser is never
    /// on PATH. `find_spec` must not be the gate, or the one capability the
    /// browser subsystem needs would be the one this tool cannot install.
    #[test]
    fn chromium_is_installable_and_is_deliberately_not_a_ledger_spec() {
        assert!(
            crate::runtimes::find_spec("chromium").is_none(),
            "a chromium RuntimeSpec would be probed on PATH and stay Missing forever"
        );
        assert!(is_installable("chromium"));
        assert!(is_installable("playwright-cli"));
        assert!(!is_installable("chrmium"));
    }

    /// The catalogue face and the RPC face answer from the same table. A tool
    /// that listed a different set than `runtimes.list` would be the second
    /// answer to "what runtimes are there" (判据 §9).
    #[tokio::test]
    async fn list_answers_from_the_same_spec_table_as_the_rpc() {
        let out = RuntimeManageTool::new()
            .call(RuntimeManageArgs {
                action: RuntimeAction::List,
                capability: None,
            })
            .await
            .expect("tool answers");
        assert!(out.ok, "{}", out.message);
        let names: Vec<&str> = out.runtimes.iter().map(|r| r.name.as_str()).collect();
        for spec in crate::runtimes::SPECS {
            assert!(names.contains(&spec.name), "{} missing from the tool face", spec.name);
        }
        assert!(names.contains(&"chromium"), "chromium is installable, so it must be listable");
    }
}
```

- [ ] **跑它，看红。** `cargo test -p alephcore --lib builtin_tools::runtime_manage`

- [ ] **最小实现。** 在同文件测试之前写入：

```rust
//! `runtime_manage` — the R8 face of the `runtimes.*` RPC family.
//!
//! Everything configurable is a tool (R8), and until now the runtime ledger was
//! the exception: `runtimes.list` / `runtimes.refresh` / `runtimes.install`
//! existed only as gateway RPCs, reachable from the Panel and from nothing the
//! model can call. So "Chromium is not installed" was a dead end in
//! conversation — the fail-closed message could name a shell command and
//! nothing else.
//!
//! `chromium` is a member of this tool's installable set WITHOUT being a
//! `RuntimeSpec`, and that is deliberate. The ledger probes PATH
//! (`runtimes::probe::probe_system_path`), and Playwright's Chromium lives in a
//! per-revision cache directory that is never on PATH — a spec for it would sit
//! at `Missing` forever and reinstall on every call. Its supply already exists
//! as `playwright-cli`'s post-install action (`install-browser chromium`), and
//! this tool re-runs exactly that command with exactly that environment.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::runtimes::ledger::{CapabilityLedger, CapabilityStatus};
use crate::runtimes::specs::EnvFromConfig;
use crate::runtimes::{ensure_capability, find_spec, supported_on_current_os, SPECS};
use crate::sync_primitives::Arc;

/// The one capability this tool installs that the ledger does not model.
const CHROMIUM: &str = "chromium";

/// The subcommand that supplies it — the SAME argv the ledger's post-install
/// action runs (`runtimes::specs`, the `playwright-cli` entry). Written once so
/// the two paths cannot drift into installing different things.
const CHROMIUM_INSTALL_ARGS: &[&str] = &["install-browser", CHROMIUM];

/// How long the browser download may take. Generous: it is ~150 MB over a
/// mirror that may be slow, and the alternative to waiting is a browser that
/// silently is not there.
const CHROMIUM_INSTALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAction {
    /// What is installed, what is missing, and what each one is for.
    List,
    /// Install one capability by name.
    Install,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeManageArgs {
    pub action: RuntimeAction,
    /// Required for `install`; ignored by `list`.
    #[serde(default)]
    pub capability: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeRow {
    pub name: String,
    pub status: String,
    pub path: Option<String>,
    pub version: Option<String>,
    pub purpose: Option<String>,
    pub supported_here: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeManageOutput {
    pub ok: bool,
    pub message: String,
    pub runtimes: Vec<RuntimeRow>,
}

/// Whether `name` is something this tool can install: a ledger spec, or the one
/// capability the ledger deliberately does not model.
#[must_use]
pub fn is_installable(name: &str) -> bool {
    name == CHROMIUM || find_spec(name).is_some()
}

/// Every installable name, for the error that has to say what IS available.
fn installable_names() -> Vec<&'static str> {
    SPECS
        .iter()
        .map(|s| s.name)
        .chain(std::iter::once(CHROMIUM))
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeManageTool;

impl RuntimeManageTool {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    async fn ledger() -> Result<Arc<tokio::sync::RwLock<CapabilityLedger>>> {
        let dir = crate::runtimes::get_runtimes_dir()
            .map_err(|e| crate::error::AlephError::tool(format!("runtimes dir: {e}")))?;
        let path = dir.join("ledger.json");
        let ledger = tokio::task::spawn_blocking(move || CapabilityLedger::load_or_create(path))
            .await
            .map_err(|e| crate::error::AlephError::tool(format!("load capability ledger: {e}")))?;
        Ok(Arc::new(tokio::sync::RwLock::new(ledger)))
    }

    async fn list() -> RuntimeManageOutput {
        let ledger = match Self::ledger().await {
            Ok(l) => l,
            Err(e) => {
                return RuntimeManageOutput {
                    ok: false,
                    message: format!("Cannot read the runtime ledger: {e}"),
                    runtimes: Vec::new(),
                }
            }
        };
        let guard = ledger.read().await;
        let mut runtimes: Vec<RuntimeRow> = SPECS
            .iter()
            .map(|spec| {
                let entry = guard.entries.get(spec.name);
                RuntimeRow {
                    name: spec.name.to_string(),
                    status: format!("{:?}", entry.map_or(CapabilityStatus::Missing, |e| e.status)),
                    path: entry
                        .filter(|e| !e.bin_path.as_os_str().is_empty())
                        .map(|e| e.bin_path.to_string_lossy().to_string()),
                    version: entry.filter(|e| !e.version.is_empty()).map(|e| e.version.clone()),
                    purpose: spec.llm_hint.map(str::to_string),
                    supported_here: supported_on_current_os(spec.name),
                }
            })
            .collect();
        // Chromium is not in the ledger (see the module doc), so its row is
        // derived from the resolver the browser driver itself uses. A row that
        // said "Missing" while a system Chrome sat in /Applications would be a
        // lie the model would act on.
        runtimes.push(chromium_row().await);
        RuntimeManageOutput {
            ok: true,
            message: format!("{} runtime(s).", runtimes.len()),
            runtimes,
        }
    }

    async fn install(capability: Option<String>) -> RuntimeManageOutput {
        let Some(name) = capability.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            return RuntimeManageOutput {
                ok: false,
                message: format!(
                    "install needs a `capability`. Installable: {}.",
                    installable_names().join(", ")
                ),
                runtimes: Vec::new(),
            };
        };
        if !is_installable(name) {
            return RuntimeManageOutput {
                ok: false,
                message: format!(
                    "unknown capability {name:?}. Installable: {}.",
                    installable_names().join(", ")
                ),
                runtimes: Vec::new(),
            };
        }
        let message = if name == CHROMIUM {
            install_chromium().await
        } else {
            let ledger = match Self::ledger().await {
                Ok(l) => l,
                Err(e) => return RuntimeManageOutput { ok: false, message: format!("{e}"), runtimes: Vec::new() },
            };
            match ensure_capability(name, &ledger).await {
                Ok(path) => format!("{name} is ready at {}.", path.display()),
                Err(e) => return RuntimeManageOutput { ok: false, message: format!("{name} install failed: {e}"), runtimes: Vec::new() },
            }
        };
        let mut out = Self::list().await;
        out.message = message;
        out
    }
}

/// The chromium row, derived from the driver's own resolver.
async fn chromium_row() -> RuntimeRow {
    let (status, path) = match crate::tools::probes::browser::managed_cli_path() {
        None => ("Unknown (no playwright-cli)".to_string(), None),
        Some(cli) => {
            let runtime = crate::config::Config::load()
                .map(|c| c.general.browser.runtime.clone())
                .unwrap_or_default();
            match crate::browser::chromium_resolve::resolve_binary(
                &runtime,
                &crate::browser::profile::BrowserType::default(),
                &cli,
            )
            .await
            {
                Ok((p, source)) => (format!("Ready ({})", source.label()), Some(p.display().to_string())),
                Err(e) => (format!("Missing ({e})"), None),
            }
        }
    };
    RuntimeRow {
        name: CHROMIUM.to_string(),
        status,
        path,
        version: None,
        purpose: Some(
            "The browser the managed browser driver launches. Supplied by \
             `playwright-cli install-browser chromium`, or by any system \
             Chrome/Chromium/Brave/Edge."
                .to_string(),
        ),
        supported_here: true,
    }
}

/// Run the same command the ledger's post-install action runs, with the same
/// environment.
async fn install_chromium() -> String {
    let Some(cli) = crate::tools::probes::browser::managed_cli_path() else {
        return "Cannot install chromium: playwright-cli is not provisioned yet. \
                Install that first (`runtime_manage{action:\"install\", \
                capability:\"playwright-cli\"}`), which also installs chromium."
            .to_string();
    };
    let mut cmd = tokio::process::Command::new(&cli);
    cmd.args(CHROMIUM_INSTALL_ARGS)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    for (key, value) in crate::runtimes::post_install::config_env(&[
        EnvFromConfig::PlaywrightDownloadHost,
    ]) {
        cmd.env(key, value);
    }
    use crate::utils::no_window::NoWindow;
    match tokio::time::timeout(CHROMIUM_INSTALL_TIMEOUT, cmd.no_window().output()).await {
        Ok(Ok(out)) if out.status.success() => "chromium installed.".to_string(),
        Ok(Ok(out)) => format!(
            "chromium install failed (exit {}): {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Ok(Err(e)) => format!("chromium install could not run: {e}"),
        Err(_) => format!(
            "chromium install did not finish within {}s; if this network blocks \
             Playwright's CDN, set [browser.runtime] download_host to a mirror.",
            CHROMIUM_INSTALL_TIMEOUT.as_secs()
        ),
    }
}

#[async_trait]
impl crate::tools::AlephTool for RuntimeManageTool {
    const NAME: &'static str = "runtime_manage";
    const DESCRIPTION: &'static str =
        "List or install the external runtimes Aleph shells out to (node, uv, cargo, git, \
         playwright-cli, chromium). Use `list` to see what is installed and where, and \
         `install` with a `capability` when a tool has just refused because its runtime is \
         missing — the refusal names the capability. `chromium` is the browser the managed \
         browser driver launches; installing it downloads ~150 MB.";

    type Args = RuntimeManageArgs;
    type Output = RuntimeManageOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        Ok(match args.action {
            RuntimeAction::List => Self::list().await,
            RuntimeAction::Install => Self::install(args.capability).await,
        })
    }
}
```

- [ ] **注册（六处，逐处照抄邻居的形状）。**
  1. `definitions.rs` — 在 `list_models` 条目（:248-252）之后：
     ```rust
     BuiltinToolDefinition {
         name: "runtime_manage",
         description: <crate::builtin_tools::runtime_manage::RuntimeManageTool as crate::tools::AlephTool>::DESCRIPTION,
         // Reads the ledger + config from process-global paths, like `doctor`.
         requires_config: false,
     },
     ```
  2. `definitions.rs` 的 `standalone` 构造区：`runtime_manage` **不需要**注入句柄，所以给它一条**真正的构造臂**而不是 `=> None`：
     ```rust
     "runtime_manage" => Some(Box::new(
         crate::builtin_tools::runtime_manage::RuntimeManageTool::new(),
     ) as Box<dyn AlephToolDyn>),
     ```
     （放在同一个 `match` 里，位置按邻近的字母顺序；若该 `match` 有 catch-all 分支，插在它之前。）
  3. `groups.rs`：把 `"runtime_manage"` 加进含 `"doctor"` / `"tool_usage"` 的那一组（`:100-137`）——它是自管理面，且**不**在 `method_authz::OPERATOR_TOOLS` 里（这张表按它自己的模块 doc 只用于展示，不带授权含义）。
  4. `registry/struct_def.rs`：
     ```rust
     /// Runtime-manage tool instance (external runtime ledger: list + install)
     pub(crate) runtime_manage_tool: crate::builtin_tools::runtime_manage::RuntimeManageTool,
     ```
  5. `builder/constructor/mod.rs`：构造 `let runtime_manage_tool = crate::builtin_tools::runtime_manage::RuntimeManageTool::new();`，并在注册结构体字面量里加 `runtime_manage_tool,`。
  6. `registry/tool_registry_impl.rs`：
     ```rust
     "runtime_manage" => {
         Box::pin(async move { self.runtime_manage_tool.call_json(arguments).await })
     }
     ```
  7. `builder/core_tools.rs`：
     ```rust
     reg(
         tools,
         "runtime_manage",
         crate::builtin_tools::runtime_manage::RuntimeManageTool::DESCRIPTION,
         schema::<crate::builtin_tools::runtime_manage::RuntimeManageArgs>("runtime_manage"),
     );
     ```

- [ ] **跑两条棘轮，按它们印出来的数字改。** `cargo test -p alephcore --lib executor::builtin_registry::definitions` → 预期 `catalog_description_bytes_ratchet` 与 `registry_schema_bytes_ratchet` 双红。**先按 R9 的两把尺量这段 `DESCRIPTION`**：① 这是模型做不到的运行时事实吗？——是：「哪些运行时存在、`install` 要一个 `capability`、chromium 是那个浏览器」在 schema 的枚举里看不出来。② 有没有别的工具拥有这句话？——没有：`doctor` 报告健康但不装东西。**先修剪再抬**：如果实测增量超过 400 B，把描述里能由 `RuntimeAction` 枚举 doc 说出来的部分删掉再量一次。然后把两个 ceiling 常量改成测试**打印出来的**那个数（flush，不留 headroom——那条常量的 doc `:2369-2384` 逐字论证过为什么 headroom 是已经发出去的额度），并在常量 doc 里追加一行注明本轮增量与归因。同样把 `REGISTRY_SCHEMA_BASELINE`（`:3027+`）加一行 `("runtime_manage", <测出来的数>)` —— **不要手编这一行**，用测试打印的值。
- [ ] **跑到绿。** `cargo test -p alephcore --lib builtin_tools::runtime_manage executor::builtin_registry`
- [ ] **证伪一次。** 把 `is_installable` 改成只 `find_spec(name).is_some()` → `chromium_is_installable_and_is_deliberately_not_a_ledger_spec` 必须变红。恢复。
- [ ] `rustfmt src/builtin_tools/runtime_manage.rs src/builtin_tools/mod.rs src/executor/builtin_registry/definitions.rs src/executor/builtin_registry/groups.rs src/executor/builtin_registry/registry/struct_def.rs src/executor/builtin_registry/registry/tool_registry_impl.rs src/executor/builtin_registry/builder/core_tools.rs src/executor/builtin_registry/builder/constructor/mod.rs`
- [ ] `cargo test -p alephcore --lib --no-run` · `cargo test -p alephcore --bins` · `cargo test -p alephcore --features test-helpers --test '*' --no-run` 全绿。
- [ ] **提交。**
  ```
  git add src/builtin_tools/runtime_manage.rs src/builtin_tools/mod.rs src/executor/builtin_registry
  git commit -m "tools: runtime_manage puts the runtime ledger in the conversation

  The runtimes.* family had a Panel face and no tool face, so 'chromium is not
  installed' was a dead end for the model. runtime_manage lists the same spec
  table the RPC lists and installs by name. chromium is installable without
  being a RuntimeSpec: the ledger probes PATH and Playwright's browser is never
  on it, so a spec would sit at Missing forever; the install re-runs the very
  argv the playwright-cli post-install already uses, with the same mirror env.

  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01TKV5PtutzoBvbT4yTpsyRY"
  ```

---

### Task 9: 真机 QA —— `qa/browser_managed/run.sh attach` + 八场景回归

**Files:**
- Modify `qa/browser_managed/run.sh`：用法头（:1-25）· 场景白名单（:29-32）· `add_browser_config.py` 参数装配（:207-261）· 驱动分派（:350-400）
- Modify `qa/browser_managed/add_browser_config.py`：新增 `--runtime-binary-path` / `--prefer-system-browser` / `--chromium-udd-root` 三个参数与对应 `[general.browser.runtime]` 写入（照 `:126-136` 的 `set_key` 列表）
- Create `qa/browser_managed/drive_attach.py`
- Modify `qa/browser_managed/drive_browser.py`：`open` / `ambient` / `headed` 三个场景的 `user-data-dir` 预言机搬家（见下）

**这一步在证明 §6.4 的那一句话**：「Aleph 起 Chrome、端口文件出现、`attach --cdp` 接上、`browser_snapshot` 有 `[ref=eN]`；`close` 后 Chrome 仍活。」

**⚠️ 现有 `open` 场景的预言机必须搬家。** `drive_browser.py` 的 `--expect-user-data-dir` 断言来自 `playwright-cli list` 打印的 `user-data-dir:`。`attach` 之后 CLI 不再拥有 profile 目录，那一行**不可能**再报出我们的 udd（spike STEP 2 记录 CLI 自启时它打印 `<in-memory>`；attach 时它打印什么本机**未测**）。所以：
- 把 `drive_browser.py` 里那条断言改成**新的、更强的**一条：`<udd>/DevToolsActivePort` 存在且 `curl -s http://127.0.0.1:<第一行端口>/json/version` 返回 200。旧断言证明的是「CLI 转述了我们写给它的配置」，新断言证明的是「浏览器确实是用那个目录被我们起起来的」。
- 顺带**打印**一次 `playwright-cli list` 对 attach 会话的输出（`say "cli sessions at the end"` 那一段已经在做，:407-408），作为下一轮的读数记录进 Task 10。

#### Steps

- [ ] **给 `add_browser_config.py` 加三个参数。** 在 `p.add_argument("--chrome-mcp-arg", ...)`（:75）之后：
```python
p.add_argument(
    "--runtime-binary-path",
    default="",
    help="[browser.runtime] binary_path — pins the browser Aleph launches. The "
    "attach scenario pins it so the run never depends on which browsers this "
    "machine happens to have, and the RED control renames it.",
)
p.add_argument(
    "--prefer-system-browser",
    default="",
    choices=["", "true", "false"],
    help="[browser.runtime] prefer_system_browser",
)
```
  并在 `set_key` 的列表（:126-135）之后追加：
```python
if args.runtime_binary_path:
    src = set_key(src, "general.browser.runtime", "binary_path", f'"{args.runtime_binary_path}"')
if args.prefer_system_browser:
    src = set_key(src, "general.browser.runtime", "prefer_system_browser", args.prefer_system_browser)
```

- [ ] **写驱动 `qa/browser_managed/drive_attach.py`。**

```python
#!/usr/bin/env python3
"""Prove the §6.4 `launch` sentence on a real machine.

Aleph starts Chrome, the port file appears, `attach --cdp` connects,
`browser_snapshot` carries `[ref=eN]`, and `close` leaves Chrome alive.

Every claim here is one a fake backend cannot make. The two that matter most
are the ones the flip created: the port file is written into the user-data-dir
ALEPH chose (so the browser is ours, not the CLI's), and the endpoint is still
serving after `browser_profile(close)` — which under `attach --cdp` is a
disconnect, not a shutdown (measured in the spike; nine Chrome processes before
and after).
"""
import argparse
import asyncio
import json
import os
import subprocess
import sys
import urllib.request

import websockets

from qa_rpc import Ledger, Rpc, cli_sessions

ap = argparse.ArgumentParser()
ap.add_argument("url")
ap.add_argument("--page-url", required=True)
ap.add_argument("--marker", required=True)
ap.add_argument("--home", required=True, help="scratch HOME, for the CLI oracle")
ap.add_argument("--cli", required=True)
ap.add_argument("--expect-user-data-dir", required=True,
                help="where DevToolsActivePort must appear — Aleph's choice, not the CLI's")
args = ap.parse_args()

_led = Ledger()
log = Ledger.log
check = _led.check

PORT_FILE = os.path.join(args.expect_user_data_dir, "DevToolsActivePort")


def read_endpoint():
    """(port, browser_path) from Chrome's own two-line file, or None."""
    try:
        with open(PORT_FILE) as fh:
            lines = fh.read().splitlines()
    except OSError:
        return None
    if len(lines) < 2 or not lines[1].startswith("/"):
        return None
    try:
        return int(lines[0]), lines[1]
    except ValueError:
        return None


def http_json(port, path):
    with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=10) as r:
        return r.status, json.loads(r.read().decode())


def chrome_pids(udd):
    """Chrome processes carrying OUR user-data-dir. The `pgrep -f` pattern is
    the flag AND its value: a bare `Chrome` would count the developer's own
    browser and the claim would pass on any machine with Chrome open."""
    out = subprocess.run(
        ["pgrep", "-f", f"--user-data-dir={udd}"],
        capture_output=True, text=True,
    )
    return [p for p in out.stdout.split() if p.strip()]


async def main():
    async with websockets.connect(args.url, max_size=None) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-attach")

        # CONTROL. A non-launching verb must FAIL first, or every claim below is
        # satisfied just as well by a browser that was already running.
        ok, body = await rpc.invoke("browser_navigate", {"profile": "default", "url": args.page_url})
        check("a non-launching verb fails on a fresh profile (control)", not ok, json.dumps(body)[:200])
        check("no port file before anything launched (control)", read_endpoint() is None, PORT_FILE)

        # 1. Aleph starts Chrome.
        ok, body = await rpc.invoke("browser_open", {"profile": "default", "url": args.page_url})
        check("browser_open succeeds", ok, json.dumps(body)[:300])

        # 2. The port file appears, in ALEPH's user-data-dir.
        endpoint = None
        for _ in range(60):
            endpoint = read_endpoint()
            if endpoint:
                break
            await asyncio.sleep(0.5)
        check("DevToolsActivePort appeared in Aleph's user-data-dir", endpoint is not None, PORT_FILE)
        if not endpoint:
            return _led.verdict()
        port, browser_path = endpoint
        log(f"  endpoint: http://127.0.0.1:{port}{browser_path}")

        # 3. It is a real, serving CDP endpoint.
        try:
            status, version = http_json(port, "/json/version")
        except Exception as e:  # noqa: BLE001 - the failure IS the claim
            status, version = 0, {"error": str(e)}
        check("the endpoint answers /json/version", status == 200, json.dumps(version)[:200])
        check("it is a Chromium-family browser", "Chrome/" in version.get("Browser", ""),
              version.get("Browser", "<none>"))

        # 4. attach --cdp connected: the CLI can drive the page Aleph opened.
        ok, body = await rpc.invoke("browser_snapshot", {"profile": "default", "max_chars": 4000})
        text = json.dumps(body)
        check("browser_snapshot succeeds", ok, text[:200])
        check("the snapshot carries playwright refs ([ref=eN])", "[ref=e" in text, text[:200])
        check("the snapshot is of the page we asked for", args.marker in text, text[:200])

        # 5. Aleph launched it, not the CLI: a process carrying our udd exists.
        pids = chrome_pids(args.expect_user_data_dir)
        check("a Chrome process carries Aleph's --user-data-dir", len(pids) > 0, " ".join(pids))

        # 6. `close` is a DISCONNECT under attach --cdp.
        #    Driven OUT OF BAND, with the scenario's scratch HOME, because that
        #    is the same command `ProfileManager::reap_idle` runs — and because
        #    `browser_profile` has no close action (its ProfileAction is List |
        #    GetState, verified). Killing Aleph's Chromium is the reaper's other
        #    half and belongs to the `reap` scenario, not to this claim.
        closed = subprocess.run(
            [args.cli, "-s=default", "close"],
            capture_output=True, text=True, timeout=60,
            env={**os.environ, "HOME": args.home},
        )
        log("  playwright-cli close ->", (closed.stdout + closed.stderr).strip()[:200])
        await asyncio.sleep(2)
        try:
            status_after, _ = http_json(port, "/json/version")
        except Exception:  # noqa: BLE001
            status_after = 0
        check("the endpoint still serves after close (close only disconnects)", status_after == 200,
              f"status={status_after}")
        check("the Chrome processes are still there after close",
              len(chrome_pids(args.expect_user_data_dir)) > 0, "")

        # 7. And the CLI can find its way back, which is what makes a reaped or
        #    crashed CLI cost nothing.
        ok, body = await rpc.invoke("browser_snapshot", {"profile": "default", "max_chars": 2000})
        check("a later tool call re-attaches and still sees the page", ok and args.marker in json.dumps(body),
              json.dumps(body)[:200])

        log("\n  playwright-cli list (recorded, not asserted — the attach-session shape is a new reading):")
        log(cli_sessions(args.cli, args.home))

    return _led.verdict()


sys.exit(asyncio.run(main()))
```

  ⚠️ 已核对：`BrowserProfileTool` 的 `ProfileAction`（`src/builtin_tools/browser_tools/profile_tool.rs:26-34`）只有 `List` 与 `GetState`——**没有 close 动词**，所以上面第 6 步走的是带 scratch HOME 的 out-of-band `playwright-cli -s=default close`，也就是 `reap_idle` 自己发的那条命令。别把断言留给一个不存在的动词。

- [ ] **接进 `run.sh`。**
  - 用法头（:4-12）加一行：`#   ./qa/browser_managed/run.sh attach    # Aleph starts Chrome; playwright-cli joins over CDP`
  - 白名单（:30）加 `attach`。
  - 在 `BROWSER_CFG_ARGS` 装配（:207）之后加：
    ```bash
    if [ "$SCENARIO" = "attach" ]; then
      # Pin the browser so the run does not depend on which browsers this
      # machine happens to have, and so the RED control below has one thing to
      # break. `find_chromium`'s own first macOS path; on Linux/Windows set
      # ALEPH_QA_CHROME to override.
      CHROME_BIN="${ALEPH_QA_CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
      [ -x "$CHROME_BIN" ] || { echo "no browser at $CHROME_BIN; set ALEPH_QA_CHROME" >&2; exit 69; }
      BROWSER_CFG_ARGS+=(--runtime-binary-path "$CHROME_BIN" --prefer-system-browser false)
      echo "chromium pinned: $CHROME_BIN"
    fi
    ```
  - 驱动分派（:350）加一臂：
    ```bash
      attach)
        python3 "$HERE/drive_attach.py" \
          "ws://127.0.0.1:$GATEWAY_PORT/ws" \
          --page-url "http://127.0.0.1:$PAGE_PORT/" \
          --marker "$MARKER" \
          --home "$QA_ROOT/home" \
          --cli "$CLI" \
          --expect-user-data-dir "$UDD" || RC=$?
        ;;
    ```

- [ ] **搬 `drive_browser.py` 的预言机。** 把它里面用 `--expect-user-data-dir` 对 `cli_sessions(...)` 输出做的断言，改成对 `<udd>/DevToolsActivePort` + `/json/version` 的断言（把 `drive_attach.py` 的 `read_endpoint` / `http_json` 两个函数提到 `qa_rpc.py` 里共用，**不要**复制第二份——两个驱动可以对断言什么有分歧，不许对怎么读端点有分歧，这正是 `qa_rpc.py` 模块 doc 写的纪律）。同时更新 `drive_browser.py` 的 docstring「The oracle」那一段，说明为什么换了。

- [ ] **跑绿。**
  ```bash
  ./qa/browser_managed/run.sh attach
  ```
  预期 `VERDICT: PASS`，13 条 claim 全 PASS。
- [ ] **跑红（控制组）。** 把 pin 指到一个不存在的文件：
  ```bash
  ALEPH_QA_CHROME=/nonexistent/chrome ./qa/browser_managed/run.sh attach
  ```
  这会在 `[ -x "$CHROME_BIN" ]` 就退出 69——**那不是控制组，那是前置检查**。真正的控制组要绕过它：临时把 `run.sh` 里的 `[ -x ... ] || exit 69` 注释掉再跑一次，期望 `browser_open` 失败且**错误文本里出现 `playwright-cli install-browser chromium`**（`ChromiumUnavailable` 的 fail-closed 文案）。把那一行错误原样记进 Task 10。跑完恢复。
- [ ] **变异验证（两处，spec §6.4 的纪律）。**
  1. 去掉 `chromium_launch.rs` 里 `write_sidecar` 的调用 → `cargo test -p alephcore --lib browser::chromium_launch` 的 `reap_orphans_*` 两条**不会**红（它们自己写 sidecar）。**这说明单测覆盖不到那条线**——所以改为：在 `ChromiumChild::spawn` 成功返回前断言 sidecar 存在的一条新单测，或者在 `attach` 场景里加一条 claim「`<udd>/aleph-chromium.json` 存在且 pid 与 `pgrep` 的一致」。**选后者**（真机能证，单测不能），加完再做这次变异，确认 `attach` 变红。
  2. 把 `reap_idle` 里新加的 `shutdown_chromium(&name)` 去掉 → 用 `./qa/browser_managed/run.sh reap` 验证：期望在 `reap` 场景末尾新增一条 claim「被收割的 profile 的 udd 下已无 Chrome 进程」变红。**这条 claim 现在不存在**，所以本步先把它加进 `drive_tools.py` 的 `reap` 分支（复用 `chrome_pids`），再做变异。
- [ ] **八场景回归，逐个跑，逐个记结果。**
  ```bash
  for s in open ambient headed tools frames reap pdf existing exec-offload; do
    echo "=== $s ==="; ./qa/browser_managed/run.sh "$s"; echo "rc=$?"
  done
  ```
  ⚠️ `reap` 约 3 分钟，`existing` 需要 `$REAL_HOME/.npm/_npx` 里有 `chrome-devtools-mcp` 缓存，`exec-offload` 需要 mock provider。**任何一条不能跑的，写下它为什么不能跑，不要报告成通过**——第四、五两轮的 11 个缺陷全部对 `FakeBackend` 结构性不可见，「单测全绿」在这一族上从来不是交付证据。
- [ ] **提交。**
  ```
  git add qa/browser_managed/run.sh qa/browser_managed/add_browser_config.py \
          qa/browser_managed/drive_attach.py qa/browser_managed/drive_browser.py \
          qa/browser_managed/drive_tools.py qa/browser_managed/qa_rpc.py
  git commit -m "qa/browser_managed: an attach stage, and a stronger launch oracle

  The new stage proves the launch sentence end to end: Aleph starts Chrome, the
  port file appears in the user-data-dir Aleph chose, attach --cdp connects,
  browser_snapshot carries [ref=eN], and close leaves the endpoint serving.
  The open/ambient/headed oracle moves off 'playwright-cli list echoes our
  user-data-dir' — under attach the CLI does not own the profile dir — onto
  DevToolsActivePort plus a live /json/version, which proves the browser rather
  than the CLI's copy of our config.

  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01TKV5PtutzoBvbT4yTpsyRY"
  ```

---

### Task 10: 文档 —— FEATURE_LOCATOR §3.12 新一轮 · SECURITY.md 一段 · qa/README.md 一行

**Files:**
- Modify `docs/reference/FEATURE_LOCATOR.md`：§3.12 末尾（现在到 `:1337`，`### 3.13` 在 `:1339`）追加第七轮条目；`### 3.12` 的「代码锚点」行（`:1258`）补两个新文件
- Modify `docs/reference/SECURITY.md`：在 `### 内嵌终端（`pty.*`）` 之后（`:1062` 起那一节的末尾、`## Command allowlist`（`:1114`）之前）加一小节
- Modify `qa/README.md`：`:19-27` 的场景清单加一行；`:1005` 的「改哪里跑哪个」表补一句

#### Steps

- [ ] **FEATURE_LOCATOR §3.12 的代码锚点行**（`:1258`）：在 `discovery.rs`（引擎偏好发现）之后插入 ``、`chromium_launch.rs`（Aleph 自己 spawn 的 Chromium + 端口文件 + sidecar + 孤儿回收）、`chromium_resolve.rs`（钉住>系统>Playwright 自带的二进制解析）``。
- [ ] **追加第七轮条目**（插在 `:1337` 之后、`### 3.13` 之前），照既有轮次的行文与缩进：

```markdown
- **第七轮（2026-09-05）——启动链翻转：Aleph 起 Chromium，`playwright-cli` 只 `attach`**。承接 spec `docs/superpowers/specs/2026-09-05-browser-live-view-design.md` §3.1/§6.1 的交付第一步（视图不在本轮）。
  - **① [Headline] 一个「已经有了但没人说得出口」的能力，和一条不能当契约的能力。** playwright-cli 起的 Chrome **本来就**带一个随机 `--remote-debugging-port`（无需任何配置，实测 `curl /json/version` 200）——但它是随机的、你自己的 `--remote-debugging-port=0` 会被它自己的覆盖（Chrome 取最后一个），**不写** `DevToolsActivePort`，`playwright-cli list` 也不打印端点。唯一的发现路径是刮 `ps`。所以翻转方向：**Aleph 自己 spawn**，端口从 `<user_data_dir>/DevToolsActivePort` 读，CLI 用 `attach --cdp <http-url>` 接入。判据：一个能力**存在**不等于它是**契约**；问「这个值有没有一个我能读的发布点」。
  - **② `open` 会 clobber 它复用的页面，`attach` 不会。** `open --config {"browser":{"cdpEndpoint":…}}` 被接受、复用现有页面，然后对它 `goto('about:blank')`——静默清掉页面上的一切。`attach --cdp`（http 与 ws 两种形式都接受）原样保留。所以 `open_argv` 整块删除，本仓从此不对一个交接过来的浏览器发 `open`。
  - **③ 生命周期跟着**「谁 spawn 的」**走。** CLI 自启时 `close` 让 `pgrep "Google Chrome"` 归零；`cdpEndpoint` 下 `close` 之后九个 Chrome 进程原样、端点仍服务、页面仍停在原 URL。于是 `reap_idle` 的 Managed 臂**长出第二半**：`close` 之后还要杀 Aleph 自己的子进程，只到 `close` 为止的收割器会**报告收割成功而浏览器永远不走**（判据 §11「报成功的 no-op」）。
  - **④ 一个近似值在有了精确答案之后必须删掉。** `session_active` / `idle_managed_profiles` 原本都用 `TabRegistry::has_tabs`，其 doc 自己写着「Approximation」。浏览器成了我们的子进程之后「有没有浏览器」有了确切答案；留着近似就是同一个问题的两个答案（判据 §1）。`reap_idle_tabs` 的候选筛选**继续**用 `has_tabs`——那问的是 tab 不是浏览器。
  - **⑤ 一个恒空的清单。** `unhonored_managed_fields` 唯一的返回条件是 `browser_flag_value(&cfg.browser).is_none()`；`attach` 不收 `--browser`，`browser_flag_value` 随之删除，于是那个函数恒返回空 —— 判据 §2 的第二张脸（恒绿）。连同 `browser_flag_value`、`open_argv` 与它们的四条测试一起 CUT。引擎偏好没有丢：它现在喂给 `discovery::find_chromium_preferred`，而 Brave **从此是被honor的**（我们自己起它），不再需要那条启动告警。
  - **⑥ 第三种「拒绝」的措辞（接 附录 D.9.13）。** 惰性启动的触发从两句「没开」变成三句：新增的一句是 attach 被拒。实测（0.1.8 / node 24.14.1）：退出 1、**stdout 空**、stderr 是 node 异常 `Error: connect ECONNREFUSED 127.0.0.1:1` 加一行 `- <ws preparing> retrieving websocket url from http://127.0.0.1:1`。它与两句旧锚点**零公共子串**，所以分类器新增一支不会遮蔽旧的；两条新锚点都留着，因为第四种措辞更可能像其中之一。分类结果**不是** `NoSession`（那会朝同一个死端点再 attach 一次），是 `AttachFailed` → 忘掉子进程 → 重启一次 → 再 attach，界限恰好一次。
  - **⑦ 一个平台路径表，问装它的人比自己猜便宜。** 提议里的解析器要硬编码 `~/.cache/ms-playwright/chromium-*/`。本机实测两处都是错的：macOS 的缓存根是 `~/Library/Caches/ms-playwright/`，可执行文件叫 `chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing`（不是 `Chromium.app`）。改为跑 `playwright-cli install-browser chromium --dry-run`，它逐行打印 `Install location:`；再在那**一个**目录里按候选名找可执行文件。判据 §1：装它的那个二进制既装它又说它在哪，只有一份推导；三张平台表会各自腐烂。⚠️ 解析要锚在段头 `(playwright chromium v`——`chromium-headless-shell` 以 `chromium` 开头，子串匹配会选中一个没有浏览器的目录。
  - **⑧ 台账早就在装 Chromium，只是名字变过。** spec §9 曾把「现有运行时台账是否已经会跑 `playwright install`」列为未验证。答案是**会**：`src/runtimes/specs.rs` 里 `playwright-cli` 的 post-install 就是 `install-browser chromium`（v0.1.14 改的名）。所以本轮**没有**新增 `chromium` capability——`runtimes::probe` 只走 PATH，而 Playwright 的浏览器永远不在 PATH 上，加一条 spec 只会得到一个恒 `Missing` 的条目（判据 §2 第四张脸：没装上）。做的是给那条已有动作加 `PLAYWRIGHT_DOWNLOAD_HOST` 透传（空串读作「没有镜像」，不是「镜像是空主机」）、一个 doctor 哨兵 `browser/chromium-missing`（**跑与启动路径同一个解析器**，判据 §9），与一个 R8 工具面 `runtime_manage{list,install}`（`runtimes.*` RPC 家族此前只有 Panel 一张脸）。
  - **⑨ 一个行为变更，写下来而不是留给人发现：Managed profile 不能再「在内存里」。** `DevToolsActivePort` 写在 user-data-dir 里，没有 profile 目录就没有可发现的端点。所以没配 `user_data_dir` 的 profile 现在拿到一个派生目录 `~/.aleph/data/browser/chromium-udd/<key>`，浏览状态（cookie / localStorage）从此对**每个** Managed profile 跨重启存活，而不只是主动要求过的那些。
  - **⑩ [装置] `qa/browser_managed/run.sh attach`，以及一条必须搬家的预言机。** 新阶段证明 §6.4 的 `launch` 一句。同时 `open`/`ambient`/`headed` 三个场景原来的预言机——`playwright-cli list` 打印出我们的 `user-data-dir`——在 attach 之后不可能再成立（CLI 不拥有那个目录）。搬到 `<udd>/DevToolsActivePort` + `curl /json/version`，**更强**：旧的证明「CLI 转述了我们写给它的配置」，新的证明「浏览器确实是用那个目录被我们起起来的」。
  - **验证**：<在此逐字填入本轮实测：`cargo test -p alephcore --lib` 的 passed/failed 数与失败项的 HEAD 复验结论；`aleph-server doctor` 里 chromium 那一行的原文；`run.sh attach` 的 claim 数；八场景逐个的 rc 与不能跑的那些的理由；两条 ceiling 常量的新旧值与逐工具归因>。
```

  ⚠️ 最后一行的尖括号占位**必须**在提交前用真实读数替换。一条写着数字却没测过的验证行，正是判据 §18 说的那种量具。

- [ ] **SECURITY.md 新增小节**（放在 `### 内嵌终端（`pty.*`）` 那一节之后）：

```markdown
### The managed browser's debug port (2026-09-05)

Aleph now launches the managed driver's Chromium itself, with
`--remote-debugging-port=0`, and hands the resulting endpoint to
`playwright-cli attach --cdp`. Chrome binds that port to **loopback only**, and
it is **unauthenticated**: any process running as this user can connect to it
and drive the browser — read its cookies, navigate it, execute script in its
pages. There is no token, and CDP has no concept of one.

This is **not a regression**. Before the flip, `playwright-cli` was already
launching Chrome with an unprompted random `--remote-debugging-port` on every
single launch (measured, spike STEP 2: launching with no config at all produced
`--remote-debugging-port=58447`, which answered `/json/version` with a live
`webSocketDebuggerUrl`). The difference is that Aleph now *knows* the port
instead of the port existing and being undiscoverable. Stating it here rather
than leaving it implicit: the security boundary for the managed browser is the
**local user account**, exactly as it is for the PTY sessions above and for
`aleph.lock`. A host where an untrusted local process runs is already outside
Aleph's trust model.

What follows from that, and is therefore NOT attempted: no per-connection auth
on the debug port (CDP has none), no binding it to a unix socket (Chrome does
not offer one), and no attempt to hide the port — the sidecar file
`<user_data_dir>/aleph-chromium.json` records it on purpose, because a browser
Aleph cannot find after a crash is a browser Aleph cannot kill.
```

- [ ] **qa/README.md**：`:19-27` 的清单里，在 `open` 那一行**之前**插入
  ```
  ./qa/browser_managed/run.sh attach   # Aleph starts Chrome; playwright-cli joins over CDP
  ```
  并在 `:1005` 的「`browser_managed` — 改 `src/browser/` 或 `src/builtin_tools/browser_tools/` 前跑」那一行后面补：「**改启动链（`chromium_launch` / `chromium_resolve` / `playwright_launch` / `playwright_cli`）必须跑 `attach`**——它是唯一证明「Aleph 起的浏览器」而不是「某个浏览器」的阶段。」
- [ ] **最终验证集全跑一遍**（CLAUDE.md 六条）：
  ```bash
  cargo test -p alephcore --lib --no-run
  cargo test -p alephcore --bins
  cargo test -p alephcore --features test-helpers --test '*' --no-run
  cargo test -p aleph-panel --lib --no-run
  just _stage-shell-placeholders && cargo clippy --workspace --all-targets
  ```
  再 `cargo test -p alephcore --lib` 跑一次全量，把 passed/failed 数抄进上面的「验证」行；**每一条红的都要先把 src 暂存回 HEAD 复验**，确认是不是本轮引入的。
- [ ] **提交。**
  ```
  git add docs/reference/FEATURE_LOCATOR.md docs/reference/SECURITY.md qa/README.md
  git commit -m "docs: record the launch-chain flip round (FEATURE_LOCATOR 3.12)

  Ten findings, the measurements behind each, and the two behaviour changes a
  reader would otherwise discover the hard way: a managed profile can no longer
  be in-memory, and the open/ambient/headed QA oracle moved. SECURITY.md states
  the unauthenticated loopback debug port and why it is not a regression.

  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01TKV5PtutzoBvbT4yTpsyRY"
  ```

---

## Self-review notes

按要求做了四轮自查，改掉的东西：

1. **Spec 覆盖对照。** §3.1 → Task 1/4/5（spawn + 端口文件 + `attach --cdp` 永不 `open` + 生命周期归 Aleph + 惰性 re-attach）与 Task 6（收割器杀浏览器）。§3.2 的访问器 → Task 5 的 `PlaywrightCliDriver::endpoint` + Task 6 的 `ProfileManager::live_endpoint`（**只有访问器，没有视图**）。§6.1 → Task 2（三个配置键）+ Task 3（解析顺序 + fail-closed 文案）+ Task 7（`download_host` 透传 + doctor 哨兵）+ Task 8（R8 工具）。§6.2 的四行 → 端口超时 = Task 1 的 `LaunchFailed{stage:"devtools-port"}`；Chrome 中途死 = Task 5 `ensure_chromium` 的 `alive()` 分支惰性重启；CLI 崩/被收割 = Task 5 的 `AttachFailed` → 重启一次 → 再 attach；Chromium 未装 = `ChromiumUnavailable` + doctor + 工具。§6.3 → Task 2（`[browser.live]` 三键属于 Plan 2，本计划不加）。§6.4 的 `launch` 一句 → Task 9。§6.5 第 1 步的「八场景回归」→ Task 9 最后一步。
2. **占位符扫描。** 全文无 `TBD` / 「类似 Task N」/「加上错误处理」。仅有的两个尖括号占位在 Task 10 的「验证」行与两条 ceiling 数值，都**必须由实测填入**，并各自带了一句「不许抄，要测」的说明——这是判据 §18 要的形状，不是占位符。
3. **跨任务签名一致性（改过三处）。** ① 初稿 Task 5 写 `PlaywrightCliDriver::new(config)` 不变、把 runtime config 从 `SessionLaunch` 取——但 `SessionLaunch`（`playwright_launch.rs:35-41`）只有五个字段且是 per-profile 的，`BrowserRuntimeConfig` 是全局的，改成构造参数。② 初稿 Task 6 让 `ProfileManager` 自己持有 `HashMap<String, ChromiumChild>`，与 Task 5 的 per-session 锁重复，改为 driver 持有 + manager 转发（前言 §3）。③ 初稿 Task 3 的 `resolve_binary` 返回 `PathBuf`，而 Task 7 的 doctor finding 要说出「哪条路线赢了」，改成返回 `(PathBuf, ChromiumSource)` 并加 `ChromiumSource::label()`，Task 5 的日志与 Task 8 的 `chromium_row` 都用它。
4. **锚点复核。** 全文引用的 `path:line` 都在本次会话里读过：`playwright_launch.rs` 与 `playwright_cli.rs` 的行号是 `grep -n` 逐符号取的；`manager.rs` / `profile.rs` / `error.rs` / `discovery.rs` / `specs.rs` / `post_install.rs` / `probe.rs` / `ensure.rs` / `browser_runtime.rs` / `definitions.rs` / `groups.rs` / `struct_def.rs` / `tool_registry_impl.rs` / `core_tools.rs` / `constructor/mod.rs` / `run.sh` / `add_browser_config.py` / `qa_rpc.py` / `drive_browser.py` / `FEATURE_LOCATOR.md` / `SECURITY.md` / `qa/README.md` 都是带行号读过的。自查中发现的**两处待确认符号都在保存前解决了**：`AlephHomeEnvGuard` 的真实路径是 `crate::utils::paths::AlephHomeEnvGuard`（`src/tasks/cron/mod.rs:819`）；`BrowserProfileTool` 的 `ProfileAction`（`profile_tool.rs:26-34`）**没有** close 动词，只有 `List` 与 `GetState`，所以 Task 9 的「close 只是断开」改由 out-of-band `playwright-cli -s=default close` 驱动——正是 `reap_idle` 自己发的那条命令。引用一个我没读过的符号，会把「没测过的断言」写成计划。
5. **两条我改掉的判据错误。** ① 初稿让 `ChromiumChild::alive` 把 `try_wait` 的 `Err` 读成「死了」，那是把「我不知道」当值花掉（判据 §8），且方向是杀掉一个活着的浏览器；改成读成「活着」，由随后的 attach 结算。② 初稿的 doctor 检查在读不到配置时回落到 `BrowserRuntimeConfig::default()`，那会让一个配了 `binary_path` 的主机被报成「没有浏览器」；改成 `unknown_finding`。
