# Browser Live View — Plan 1: Managed Launch-Chain Flip Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal（评审后修订版，2026-09-05）：** 把 `BrowserDriver::Managed` 的启动链翻转过来——Aleph 自己 spawn Chromium（`--remote-debugging-port=0`），从 `<user_data_dir>/DevToolsActivePort` 读出 CDP 端点，`playwright-cli` 只以 `attach --cdp <http-url>` 接入（永不 `open`）；浏览器进程的生命周期归 Aleph，`playwright-cli close` 退化成断开，惰性 `open` 变成惰性 `attach`。同时把 Chromium 作为外部运行时供给（配置钉住 > 系统浏览器 > Playwright 自带），补 fail-closed 文案、doctor 哨兵与 R8 安装工具面，并给现有真机装置 `qa/browser_managed/run.sh` 加一个 `attach` 阶段。**本计划不做视图**：只交付 §3.2 的 `live_endpoint(profile)` 访问器，`src/browser/live/`、`qa/browser_live/`、`browser_control` 工具都不在范围内。

**Architecture:** `ChromiumLaunchSpec` → `ChromiumChild`（`std::process::Child` + `CdpEndpoint` + sidecar 文件）住在 `PlaywrightCliDriver` 的 per-session 映射里，与惰性启动咽喉、per-session 锁同处一地；`ProfileManager` 只多三个转发访问器（`live_endpoint` / `session_active` / `reap_idle` 的 Managed 臂）。二进制解析走 `chromium_resolve`：配置钉住 > `discovery::find_chromium_preferred` > 问 `playwright-cli install-browser <b> --dry-run` 要 `Install location:` 再在那一个目录里找可执行文件。运行时供给复用台账**已有的** `playwright-cli` post-install 动作（`install-browser chromium`），只加 `PLAYWRIGHT_DOWNLOAD_HOST` 透传；新增 doctor 哨兵 `browser/chromium-missing` 与 R8 工具 `runtime_manage{list,install}`。

**Tech Stack:** Rust (alephcore) · tokio · serde / schemars · `std::process::Command`（子进程，`NoWindow` 扩展）· `sysinfo`（只经 `utils::process_alive::with_process_specifics`，本仓单 pid `sysinfo` 惯用法的唯一所有者；**不经** `gateway::pty::foreground::fact_for_pid`，理由见 Task 1）· bash + python3（`qa/browser_managed/`）。**零新 crate**。

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

## 前言：与提议结构的十一处偏离（每一处都是代码逼出来的）

1. **`ProfileManager` 构造点不是 `builder/subsystems.rs`**，是 `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:509-513`。而「每个被服务的 manager 恰好跑一次」的 boot 钩子是 `spawn_idle_reaper`，唯一生产调用点在 `src/executor/builtin_registry/builder/constructor/mod.rs:639`。所以 boot 时的孤儿回收挂进 `spawn_idle_reaper`（Task 6），不挂在 builder 里——那一处的论证（manager.rs:202-207）逐字说了为什么它是那个钩子。
2. **macOS 的 Playwright 浏览器缓存不是 `~/.cache/ms-playwright/`**，是 `~/Library/Caches/ms-playwright/`；而且可执行文件不是 `Chromium.app/Contents/MacOS/Chromium`，本机实测是 `chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing`。**所以 `chromium_resolve` 不硬编码缓存路径**，改为问装它的那个 CLI：`playwright-cli install-browser chromium --dry-run` 逐行打印 `Install location:`（本机实测输出逐字抄在 Task 2 里）。同一个二进制既装它又说它在哪，这是判据 §1 要的单一推导；硬编码三条平台路径则是三份会腐烂的表述。
3. **`ChromiumChild` 的映射住在 `PlaywrightCliDriver`，不是 `ProfileManager`。** 惰性启动咽喉在 `playwright_cli.rs:197-221` 的 `run()`，而它已经在 per-session 锁（`:205-206`）下；把子进程映射放到别处等于把这把锁解决掉的竞态重新引进来。`ProfileManager` 保留 `live_endpoint` / `session_active` / `reap_idle` 三个**转发**访问器。
4. **`playwright-cli attach` 接受 `--config`——已实测，不是两个分支。** 本机 0.1.8 的 `attach --help` 逐字列出 `--config  path to the configuration file, defaults to .playwright/cli.config.json`。spec §9 那条未验证项就此关闭，Task 4 只写一个分支。
5. **不新增 `chromium` 台账 capability。** `RuntimeSpec` 的探测是 PATH-only（`src/runtimes/probe.rs:65-83` 只走 `find_on_path`），而 Playwright 的 Chromium 永远不在 PATH 上——加一条 spec 只会得到一个**永远 Missing** 的条目，每次 `ensure_capability` 都重装。而供给本来就已经在了：`src/runtimes/specs.rs:173-176` 的 `PostInstallAction::RunSubcommand { args: &["install-browser", "chromium"] }` 是 `playwright-cli` 的 post-install。所以 Task 7 做的是**给这条已有动作加 `PLAYWRIGHT_DOWNLOAD_HOST` 透传** + doctor 哨兵 + fail-closed 文案，Task 8 做 R8 工具面。这同时回答了 spec §9 的第二条未验证项：**台账已经会装 Chromium**，只是叫 `install-browser` 不叫 `install`（v0.1.14 改的名，specs.rs:166-172 记着）。
6. **提议的 Task 7 拆成两个任务（7 与 8）。** 注册一个新工具要动六处（`definitions.rs` 的条目 + `REGISTRY_SCHEMA_BASELINE` 行 + `standalone` 的 `=> None` 臂、`groups.rs`、`registry/struct_def.rs`、`builder/constructor/mod.rs`、`registry/tool_registry_impl.rs`、`builder/core_tools.rs`）外加一条描述字节棘轮（`CATALOG_DESCRIPTION_CEILING_BYTES = 113_719`）。塞进供给任务里会让「哪一步红了」不可归因。**全计划共 10 个任务。**
7. **三处代码在翻转后失去被引用者，本计划 CUT 它们，但第三处的理由不是「恒真」**：`playwright_launch::open_argv`（:207-234）与 `playwright_launch::browser_flag_value`（:106-123；`attach` 不收 `--browser`/`--headed`，引擎选择改由 `chromium_resolve` 承担）确实成了死码。`manager::unhonored_managed_fields`（:580-591）**不是**恒空——它的真实谓词是 `browser_flag_value(&cfg.browser).is_none() && cfg.browser != BrowserType::default()`（`manager.rs:585-587`），也就是**恰好 Brave**，删掉 `browser_flag_value` 之后这个条件仍然表达得出来。它被删是因为它保护的那件事**换了地方**：那条启动告警存在的意义是「一个 managed Brave profile 不要静默拿到 Chromium」，而翻转之后引擎由 `chromium_resolve` 兑现。⚠️ 只说「我们自己起 Brave 所以 honor 了」是**半个替换**：`find_chromium_preferred`（`discovery.rs:125-150`）在首选引擎不存在时**静默降级**（`prefer_paths` 只是重排，回落只打 `debug!`）。所以本轮把告警搬进解析器本身——`resolve_binary` 返回**实际解析出的引擎**，与 profile 请求的引擎不一致时 `warn!`（判据 §1：一份推导；判据 §16：修在一边的判据要主动搬过去）。
8. **STRUCTURAL：sidecar 不放在各自的 user-data-dir 里，放在一个注册表目录。** 初稿把 `aleph-chromium.json` 写进每个 profile 的 udd，于是 boot 清扫只能扫「派生出来的那个根」，而配了 `user_data_dir` 的 profile（本仓 QA 自己就配，`qa/browser_managed/run.sh:85`）的 sidecar 落在根之外，**永远扫不到**。改为一个注册表：`browser_state_dir("chromium")/<sanitize_session_key(profile)>.json`，每份记 `{pid, http_url, user_data_dir, aleph_version}`。清扫只走那一个目录，「有哪些浏览器要收」这件事就只有一个推导点（判据 §12）。
9. **STRUCTURAL：守护进程优雅退出时要杀掉自己的 Chromium（spec §3.6「退出时杀」）。** `std::process::Child` 不在 drop 时 kill，初稿也没有任何 shutdown 钩子——每次重启都留一个浏览器，等下一次 boot 清扫。本轮在 `src/bin/aleph-server/commands/start/mod.rs:3642` 之后的有序停机段落里加一次 `browser::manager::shutdown_browsers_global()`，与同段 `:3658` 的 `bash_exec::kill_all_running_background()` **同形同理由**（那一行的注释逐字写着为什么 `kill_on_drop` 不够、为什么必须显式调用、为什么放在这里而不是信号处理器里）。
10. **`session_active` 与 `idle_managed_profiles` 从 `TabRegistry::has_tabs` 改成子进程活性。** 现状两处都用 `has_tabs`，且 manager.rs:355-357 逐字承认那是「近似」。翻转后「有没有浏览器」有了一个确切答案（我们自己的 `Child`），留着近似就是同一个问题的第二个答案（判据 §1）。`reap_idle_tabs` 的候选筛选**继续**用 `has_tabs`——那问的是 tab 不是浏览器。
11. **现有 `open` 场景的预言机会失效，Task 9 负责搬家。** `qa/browser_managed/drive_browser.py` 用 `playwright-cli list` 打印的 `user-data-dir` 证明「Aleph 生成的 `--config` 真的到了浏览器」。`attach` 之后 CLI 不再拥有 profile 目录，那一行不可能再报出我们的 udd。新的预言机是 `<udd>/DevToolsActivePort` 存在 + `curl http://127.0.0.1:<port>/json/version` 200——**更强**，因为它证明的是浏览器确实被我们用那个 udd 起起来了，而不是 CLI 转述了一遍我们写给它的配置。

---

### Task 1: `ChromiumLaunchSpec` / `ChromiumChild` / `CdpEndpoint` —— Aleph 自己起 Chromium

**Files:**
- Create `src/browser/chromium_launch.rs`
- Modify `src/browser/mod.rs:1-21`（模块声明；现有 21 行全部读过）
- Modify `src/browser/error.rs:5-6`（`LaunchFailed(String)` → 带 `stage` 的结构变体）与它现存的四个构造点 `src/browser/chrome_mcp.rs:135, 578, 603, 609`、一个测试点 `src/diagnostics/checks/browser_runtime.rs:480`

**Interfaces:**
- Consumes: `crate::utils::no_window::NoWindow`（`src/utils/no_window.rs:32-51`，`std::process::Command` 与 `tokio::process::Command` 都实现了）· `crate::security::secret_env::is_secret_env`（`playwright_cli.rs:19` 已在用）· `crate::utils::process_alive::with_process_specifics`（`src/utils/process_alive.rs:126-131`，`pub(crate)`，本仓**唯一**的单 pid `sysinfo` 惯用法所有者）
- **不消费** `gateway::pty::foreground::fact_for_pid`。它是 pty 侧的类型，契约不同：`ForegroundFact::cmdline` 的 doc 逐字写着「The whole command line, **space-joined**」（`src/gateway/pty/foreground.rs:142-143`），由 `cmd.iter().map(to_string_lossy).collect::<Vec<_>>().join(" ")` 造出（`:266-273`）。在那个字符串上做匹配只能是 `str::contains`——一次子串扫描，而这里的动作是 SIGKILL。见下方「为什么读 argv 向量而不是那一行字符串」。
- Produces:
  ```rust
  pub(crate) const DEVTOOLS_PORT_DEADLINE: std::time::Duration;
  pub(crate) const SIDECAR_EXT: &str;                        // "json"
  pub(crate) struct ChromiumLaunchSpec { pub binary: PathBuf, pub user_data_dir: PathBuf,
                                          pub headless: bool, pub proxy: Option<String>,
                                          pub extra_args: Vec<String> }
  impl ChromiumLaunchSpec { pub(crate) fn argv(&self) -> Vec<String>; }
  pub(crate) struct CdpEndpoint { pub http_url: String, pub ws_url: String, pub pid: u32 }
  pub(crate) fn parse_devtools_active_port(text: &str) -> Option<(u16, String)>;
  pub(crate) fn endpoint_from_port_file(text: &str, pid: u32) -> Option<CdpEndpoint>;
  pub(crate) struct ChromiumSidecar { pub pid: u32, pub http_url: String,
                                      pub user_data_dir: PathBuf, pub aleph_version: String }
  pub(crate) fn sidecar_registry_dir() -> Result<PathBuf, BrowserError>;
  pub(crate) fn sidecar_path(session_key: &str) -> Result<PathBuf, BrowserError>;
  pub(crate) struct ChromiumChild { /* child: Child, endpoint: CdpEndpoint,
                                       user_data_dir: PathBuf, session_key: String */ }
  impl ChromiumChild {
      pub(crate) async fn spawn(spec: &ChromiumLaunchSpec, session_key: &str, deadline: Duration)
          -> Result<Self, BrowserError>;
      pub(crate) const fn endpoint(&self) -> &CdpEndpoint;
      pub(crate) fn alive(&mut self) -> bool;
      pub(crate) fn shutdown(self);
  }
  pub(crate) enum ArgvProbe { Absent, Unreadable, Argv(Vec<String>) }
  pub(crate) fn argv_names_dir(argv: &[String], dir: &Path) -> bool;
  pub(crate) fn reap_orphans(registry: &Path,
                             argv_of: &dyn Fn(u32) -> ArgvProbe,
                             kill: &dyn Fn(u32)) -> usize;
  pub(crate) fn reap_orphans_now() -> usize;
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
    fn the_sidecar_round_trips_and_records_the_dir_it_is_not_stored_in() {
        let json = serde_json::to_string(&ChromiumSidecar {
            pid: 4242,
            http_url: "http://127.0.0.1:58363".into(),
            user_data_dir: PathBuf::from("/tmp/explicit-udd"),
            aleph_version: env!("ALEPH_VERSION").to_string(),
        })
        .expect("serialize");
        let back: ChromiumSidecar = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.pid, 4242);
        assert_eq!(back.http_url, "http://127.0.0.1:58363");
        // The whole point of the registry: the record lives in ONE directory
        // and names the udd, instead of living IN the udd where a profile that
        // configures its own path puts it outside anything a sweep walks.
        assert_eq!(back.user_data_dir, PathBuf::from("/tmp/explicit-udd"));
        assert_eq!(back.aleph_version, env!("ALEPH_VERSION"));
    }

    /// The containment property the registry inherits from `config_path_for`:
    /// one component under the state dir, whatever the profile is called.
    #[test]
    fn a_sidecar_path_is_one_component_under_the_registry() {
        let dir = sidecar_registry_dir().expect("home resolves");
        for hostile in ["default", "../../etc/passwd", "/etc/passwd", "..", "", "a/b"] {
            let p = sidecar_path(hostile).expect("home resolves");
            assert_eq!(p.parent(), Some(dir.as_path()), "escaped with {hostile:?}");
            assert_eq!(
                p.components().count(),
                dir.components().count() + 1,
                "not a single component for {hostile:?}"
            );
        }
    }

    /// Convenience: the argv vector a real process table yields.
    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_string()).collect()
    }

    /// The match is **token equality over the argv vector**, never a substring
    /// scan over a joined line, and both halves of that sentence are load-bearing
    /// because the action this predicate authorises is SIGKILL.
    ///
    /// The junk in these vectors is not invented. `crates/agent-detect/src/engine.rs:938-957`
    /// pins a VERBATIM reading from this machine in which an exported variable
    /// whose value contains spaces scattered the bare words `prefer`, `modern`
    /// and `like` into `sysinfo::cmd()` — macOS lets a process that rewrites
    /// its title (every Node CLI does) leak past the argv region into the
    /// environment (`:427-431`). That module's defence is tokenize-and-skip
    /// assignments; this one is the same shape, and 判据 §16 says the twin's
    /// answer gets carried over rather than rediscovered.
    #[test]
    fn the_udd_match_is_token_equality_over_argv() {
        let dir = Path::new("/tmp/udd/default");

        // (1) The real flag, with a macOS env bleed sitting beside it.
        assert!(argv_names_dir(
            &argv(&[
                "/x/chrome",
                "--user-data-dir=/tmp/udd/default",
                "--headless=new",
                "about:blank",
                "ZSH_AI_PROMPT_EXTEND=Always",
                "prefer",
                "modern",
                "CLI",
                "tools",
                "like",
                "ripgrep,",
                "fd,",
                "and",
                "bat.",
            ]),
            dir
        ));

        // Chrome accepts the two-token form too, so a browser someone launched
        // that way is still ours.
        assert!(argv_names_dir(
            &argv(&["/x/chrome", "--user-data-dir", "/tmp/udd/default"]),
            dir
        ));

        // (2) THE SIBLING PREFIX. `reap_orphans` walks profiles under one root,
        // so the flags it builds are prefixes of one another — and
        // `sanitize_session_key` produces prefix-related names routinely
        // (`work` / `work-archive`). A substring test kills the neighbour's
        // live browser, which is precisely the case the argv check exists to
        // prevent, failing on its most likely neighbour.
        assert!(!argv_names_dir(
            &argv(&["/x/chrome", "--user-data-dir=/tmp/udd/default-2"]),
            dir
        ));
        // …and the two-token form of the same trap.
        assert!(!argv_names_dir(
            &argv(&["/x/chrome", "--user-data-dir", "/tmp/udd/default-2"]),
            dir
        ));

        // (3) The whole flag string appearing INSIDE a bled-in env value.
        assert!(!argv_names_dir(
            &argv(&[
                "/usr/bin/vim",
                "notes.txt",
                "LAST_CMD=chrome --user-data-dir=/tmp/udd/default",
            ]),
            dir
        ));

        // The path as some other flag's value; a recycled pid; nothing at all.
        assert!(!argv_names_dir(
            &argv(&["/x/chrome", "--crash-dumps-dir=/tmp/udd/default"]),
            dir
        ));
        assert!(!argv_names_dir(&argv(&["/usr/bin/vim", "/tmp/udd/default/notes.txt"]), dir));
        assert!(!argv_names_dir(&[], dir));
    }

    /// Fixture: a registry directory holding one sidecar per profile.
    fn registry_with(entries: &[(&str, u32, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (profile, pid, udd) in entries {
            std::fs::write(
                dir.path().join(format!("{profile}.json")),
                serde_json::to_string(&ChromiumSidecar {
                    pid: *pid,
                    http_url: "http://127.0.0.1:1".into(),
                    user_data_dir: PathBuf::from(udd),
                    aleph_version: env!("ALEPH_VERSION").to_string(),
                })
                .expect("serialize"),
            )
            .expect("write");
        }
        dir
    }

    /// The whole point of reading argv before killing: a pid recorded hours ago
    /// may belong to somebody else's process now. "The sidecar named this pid"
    /// is not evidence; "the process still carries OUR user-data-dir" is.
    ///
    /// Four sidecars, four outcomes, one sweep. The SIBLING PREFIX
    /// (`recycled` vs `recycled-2`) is the one a substring test gets wrong:
    /// `reap_orphans` walks profiles under one root, so the flags it builds are
    /// prefixes of one another by construction, and `sanitize_session_key`
    /// produces prefix-related names routinely (`work` / `work-archive`).
    #[test]
    fn reap_orphans_kills_only_the_process_that_carries_our_own_flag() {
        let reg = registry_with(&[
            ("default", 111, "/tmp/udd/default"),
            ("recycled", 222, "/tmp/udd/recycled"),
            ("gone", 333, "/tmp/udd/gone"),
            ("opaque", 444, "/tmp/udd/opaque"),
        ]);
        let killed = std::cell::RefCell::new(Vec::new());
        let n = reap_orphans(
            reg.path(),
            &|pid| match pid {
                // Ours, with a macOS env bleed sitting in the argv.
                111 => ArgvProbe::Argv(argv(&[
                    "/x/chrome",
                    "--user-data-dir=/tmp/udd/default",
                    "--headless=new",
                    "ZSH_AI_PROMPT_EXTEND=Always",
                    "prefer",
                    "modern",
                    "CLI",
                    "tools",
                    "like",
                    "ripgrep",
                ])),
                // A recycled pid: alive, and it is the NEIGHBOURING profile's
                // browser. A substring test would kill it.
                222 => ArgvProbe::Argv(argv(&[
                    "/x/chrome",
                    "--user-data-dir=/tmp/udd/recycled-2",
                ])),
                333 => ArgvProbe::Absent,
                444 => ArgvProbe::Unreadable,
                _ => ArgvProbe::Absent,
            },
            &|pid| killed.borrow_mut().push(pid),
        );
        assert_eq!(n, 1, "exactly the matching pid is reaped");
        assert_eq!(*killed.borrow(), vec![111]);

        // Killed -> record gone.
        assert!(!reg.path().join("default.json").exists());
        // Provably somebody else's -> record gone, process untouched.
        assert!(!reg.path().join("recycled.json").exists());
        // Absent from the process table -> nothing to kill, record stale, gone.
        assert!(!reg.path().join("gone.json").exists());
        // Argv unreadable -> we learned NOTHING. Keep the record.
        assert!(
            reg.path().join("opaque.json").exists(),
            "the record must survive an unreadable argv: it is the only way \
             this browser can ever be reaped"
        );
    }

    /// The case the first two drafts of this function got backwards, kept as
    /// its own test because it is the expensive one.
    ///
    /// `Unreadable` is routine on Windows, where `sysinfo` often cannot read
    /// another process's command line — i.e. the platform spec §3.6 already
    /// flags as unexercised is exactly the one where the wrong answer would be
    /// permanent. Deleting the record there is irreversible: the browser stays
    /// alive and the only thing that could ever find it again is gone
    /// (判据 §8 crossed with §15 — a one-shot latch missed once is missed
    /// forever). It is also why the probe has THREE states and not `Option`:
    /// an `Option` cannot tell "no such process" from "I could not look", and
    /// collapsing those two IS the defect.
    #[test]
    fn an_unreadable_argv_kills_nothing_and_keeps_everything() {
        let reg = registry_with(&[("default", 444, "/tmp/udd/default")]);
        let killed = std::cell::RefCell::new(Vec::new());
        let n = reap_orphans(reg.path(), &|_| ArgvProbe::Unreadable, &|pid| {
            killed.borrow_mut().push(pid)
        });
        assert_eq!(n, 0);
        assert!(killed.borrow().is_empty());
        assert!(reg.path().join("default.json").exists());
    }
}
```

- [ ] **跑它，看红。** `cargo test -p alephcore --lib browser::chromium_launch` → 期望 `error[E0433]: failed to resolve: use of undeclared type ChromiumLaunchSpec`（以及同类的 `parse_devtools_active_port` / `reap_orphans` / `sidecar_path` / `argv_names_dir` 未解析）。⚠️ 在把 `pub(crate) mod chromium_launch;` 加进 `src/browser/mod.rs` 之前，这个文件根本不参与编译，`cargo test` 会直接绿——所以**先**在 `src/browser/mod.rs` 第 3 行之后（按字母序）插入一行：
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

/// Our own record of a launched browser: what Aleph needs to recognise its own
/// orphan after a crash. Chrome does not remove `DevToolsActivePort` on exit,
/// so that file cannot answer "is this browser mine and still running".
///
/// These live in ONE registry directory, not beside each browser's profile.
/// A profile may configure `user_data_dir` to anywhere on disk (the repo's own
/// QA does), so a per-udd record puts itself outside anything a boot sweep can
/// walk — and the sweep would then miss exactly the case the fixture
/// exercises. One directory means "which browsers are there to reclaim" has a
/// single derivation (判据 §12).
pub(crate) const SIDECAR_EXT: &str = "json";

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

/// What Aleph records about a browser it launched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChromiumSidecar {
    pub pid: u32,
    pub http_url: String,
    /// The profile directory that process was launched with. Recorded rather
    /// than implied by the file's location, because the file is not stored
    /// there — and this is the value the orphan sweep matches against argv.
    pub user_data_dir: PathBuf,
    /// The build that launched it. Not used as a gate — recorded because an
    /// orphan from a different version is exactly the case a reader will want
    /// named when this goes wrong.
    pub aleph_version: String,
}

/// The one directory every sidecar lives in.
pub(crate) fn sidecar_registry_dir() -> Result<PathBuf, BrowserError> {
    super::playwright_launch::browser_state_dir("chromium")
}

/// This profile's record. Sanitized through the same helper the launch config
/// and the derived udd use, so a profile name can never escape the registry.
pub(crate) fn sidecar_path(session_key: &str) -> Result<PathBuf, BrowserError> {
    Ok(sidecar_registry_dir()?.join(format!(
        "{}.{SIDECAR_EXT}",
        super::playwright_launch::sanitize_session_key(session_key)
    )))
}

/// What a process's argv turned out to be — three states, not two.
///
/// `Option` cannot carry this, and collapsing it IS the defect this enum
/// exists to prevent: a reader that answers `None` for both "no such process"
/// and "I could not read its command line" makes the sweep spend an unknown as
/// a certainty, and the action on the other side is SIGKILL plus an
/// irreversible record deletion (判据 §8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArgvProbe {
    /// The pid is not in the process table. It is gone.
    Absent,
    /// The pid is there and its command line could not be read. Routine on
    /// Windows. We have learned nothing.
    Unreadable,
    /// The process's argv, one element per word as the kernel reports it.
    Argv(Vec<String>),
}

/// Whether `argv` names `dir` as the browser's profile directory.
///
/// **Token equality over the argv vector — never a substring scan over a
/// joined command line.** Both halves matter, because the action this
/// predicate authorises is a kill:
///
/// * **Prefix collision, no bleed required.** The sweep walks profiles that sit
///   under one root, so the flags it builds are prefixes of one another:
///   `--user-data-dir=<root>/default` is a substring of a live browser's
///   `--user-data-dir=<root>/default-2`. `sanitize_session_key` produces
///   prefix-related names routinely (`work` / `work-archive`). A substring test
///   therefore kills the neighbouring profile's browser — the exact case the
///   argv check was added to prevent, failing on its most likely neighbour.
/// * **The macOS argv/env bleed, already measured and pinned in this repo.**
///   `crates/agent-detect/src/engine.rs:427-431` records it verbatim: a process
///   that rewrites its title (every Node CLI does) leaves `sysinfo::cmd()`
///   reading past the argv region into the environment, and `:938-957` pins a
///   real reading in which an exported variable whose value contained spaces
///   scattered the bare words `prefer`, `modern`, `like` into the command line.
///   That module's defence is to tokenize and skip `VAR=value` words rather
///   than scan a joined string; 判据 §16 says the twin's answer gets carried
///   over rather than rediscovered.
///
/// Both spellings Chrome accepts are matched: `--user-data-dir=<path>` and the
/// two-token `--user-data-dir <path>`. Missing the second would let a browser
/// launched that way become unreapable.
#[must_use]
pub(crate) fn argv_names_dir(argv: &[String], dir: &Path) -> bool {
    let joined = format!("--user-data-dir={}", dir.display());
    let value = dir.to_string_lossy();
    argv.iter().enumerate().any(|(i, word)| {
        word == &joined
            || (word == "--user-data-dir"
                && argv.get(i + 1).is_some_and(|v| v.as_str() == value))
    })
}

/// One Chromium process owned by this Aleph.
pub(crate) struct ChromiumChild {
    child: Child,
    endpoint: CdpEndpoint,
    user_data_dir: PathBuf,
    session_key: String,
}

impl ChromiumChild {
    /// Launch Chromium and wait for it to publish its debug port.
    ///
    /// `session_key` is the profile name, and it is taken here rather than
    /// derived, because it is what names this browser's record in the sidecar
    /// registry — the only thing that can find the process again after a crash.
    pub(crate) async fn spawn(
        spec: &ChromiumLaunchSpec,
        session_key: &str,
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
                        session_key: session_key.to_string(),
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

    /// Kill the browser and clear its registry record.
    ///
    /// `wait()` runs **only after a successful `kill()`**. It is a blocking
    /// call and every production caller is inside async code: after a kill the
    /// reap is immediate, but `kill()` can fail (EPERM, or the child was
    /// already reaped) and then `wait()` would park a tokio worker until the
    /// process happened to exit on its own.
    pub(crate) fn shutdown(mut self) {
        let pid = self.endpoint.pid;
        match self.child.kill() {
            Ok(()) => {
                let _ = self.child.wait();
                tracing::info!(pid, "chromium shut down");
            }
            // Say which of the two happened rather than logging "shut down"
            // over a process that may still be running: an untrue log line is
            // the thing a reader would spend as evidence.
            Err(e) => tracing::warn!(pid, error = %e, "could not kill chromium; leaving it"),
        }
        match sidecar_path(&self.session_key) {
            Ok(path) => {
                let _ = std::fs::remove_file(path);
            }
            Err(e) => tracing::warn!(error = %e, "cannot resolve the chromium sidecar to remove"),
        }
    }

    async fn write_sidecar(&self) {
        let path = match sidecar_path(&self.session_key) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "cannot resolve the chromium sidecar path");
                return;
            }
        };
        if let Some(dir) = path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(dir).await {
                tracing::warn!(error = %e, "cannot create the chromium sidecar registry");
                return;
            }
        }
        let body = match serde_json::to_string(&ChromiumSidecar {
            pid: self.endpoint.pid,
            http_url: self.endpoint.http_url.clone(),
            user_data_dir: self.user_data_dir.clone(),
            aleph_version: env!("ALEPH_VERSION").to_string(),
        }) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "cannot serialize the chromium sidecar");
                return;
            }
        };
        if let Err(e) = tokio::fs::write(&path, body).await {
            // Best-effort here, but NOT unobserved: a missing sidecar costs an
            // orphan across a crash. The QA `attach` stage asserts the file
            // exists and that its pid matches a live process (Task 9 step 6),
            // because no unit test can see this.
            tracing::warn!(error = %e, path = %path.display(), "cannot write the chromium sidecar");
        }
    }
}

/// Kill Chromium processes left behind by a previous Aleph.
///
/// `registry` is [`sidecar_registry_dir`] — one directory holding one record
/// per profile, whatever each profile's `user_data_dir` happens to be. That is
/// why the sweep can be a single walk (判据 §12: the set has one derivation).
///
/// Four outcomes per record, and they are deliberately NOT collapsed:
///
/// * [`ArgvProbe::Argv`] naming our directory → it is ours: kill it, drop the
///   record;
/// * [`ArgvProbe::Argv`] naming something else → the pid was recycled and now
///   belongs to somebody else's program: kill nothing, drop the record (this
///   answer is determinate);
/// * [`ArgvProbe::Absent`] → the process is gone: nothing to kill, the record
///   is stale, drop it;
/// * [`ArgvProbe::Unreadable`] → we have learned **nothing**. Kill nothing, and
///   **keep the record**. Deleting it here is irreversible: the browser stays
///   alive and the only thing that could ever find it again is gone (判据 §8
///   crossed with §15). Routine on Windows, where `sysinfo` often cannot read
///   another process's command line — i.e. the platform spec §3.6 already flags
///   as unexercised is exactly the one where the wrong answer would be permanent.
///
/// Both effects are injected so the decision is testable without a browser;
/// [`reap_orphans_now`] is the production wiring.
pub(crate) fn reap_orphans(
    registry: &Path,
    argv_of: &dyn Fn(u32) -> ArgvProbe,
    kill: &dyn Fn(u32),
) -> usize {
    let Ok(entries) = std::fs::read_dir(registry) else {
        // The dir not existing is the normal first-boot state, not a failure.
        return 0;
    };
    let mut reaped = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != SIDECAR_EXT) {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(rec) = serde_json::from_str::<ChromiumSidecar>(&body) else {
            // A record we cannot parse names no pid, so it can never be acted
            // on; dropping it is the only way it stops being read every boot.
            tracing::warn!(path = %path.display(), "unparseable chromium sidecar; dropping it");
            let _ = std::fs::remove_file(&path);
            continue;
        };
        match argv_of(rec.pid) {
            ArgvProbe::Argv(argv) if argv_names_dir(&argv, &rec.user_data_dir) => {
                tracing::info!(
                    pid = rec.pid,
                    dir = %rec.user_data_dir.display(),
                    "reaping orphaned chromium"
                );
                kill(rec.pid);
                reaped += 1;
                let _ = std::fs::remove_file(&path);
            }
            // A pid that resolved to somebody ELSE's argv is provably not ours:
            // determinate, so the record goes and the process is left alone.
            ArgvProbe::Argv(_) => {
                let _ = std::fs::remove_file(&path);
            }
            ArgvProbe::Absent => {
                let _ = std::fs::remove_file(&path);
            }
            // Present, argv unreadable: keep it. See the doc above.
            ArgvProbe::Unreadable => tracing::warn!(
                pid = rec.pid,
                "chromium sidecar kept: the process exists but its argv is unreadable"
            ),
        }
    }
    reaped
}

/// The real process-table reader.
///
/// Goes to `sysinfo::Process::cmd()`, which is a `Vec<OsString>` — **the argv
/// vector, not a joined line**. That is the whole point:
/// `gateway::pty::foreground::fact_for_pid` is the obvious-looking source and
/// it is the wrong one, because `ForegroundFact::cmdline` is documented as
/// "The whole command line, space-joined" (`src/gateway/pty/foreground.rs:142-143`,
/// built by `cmd.iter().map(to_string_lossy).collect::<Vec<_>>().join(" ")` at
/// `:266-273`). Matching on that string can only ever be `str::contains`, and
/// token equality is **not expressible** through it. `ForegroundFact` is also a
/// pty-side type with a different contract; borrowing it here would couple two
/// subsystems through a projection neither of them wants.
///
/// The `None` / `Some(empty)` split is what gives the three states: the helper
/// answers `None` only when the pid is not in the process table, and an empty
/// `cmd()` for a pid that IS there means the command line could not be read.
///
/// Built on `utils::process_alive::with_process_specifics` rather than a fresh
/// `System::new_with_specifics(...)`: that helper is this repo's single owner of
/// the single-pid `sysinfo` idiom and its own doc says so — "A second copy of
/// the `System::new()` + `refresh_processes_specifics` dance would be the same
/// fact written twice (判据 §1), and the two would drift on exactly the axis
/// that matters, which fields get refreshed." It also scopes the refresh to one
/// pid instead of walking every process on the machine.
fn argv_probe(pid: u32) -> ArgvProbe {
    let cmd = crate::utils::process_alive::with_process_specifics(
        pid,
        sysinfo::ProcessRefreshKind::nothing().with_cmd(sysinfo::UpdateKind::Always),
        |p| {
            p.cmd()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<String>>()
        },
    );
    match cmd {
        None => ArgvProbe::Absent,
        Some(v) if v.is_empty() => ArgvProbe::Unreadable,
        Some(v) => ArgvProbe::Argv(v),
    }
}

/// [`reap_orphans`] wired to the real process table.
pub(crate) fn reap_orphans_now() -> usize {
    let registry = match sidecar_registry_dir() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "cannot sweep orphaned chromium processes");
            return 0;
        }
    };
    reap_orphans(&registry, &argv_probe, &|pid| {
        let killed = crate::utils::process_alive::with_process_specifics(
            pid,
            sysinfo::ProcessRefreshKind::nothing(),
            sysinfo::Process::kill,
        );
        if killed != Some(true) {
            tracing::warn!(pid, "orphaned chromium did not accept the kill");
        }
    })
}
```

- [ ] **跑到绿。** `cargo test -p alephcore --lib browser::chromium_launch` → 全过。若 `sysinfo::Process::kill` / `ProcessRefreshKind::with_cmd` 的签名在 0.39 上与这里写的不同，按编译器提示改（`|p| p.kill()`、`UpdateKind` 的路径），**不要改判据**。
- [ ] **证伪四次守卫。**
  1. 把 `parse_devtools_active_port` 里 `if !path.starts_with('/') { return None; }` 注释掉 → `every_partial_or_malformed_port_file_reads_as_not_yet` 必须变红（`58363\ndevtools/browser/x` 那一条）。
  2. **把 `argv_names_dir` 改回子串扫描**：`argv.join(" ").contains(&joined)` → `the_udd_match_is_token_equality_over_argv` 与 `reap_orphans_kills_only_the_process_that_carries_our_own_flag` 必须**同时**变红（前者的 sibling-prefix 与 env-value 两条，后者的 `recycled` 那一条会被误杀）。**这就是本轮 addendum 修的那个缺陷**——变异回去必须立刻可见。
  3. 把 `argv_names_dir` 的两 token 形式那一支删掉 → `the_udd_match_is_token_equality_over_argv` 必须变红（`["--user-data-dir", "/tmp/udd/default"]` 那一条）。
  4. 把 `reap_orphans` 的 `ArgvProbe::Unreadable` 臂改成也 `remove_file` → `an_unreadable_argv_kills_nothing_and_keeps_everything` 与 `reap_orphans_kills_only_the_process_that_carries_our_own_flag` 的 `opaque.json` 断言必须变红。**这一条是本任务最贵的守卫**：它守的是「不知道」不许当成「已经没了」花掉。
  四次都确认之后**恢复原样**。
- [ ] `rustfmt src/browser/chromium_launch.rs src/browser/error.rs src/browser/chrome_mcp.rs src/diagnostics/checks/browser_runtime.rs`（四个都是叶子文件，不声明子模块 —— ⚠️ `src/browser/mod.rs` 这一笔只加了一行 `pub(crate) mod chromium_launch;`，**不要**把它交给 `rustfmt`：它声明 18 个子模块，`rustfmt` 会递归进去重排整个 `src/browser/`。那一行手写即可，没有可格式化的东西。）
- [ ] 两条命令，**不要合并**（`cargo test` 只收一个 TESTNAME，第二个位置参数会被拒：`error: unexpected argument 'bbb' found`）：
  ```
  cargo test -p alephcore --lib browser::
  cargo test -p alephcore --lib diagnostics::checks::browser_runtime
  ```
- [ ] **提交。**
  ```
  git add src/browser/chromium_launch.rs src/browser/mod.rs src/browser/error.rs \
          src/browser/chrome_mcp.rs src/diagnostics/checks/browser_runtime.rs
  git commit -m "browser: launch chromium and read its DevToolsActivePort

  Aleph spawns Chromium with --remote-debugging-port=0, polls the port file
  with a deadline, and records the launch in one sidecar registry under
  ~/.aleph/data/browser/chromium so a crashed daemon's orphan can be found
  whatever user_data_dir the profile configured. The sweep reads the argv
  VECTOR from sysinfo and matches --user-data-dir as a whole token: a
  space-joined command line only supports substring matching, which would kill
  a sibling profile's browser (default is a substring of default-2) and is read
  on macOS past the argv region into the environment, as agent-detect already
  measured and pinned. The probe has three states, so a process whose argv is
  merely unreadable keeps its record instead of being spent as gone. LaunchFailed
  gains a stage so \"would not spawn\", \"died early\" and \"no port file\" stop
  reading alike.

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
        // The `[runtime]` table is PRESENT here and the key is absent, so this
        // exercises serde's field-level `default = "default_true"` — which is a
        // different mechanism from `Default::default()` and the one that would
        // silently flip to `false` if the attribute were dropped.
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
- Modify `src/browser/mod.rs`（模块声明，紧跟 Task 1 加的那一行；只加一行，**不交给 `rustfmt`**）
- Modify `src/browser/discovery.rs`：`find_chromium_preferred`（:125）现在被本模块调用（`mod.rs:4` 的 `mod discovery;` 是私有的，同 crate 内 `super::discovery::` 可达，**无需**改可见性——先确认，若编译器要求再改成 `pub(super)`）；新增 `engine_of`，就写在 `engine_hints`（:85-98）下方，与它共用那张表

**Interfaces:**
- Consumes: `super::profile::{BrowserRuntimeConfig, BrowserType}`（Task 2）· `super::discovery::find_chromium_preferred`（`discovery.rs:125`）· `super::error::BrowserError::{ChromiumUnavailable, PlaywrightCliError}`（Task 1）· `crate::utils::no_window::NoWindow`
- Produces:
  ```rust
  pub(crate) enum ChromiumSource { Pinned, System, PlaywrightManaged }
  pub(crate) struct ResolvedChromium { pub path: PathBuf,
                                       pub source: ChromiumSource,
                                       pub engine: Option<BrowserType> }
  pub(crate) fn parse_install_location(dry_run_stdout: &str) -> Option<PathBuf>;
  pub(crate) fn executable_among(files: &[PathBuf]) -> Option<PathBuf>;
  pub(crate) async fn resolve_binary(runtime: &BrowserRuntimeConfig,
                                     browser: &BrowserType,
                                     cli_binary: &Path)
      -> Result<ResolvedChromium, BrowserError>;
  // discovery.rs
  pub(super) fn engine_of(path: &Path) -> Option<BrowserType>;
  ```
  ⚠️ `resolve_binary` 返回的是 `ResolvedChromium`，**不是** `(PathBuf, ChromiumSource)`：`engine` 那一栏是替代被删掉的启动告警的东西（前言 §7），调用方比对「请求的引擎」与「解析出的引擎」并在不一致时 `warn!`。三个消费者（Task 5 / 7 / 8）都读这个结构。

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

**关于 spec §6.1「首次用到 Managed profile 时再试一次[安装]」——本计划刻意不做，理由写在这里而不是省略：** 那一次尝试要下载约 150 MB，而它会落在**第一次浏览器工具调用**的路径上，那条路径的硬预算是 180 s 的工具预算（`WAIT_MAX_TIMEOUT_SECS=170`，CLAUDE.md 明写不许扩展）。所以本计划的 `resolve_binary` 从「三条路线都没给出文件」直接走到 `ChromiumUnavailable`，而安装留给三个显式入口：台账在装 `playwright-cli` 时的 post-install（Task 7）、doctor 的 fix hint、以及模型自己调 `runtime_manage{install}`（Task 8）。**代价说清楚**：一台干净的 Linux 服务器上，第一次浏览器调用会失败一次，模型读到那句话之后再装。这比一个会卡满工具预算然后超时的「自动」路径诚实。

#### Steps

- [ ] **先加 `engine_of`（一份推导的来源）。** 在 `src/browser/discovery.rs` 的 `engine_hints`（:85-98）之后插入：

```rust
/// Which engine a resolved binary IS, read off its path with the same table
/// [`engine_hints`] orders candidates by.
///
/// This exists so the substitution a launch performs can be reported. The old
/// boot warning (`manager::unhonored_managed_fields`) covered exactly one case
/// — a managed Brave profile, which `playwright-cli open --browser` had no
/// value for. Aleph launches the browser itself now, so that case is honoured
/// when Brave is installed; but [`find_chromium_preferred`] **degrades
/// silently** when it is not ([`prefer_paths`] merely reorders and the fallback
/// is only `debug!`), so "asked for Brave, got Chrome" would go unreported at
/// every level. Deriving both the ordering and the identification from one
/// table is what keeps the warning about the same notion of "which browser
/// is this" that the search used (判据 §1).
///
/// `None` means *unidentifiable*, and callers must not spend it as "the
/// requested engine was honoured".
pub(super) fn engine_of(path: &Path) -> Option<BrowserType> {
    let s = path.to_string_lossy();
    // Most specific first: "Google Chrome" contains "Chrome", and a Brave or
    // Edge path must not be answered as Chrome.
    for browser in [
        BrowserType::Brave,
        BrowserType::Edge,
        BrowserType::Chrome,
        BrowserType::Chromium,
    ] {
        let (path_substrings, names) = engine_hints(&browser);
        if path_substrings.iter().any(|sub| s.contains(sub))
            || names.iter().any(|n| {
                path.file_name()
                    .is_some_and(|f| f.to_string_lossy().starts_with(n))
            })
        {
            return Some(browser);
        }
    }
    None
}
```

  并在 `discovery.rs` 的 `use std::path::PathBuf;`（:13）改为 `use std::path::{Path, PathBuf};`。

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
    ///
    /// ⚠️ Every path here uses `/`. A backslash Windows path (`C:\…\chrome.exe`)
    /// written as a literal would make this test RED on macOS and Linux, where
    /// `\` is not a separator and `Path::file_name` therefore returns the whole
    /// string — and the RED→GREEN loop would push the executor to "fix" a
    /// correct implementation. `file_name()` handles forward slashes on every
    /// target, and Windows accepts them too.
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
            PathBuf::from("C:/c/chromium-1219/chrome-win/chrome.dll"),
            PathBuf::from("C:/c/chromium-1219/chrome-win/chrome.exe"),
        ];
        assert_eq!(
            executable_among(&windows),
            Some(PathBuf::from("C:/c/chromium-1219/chrome-win/chrome.exe"))
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

    /// `chrome-headless-shell` is a real Chromium binary and spec §6.1 names it
    /// as the no-root Linux degrade — and it is deliberately NOT a candidate
    /// here. Taking it silently would give a `headless = false` profile a
    /// browser that can never show a window. A route that reports success and
    /// delivers less is worse than one that refuses, so the shell must not be
    /// picked even when it is the ONLY thing in the directory.
    #[test]
    fn the_headless_shell_is_never_picked_even_when_it_is_the_only_binary() {
        let both = vec![
            PathBuf::from("/c/x/chrome-headless-shell-linux/chrome-headless-shell"),
            PathBuf::from("/c/x/chrome-linux/chrome"),
        ];
        assert_eq!(
            executable_among(&both),
            Some(PathBuf::from("/c/x/chrome-linux/chrome"))
        );
        let shell_only =
            vec![PathBuf::from("/c/x/chrome-headless-shell-linux/chrome-headless-shell")];
        assert_eq!(executable_among(&shell_only), None);
    }

    /// The engine identifier the substitution warning is derived from. It must
    /// answer from the SAME table `find_chromium_preferred` orders by, or the
    /// warning would be about a different notion of "which browser is this".
    #[test]
    fn the_resolved_engine_is_read_off_the_path_by_the_discovery_table() {
        use crate::browser::profile::BrowserType;
        assert_eq!(
            crate::browser::discovery::engine_of(std::path::Path::new(
                "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"
            )),
            Some(BrowserType::Brave)
        );
        assert_eq!(
            crate::browser::discovery::engine_of(std::path::Path::new("/usr/bin/google-chrome")),
            Some(BrowserType::Chrome)
        );
        // Playwright's own build is Chromium, whatever its file is called.
        assert_eq!(
            crate::browser::discovery::engine_of(std::path::Path::new(
                "/c/chromium-1219/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
            )),
            Some(BrowserType::Chrome),
            "'Google Chrome for Testing' matches the Chrome hints; the caller \
             compares against what it ASKED for, and the Playwright route \
             already logs its own substitution"
        );
        assert_eq!(
            crate::browser::discovery::engine_of(std::path::Path::new("/opt/weird/browser")),
            None,
            "unidentifiable must be None, not a guess — an unknown engine is \
             not evidence that the requested one was honoured"
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

/// How long the `--dry-run` probe may take.
///
/// It performs **no download** — it prints a table and exits (measured: it
/// answered instantly on the machine this plan was written on). Six seconds is
/// therefore generous, and the ceiling matters upward as well as downward: the
/// doctor check that calls this (`diagnostics::checks::chromium_missing`)
/// bounds the whole resolution at 8 s so that ITS own "could not verify" answer
/// fires, and that 8 s sits under the engine's `DEFAULT_CHECK_TIMEOUT` of 20 s
/// (`src/diagnostics/check.rs:27`), past which the engine abandons the check
/// and emits a `Warning` of its own. Three budgets, strictly nested: 6 < 8 < 20,
/// and the nesting is asserted by
/// `diagnostics::checks::chromium_missing::the_check_answers_before_the_engine_abandons_it`
/// rather than restated as prose — which is why this is `pub(crate)`.
pub(crate) const DRY_RUN_TIMEOUT: Duration = Duration::from_secs(6);

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
/// **`chrome-headless-shell` is deliberately NOT here.** It is a real Chromium
/// binary and spec §6.1 names it as the no-root Linux degrade, but taking it
/// would be a silent capability cut: it cannot run headed, so a
/// `headless = false` profile that resolved to it would launch a browser that
/// can never show a window, and nothing in this plan forces `headless` off the
/// resolution. Wiring that degrade properly (a source variant the launch reads,
/// plus an install path for the shell) is a separate piece of work; listing the
/// binary without it would be a route that reports success and delivers less.
const EXECUTABLE_LEAVES: &[&str] = &[
    "Google Chrome for Testing",
    "Chromium",
    "chrome",
    "chrome.exe",
];

/// How deep the install directory is walked looking for the executable.
///
/// macOS is the deepest known layout and needs five levels below the install
/// dir (`chrome-mac-arm64` / `X.app` / `Contents` / `MacOS` / `X`); Linux and
/// Windows need two. ⚠️ The walk enumerates the whole `.app` bundle, which is
/// thousands of entries — acceptable because it runs only on the
/// Playwright-managed route, i.e. only when no pin and no system browser
/// answered, and the result is cached by the caller.
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

/// What a resolution answered: the file, which route found it, and **which
/// engine it turned out to be**.
///
/// The third field is the replacement for the boot warning this round deletes
/// (`manager::unhonored_managed_fields`). `find_chromium_preferred` degrades
/// silently when the requested engine is absent, so without this the
/// substitution "asked for Brave, got Chrome" would be reported nowhere.
/// `None` means the path matched no engine hint — unidentifiable, which is
/// **not** evidence that the request was honoured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedChromium {
    pub path: PathBuf,
    pub source: ChromiumSource,
    pub engine: Option<BrowserType>,
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
///
/// **This function never installs anything.** Spec §6.1's "try once more at
/// first use" is deliberately not done here: the download is ~150 MB and this
/// runs on the first browser tool call, whose hard budget is 180 s. Installing
/// has three explicit entrances instead — the ledger's post-install, the
/// doctor's fix hint, and `runtime_manage{install}`.
pub(crate) async fn resolve_binary(
    runtime: &BrowserRuntimeConfig,
    browser: &BrowserType,
    cli_binary: &Path,
) -> Result<ResolvedChromium, BrowserError> {
    let mut tried: Vec<String> = Vec::new();

    if let Some(pin) = runtime.pinned_binary() {
        let path = PathBuf::from(pin);
        if path.is_file() {
            let engine = super::discovery::engine_of(&path);
            return Ok(ResolvedChromium { path, source: ChromiumSource::Pinned, engine });
        }
        return Err(BrowserError::ChromiumUnavailable {
            tried: format!("[browser.runtime] binary_path = {pin:?} does not exist"),
        });
    }

    if runtime.prefer_system_browser {
        match super::discovery::find_chromium_preferred(browser) {
            Ok(path) => {
                let engine = super::discovery::engine_of(&path);
                return Ok(ResolvedChromium { path, source: ChromiumSource::System, engine });
            }
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
                // asking for Brave gets Chromium here. The caller's own
                // requested-vs-resolved check covers the OTHER silent
                // substitution (`find_chromium_preferred` degrading to whatever
                // is installed); this arm covers the one that route cannot see.
                tracing::warn!(
                    requested = ?browser,
                    "no system browser for the requested engine; falling back to \
                     Playwright's managed Chromium"
                );
            }
            let engine = super::discovery::engine_of(&path);
            Ok(ResolvedChromium { path, source: ChromiumSource::PlaywrightManaged, engine })
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

- [ ] **跑到绿。** `cargo test -p alephcore --lib browser::chromium_resolve` 与 `cargo test -p alephcore --lib browser::discovery`（两条命令，别合并）→ 全过。
- [ ] **手工核对解析器打在真输出上，并且带一条断言。** 本机跑
  ```
  playwright-cli install-browser chromium --dry-run > /tmp/dry.txt
  grep -n "playwright chromium v" -A 1 /tmp/dry.txt
  ```
  期望第一块的 `Install location:` 紧跟在 `(playwright chromium v…)` 那一行之后（上面的 `DRY_RUN` 常量就是本机这一次的逐字副本）。**如果措辞变了**：把 `/tmp/dry.txt` 的内容整段替换进 `DRY_RUN` 常量、按新措辞改 `CHROMIUM_BLOCK`，**再把两条解析测试重跑一遍**（不是只改常量就算完——测试是对新转录的断言，判据 §5），并在 Task 10 的 FEATURE_LOCATOR 条目里记下新旧措辞。
- [ ] **证伪两次。** ① 把 `CHROMIUM_BLOCK` 改成 `"(playwright chromium"`（去掉尾部的 ` v`）→ `the_headless_shell_block_is_not_mistaken_for_the_browser` 必须变红。② 把 `EXECUTABLE_LEAVES` 加回 `"chrome-headless-shell"` → `the_headless_shell_is_never_picked_even_when_it_is_the_only_binary` 必须变红。两次都恢复。
- [ ] `rustfmt src/browser/chromium_resolve.rs src/browser/discovery.rs`（两个都是叶子文件）
- [ ] `cargo test -p alephcore --lib browser::` 全绿。
- [ ] **提交。**
  ```
  git add src/browser/chromium_resolve.rs src/browser/discovery.rs src/browser/mod.rs
  git commit -m "browser: resolve chromium as pin > system > playwright-managed

  The playwright-managed route asks the CLI that installs it
  (install-browser chromium --dry-run prints Install location:) instead of
  hard-coding three platform cache paths and a revision guess. On macOS the
  cache is ~/Library/Caches/ms-playwright and the binary is 'Google Chrome for
  Testing', neither of which the guessed layout would have found. The
  resolution also reports which engine it actually found, so the substitution
  the deleted boot warning used to cover is still reported — this time on the
  path that performs it rather than at boot.

  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01TKV5PtutzoBvbT4yTpsyRY"
  ```

---

### Task 4: `playwright_launch` —— 新增 `attach_argv`，配置只剩两键

> **偏离（review 采纳）**：初稿把 `open_argv` / `browser_flag_value` / `unhonored_managed_fields` 的删除也放在本任务，于是本任务结束时 crate **编译不过**（`playwright_cli.rs:24` 还 `use open_argv`），「跑到绿」那一步根本跑不起来，红也就不可归因——而 Task 4/5 拆开的唯一理由就是可归因。现在本任务只做**加法与改签名**，并顺手改掉 `playwright_cli.rs` 里那一处机械的调用点，因此**本任务自己是绿的、自己提交**；三处删除随翻转一起进 Task 5。

**Files:**
- Modify `src/browser/playwright_launch.rs`：模块 doc（:1-21）· `launch_config_json`（:123-200）改签名 · 新增 `attach_argv`（写在 `open_argv` 之后、`config_path_for` 之前）· `write_launch_config`（:294-329）改签名 · 测试：改写 `:335-388` 的三条键集测试为一条、改写 `:407-425` 的 `every_launch_carries_an_explicit_config`、**改写 `:458-475` 的 `browsed_page_content_is_contained_under_aleph_home`**（它是 `launch_config_json` 的**第二个**调用点，初稿漏了它，Task 4 会编译不过）
- Modify `src/browser/playwright_cli.rs:235`（`write_launch_config(session_key, launch)` 的实参少一个——一行机械修改，放在这里是为了让本任务有一个绿）

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

- [ ] **写失败测试。** 在 `src/browser/playwright_launch.rs` 的 `mod tests` 里：把 `a_configuring_launch_maps_onto_the_documented_schema`（:336-366）、`a_default_launch_still_produces_a_config_to_displace_the_ambient_one`（:367-379）、`launch_options_is_omitted_rather_than_emitted_empty`（:380-388）三条**合并成一条**新的键集测试；把 `every_launch_carries_an_explicit_config`（:409-425）改写成 attach 版；并把 `browsed_page_content_is_contained_under_aleph_home`（:458-475）里那一行 `launch_config_json(&SessionLaunch::headless_default(), &out)` 改成一个实参（其余断言原样保留）。新增/改写后的测试：

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

    /// The OTHER caller of `launch_config_json`, which the arity change would
    /// otherwise leave uncompilable. Its containment assertions are about
    /// `output_dir_for` and are unaffected — only the call loses an argument.
    #[test]
    fn browsed_page_content_is_contained_under_aleph_home() {
        let out = output_dir_for("default").expect("home resolves");
        let json = launch_config_json(&out);
        assert_eq!(json["outputDir"], json!(out.to_string_lossy()));
        assert!(
            out.to_string_lossy().contains("browser"),
            "expected a path under the browser state dir, got {}",
            out.display()
        );
        // Same containment property as the config file: one component, under
        // the state dir, whatever the session key looks like.
        let dir = out.parent().expect("has a parent").to_path_buf();
        for hostile in ["../../etc", "/etc", "..", "", "a/b"] {
            let p = output_dir_for(hostile).expect("home resolves");
            assert_eq!(p.parent(), Some(dir.as_path()), "escaped with {hostile:?}");
        }
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

- [ ] **跑它，看红。** `cargo test -p alephcore --lib browser::playwright_launch` → 期望 `error[E0425]: cannot find function attach_argv` 与 `error[E0061]: this function takes 1 argument but 2 arguments were supplied`（旧的 `launch_config_json` 调用点**两处**：`write_launch_config` 里一处，`browsed_page_content_is_contained_under_aleph_home` 里一处）。

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

  2. `launch_config_json`（:123-200）：doc 中删掉 `proxy`/`extra_args`/`userDataDir` 那两段（它们描述的键不再存在），保留 `outputDir` 与 `allowUnrestrictedFileAccess` 两段；函数体改为：

```rust
#[must_use]
pub fn launch_config_json(output_dir: &Path) -> Value {
    json!({
        "outputDir": output_dir.to_string_lossy(),
        "allowUnrestrictedFileAccess": true,
    })
}
```

  3. 在 `open_argv`（:207-234，本任务**不动它**，Task 5 才删）之后、`config_path_for` 的 doc（:236 起）之前写入：

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

  4. `write_launch_config`（:294-329）：签名去掉 `launch` 参数，doc 的最后一段（「Rewritten on every launch rather than written once: the profile's proxy / user-data-dir / extra args can change…」）替换成：

```rust
/// Rewritten on every attach rather than written once: `outputDir` is derived
/// from the session key and the home dir, both of which a restart can move, and
/// the file is only read at attach time — so the cheapest correct thing is to
/// make it a pure function of the current session.
```

  函数体里删掉 `let body = launch_config_json(launch, &output_dir).to_string();` 的 `launch` 实参，改为 `launch_config_json(&output_dir)`。

- [ ] **最小实现（唯一的外部调用点）。** `src/browser/playwright_cli.rs:235` 改为
  ```rust
  let config_path = write_launch_config(session_key).await?;
  ```
  `open_session` 的其余部分、`open_argv` 的 import（:24）与调用（:236）**都不动**——Task 5 才翻转它们。这一行让本任务留下一个能编译、能跑测试的树。

- [ ] **跑到绿（两条命令，别合并——`cargo test` 只收一个 TESTNAME，第二个位置参数会被拒）。**
  ```
  cargo test -p alephcore --lib browser::playwright_launch
  cargo test -p alephcore --lib browser::playwright_cli
  ```
- [ ] **证伪两次。** 本任务新增三条守卫，而一条没被证伪过的守卫不算守卫（判据 §3）：
  1. 把 `attach_argv` 的第一个元素改成 `"open".to_string()` → `the_launch_verb_is_attach_and_never_open` 与 `attach_argv_names_the_endpoint_and_always_carries_an_explicit_config` 必须**同时**变红。
  2. 在 `launch_config_json` 的 `json!` 里加回 `"browser": {"userDataDir": "/x"}` → `the_attach_config_carries_exactly_the_two_keys_the_cli_still_owns` 必须变红（那条测试对**不该存在的键**逐个断言，所以这次变异打得中）。
  两次都恢复。
- [ ] `rustfmt src/browser/playwright_launch.rs src/browser/playwright_cli.rs`（两个都是叶子文件，不声明子模块）
- [ ] **提交。**
  ```
  git add src/browser/playwright_launch.rs src/browser/playwright_cli.rs
  git commit -m "browser: add attach_argv and slim the CLI launch config to two keys

  attach --cdp takes --config (verified against playwright-cli 0.1.8's own
  --help), so the outputDir containment survives the flip. Everything else the
  config carried - userDataDir, proxy, args - describes launching a browser,
  which the CLI is about to stop doing. open_argv stays for one more commit so
  this one leaves a tree that compiles.

  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01TKV5PtutzoBvbT4yTpsyRY"
  ```

---

### Task 5: 惰性 `open` → 惰性 `attach`（`PlaywrightCliDriver` 的翻转）

**Files:**
- Modify `src/browser/playwright_cli.rs`：imports（:6-25）· `PlaywrightCliDriver` 结构（:51-56）与 `new`（:59-67）· `run`（:177-221）· `open_session`（:223-249）→ `attach_session` · 新增 `ensure_chromium` / `forget_chromium` / `endpoint` / `chromium_alive` / `shutdown_chromium` · `classify_failure`（:331-389）加锚点 · 测试模块（文件尾）
- Modify `src/browser/playwright_launch.rs`：**删** `browser_flag_value` 及其 doc（:106-123 整块 —— ⚠️ 不是 `:110-121`，那个范围会把 doc 的头四行和函数尾巴留在原地）· **删** `open_argv` 及其 doc（:207-234 整块）· 删它们的三条测试 `headed_puts_the_flag_on_open_where_the_cli_accepts_it`（:389-400）、`headless_omits_the_headed_flag`（:401-408）、`browser_flag_only_carries_values_the_cli_accepts`（:431-457）
- Modify `src/browser/manager.rs`：删 `unhonored_managed_fields`（:575-591 的 doc + fn）与 `ProfileManager::new` 里调用它的循环（:153-163），以及它的测试 `managed_profiles_name_the_fields_their_driver_drops`（**`:830-875` 整个函数**，`#[test]` 在 :830、`fn` 在 :831 —— ⚠️ 不是 `:838-875`，那是函数体中段）
- Modify `src/browser/profile.rs:36-42`（`ProfileConfig::browser` 的 doc 引用一个不再存在的机制）
- Modify `src/browser/playwright_cli_backend.rs:170-173`（注释引用 `playwright_launch::open_argv`）

**Interfaces:**
- Consumes: `super::chromium_launch::{ChromiumChild, ChromiumLaunchSpec, CdpEndpoint, DEVTOOLS_PORT_DEADLINE}`（Task 1）· `super::chromium_resolve::{resolve_binary, ResolvedChromium}`（Task 3）· `super::playwright_launch::{attach_argv, write_launch_config, browser_state_dir, sanitize_session_key}`（Task 4；后两个是 `pub(super)`，同 `browser` 模块内可达）· `super::profile::BrowserRuntimeConfig`（Task 2）
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
  // DELETED this task: playwright_launch::{open_argv, browser_flag_value},
  //                    manager::unhonored_managed_fields
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

    /// The anchor is the **pair**, not either phrase alone, and that is not
    /// fussiness: `classify_failure` runs on output that can contain page text
    /// (its own doc says so, and `snapshot`/`console` echo the page under
    /// `### Result`). A developer's own error page carrying the word
    /// `ECONNREFUSED` must not be able to talk the driver into relaunching a
    /// browser. Requiring both the node error AND playwright's call-log line is
    /// what a page cannot supply by accident.
    #[test]
    fn one_half_of_the_attach_signature_is_not_enough() {
        for half in [
            "Error: connect ECONNREFUSED 127.0.0.1:8080",
            "  - <ws preparing> retrieving websocket url from http://127.0.0.1:1",
        ] {
            let err = classify_failure(half, "", 1, "default", 10_000);
            assert!(
                !matches!(err, BrowserError::AttachFailed(_)),
                "half the signature was enough: {half:?} -> {err:?}"
            );
        }
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

    /// An ordinary Playwright failure keeps its own class.
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

    /// spec §6.2 row "Chrome 中途死 → 下次工具调用惰性重启" — and the two ways it
    /// can present, which the first draft covered only one of.
    ///
    /// `NoSession` is the CLI saying it has no session: attach, whatever the
    /// browser is doing. `AttachFailed` is the endpoint refusing a connection,
    /// and it must trigger a relaunch **only when the browser is not alive**.
    /// Relaunching over a live browser is the D.9.10 hazard in a new costume:
    /// the old one was a second `open` dropping every tab, the new one is a
    /// second Chromium writing the same `DevToolsActivePort`. Everything else
    /// is the model's error to read, not the driver's to route (R10 第 5 不).
    #[test]
    fn only_a_dead_browser_earns_a_relaunch() {
        // The CLI has no session: attach regardless of the browser's state.
        assert!(needs_relaunch(&BrowserError::NoSession("d".into()), true));
        assert!(needs_relaunch(&BrowserError::NoSession("d".into()), false));
        // A refused endpoint with the browser gone: relaunch.
        assert!(needs_relaunch(&BrowserError::AttachFailed("econnrefused".into()), false));
        // A refused endpoint while the browser is ALIVE: do not. Something else
        // is wrong and a second Chromium would make it worse.
        assert!(!needs_relaunch(&BrowserError::AttachFailed("econnrefused".into()), true));
        // Ordinary failures are the model's to read.
        for other in [
            BrowserError::ActionFailed("element not found".into()),
            BrowserError::Timeout(1000),
            BrowserError::PlaywrightCliError("exit 1: boom".into()),
        ] {
            assert!(!needs_relaunch(&other, false), "{other:?} must not relaunch");
            assert!(!needs_relaunch(&other, true), "{other:?} must not relaunch");
        }
    }

    /// The cheap pre-verb check. `chromium_alive` is a `try_wait` on a child we
    /// own — no syscall storm, no process-table scan — so asking before every
    /// verb costs nothing and closes the gap where the CLI's error text does
    /// not happen to be one of the phrasings above.
    ///
    /// With no child recorded it must answer `false`: "there is no browser" is
    /// not "the browser is dead", and the lazy attach already handles the
    /// first case.
    #[tokio::test]
    async fn a_profile_with_no_child_is_not_reported_as_a_dead_one() {
        let driver = PlaywrightCliDriver::new(
            PlaywrightCliConfig::default(),
            crate::browser::profile::BrowserRuntimeConfig::default(),
        );
        assert!(!driver.chromium_alive("default"));
        assert!(!driver.chromium_died("default"));
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

  3. `run`（:197-221）：加一道**动词前的活性检查**，并把重启臂从「只认 NoSession」放宽到「NoSession，或浏览器已死时的 AttachFailed」：
```rust
        let bin = self.resolve_binary().await?;
        let lock = self.session_lock(session_key);
        let _guard = lock.lock().await;

        // Before the verb: if this profile HAD a browser and it has since
        // exited, no CLI subcommand can succeed and the error it returns is
        // not guaranteed to be one of the phrasings below. `chromium_died` is
        // a `try_wait` on a child we own — cheap enough to ask every time, and
        // it is the only thing that closes spec §6.2's "Chrome 中途死" row for
        // the verbs whose failure text says something else entirely.
        if self.chromium_died(session_key) {
            if let Some(launch) = policy.launch() {
                tracing::info!(session = %session_key, "chromium exited; relaunching before the verb");
                self.forget_chromium(session_key);
                self.attach_session(&bin, session_key, launch).await?;
            }
        }

        let first = self.spawn(&bin, session_key, args, timeout).await;
        let Err(err) = first else {
            return first;
        };
        if !needs_relaunch(&err, self.chromium_alive(session_key)) {
            return Err(err);
        }
        let Some(launch) = policy.launch() else {
            return Err(err);
        };
        self.forget_chromium(session_key);
        self.attach_session(&bin, session_key, launch).await?;
        // One retry only. If the verb still fails after a successful attach,
        // that is a real failure and must surface rather than loop.
        self.spawn(&bin, session_key, args, timeout).await
```
  以及一个纯判定函数，写在 `classify_failure` 旁边（它是这条规则的**唯一**推导点，两个调用点——`run` 与将来的任何面——都读它）：
```rust
/// Whether this failure means "the browser needs relaunching", given whether
/// the browser is currently alive.
///
/// * [`BrowserError::NoSession`] — the CLI has no session for this key. Attach,
///   whatever the browser is doing; that is the lazy launch this driver has
///   always had.
/// * [`BrowserError::AttachFailed`] — the endpoint refused a connection. Only a
///   reason to relaunch when the browser is **not** alive. Relaunching over a
///   live browser is appendix D.9.10's hazard in a new costume: there it was a
///   second `open` dropping every tab, here it is a second Chromium writing the
///   same `DevToolsActivePort`.
/// * everything else — the model's error to read. The harness does not pick a
///   recovery strategy on its behalf (R10 第 5 不).
#[must_use]
fn needs_relaunch(err: &BrowserError, chromium_alive: bool) -> bool {
    match err {
        BrowserError::NoSession(_) => true,
        BrowserError::AttachFailed(_) => !chromium_alive,
        _ => false,
    }
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
        let endpoint = self.ensure_chromium(bin, session_key, launch).await?;
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
    ///
    /// `bin` is passed in rather than re-resolved: `run` already resolved it
    /// three lines up, and a second call site for the same fact is how two
    /// answers get created even when both are cached.
    async fn ensure_chromium(
        &self,
        bin: &Path,
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

        let resolved =
            super::chromium_resolve::resolve_binary(&self.runtime, &launch.browser, bin).await?;
        // The replacement for the boot-time `unhonored_managed_fields` warning
        // this round deletes. `find_chromium_preferred` degrades SILENTLY when
        // the requested engine is not installed — it merely reorders candidates
        // and logs the fallback at `debug!` — so without this line "asked for
        // Brave, got Chrome" is reported nowhere. `None` is an unidentifiable
        // path, which is not evidence that the request was honoured, so it
        // warns too rather than being read as agreement (判据 §8).
        if resolved.engine.as_ref() != Some(&launch.browser) {
            tracing::warn!(
                requested = ?launch.browser,
                resolved = ?resolved.engine,
                path = %resolved.path.display(),
                "the managed profile asked for one engine and got another"
            );
        }
        let user_data_dir = chromium_user_data_dir(launch, session_key)?;
        let spec = ChromiumLaunchSpec {
            binary: resolved.path,
            user_data_dir,
            headless: launch.headless,
            proxy: launch.proxy.clone(),
            extra_args: launch.extra_args.clone(),
        };
        tracing::info!(
            session = %session_key,
            binary = %spec.binary.display(),
            source = resolved.source.label(),
            "launching chromium for the managed profile"
        );
        let child = ChromiumChild::spawn(&spec, session_key, DEVTOOLS_PORT_DEADLINE).await?;
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

    /// Whether this session HAD a browser and it has since exited.
    ///
    /// Deliberately not `!chromium_alive`: "there is no browser" and "the
    /// browser died" are different facts and only the second one is a reason to
    /// tear down and relaunch before a verb. Reading the first as the second
    /// would make the pre-verb check fire on every cold profile.
    pub(crate) fn chromium_died(&self, session_key: &str) -> bool {
        let mut map = self.chromium.lock().unwrap_or_else(|e| e.into_inner());
        map.get_mut(session_key).is_some_and(|c| !c.alive())
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
    // endpoint) nor a page-level failure.
    //
    // BOTH phrases are required, and that is the point. This function runs on
    // output that can contain page text — its own doc says so, and
    // `snapshot`/`console` echo the page under `### Result` — so a single
    // anchor on `econnrefused` would let a developer's own error page talk the
    // driver into relaunching a browser. The node error and playwright's own
    // call-log line appear together in the real transcript and not, by
    // accident, in a page.
    if s.contains("econnrefused") && s.contains("retrieving websocket url from") {
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
/// interact.
///
/// Unlike the not-open anchors, this one is a **conjunction**. The not-open
/// pair could each stand alone because both are sentences only this CLI writes;
/// `ECONNREFUSED` is a sentence any program writes, and this function is
/// documented as running on output that can include page text. Requiring
/// playwright's own call-log line beside it is what a page cannot supply by
/// accident. A fourth wording is handled the same way it always was: add it,
/// do not widen an existing anchor.
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

- [ ] **删掉现在没有引用者的三处（Task 4 推迟到这里的部分）。**
  - `src/browser/playwright_launch.rs`：删 `browser_flag_value` 及其上方 doc（**:106-123 整块**——doc 从 :106 起，函数体到 :123 止；用 `:110-121` 会留下四行悬空代码）；删 `open_argv` 及其 doc（:207-234 整块）；删三条只测它们的测试 `headed_puts_the_flag_on_open_where_the_cli_accepts_it`（:389-400）、`headless_omits_the_headed_flag`（:401-408）、`browser_flag_only_carries_values_the_cli_accepts`（:431-457）。
  - `src/browser/playwright_cli.rs`：删 imports 里的 `open_argv`，删 `open_session` 剩下的躯壳（它已被 `attach_session` / `attach_once` 取代）。
  - `src/browser/manager.rs`：删 `:153-163` 那个 `for … unhonored_managed_fields(…)` 的告警循环（保留它前后的代码），删 `:575-591` 的函数及其 doc，删测试 `managed_profiles_name_the_fields_their_driver_drops`（**`:830-875` 整个函数**，`#[test]` 属性在 :830）。
  ⚠️ **删除理由不是「它恒空」**——它的真实条件是 `browser_flag_value(&cfg.browser).is_none() && cfg.browser != BrowserType::default()`（:585-587），即恰好 Brave，删掉 `browser_flag_value` 之后仍表达得出来。删它是因为它保护的事换了位置：那条替代告警现在在 `ensure_chromium` 里，由**执行替换的那一段代码**发出，并且覆盖 `find_chromium_preferred` 静默降级这条它原本够不着的路径。这句话要进 Task 10 的 FEATURE_LOCATOR 条目。
  - `src/browser/profile.rs:36-42`：`ProfileConfig::browser` 的 doc 改为：
    ```rust
    /// Which browser engine to use.
    ///
    /// Honored by both drivers. The managed driver no longer passes the engine
    /// to `playwright-cli` (it launches the browser itself); the value steers
    /// `discovery::find_chromium_preferred`. When the requested engine is not
    /// installed the search degrades to whatever is, and
    /// `PlaywrightCliDriver::ensure_chromium` warns with the engine it actually
    /// resolved — the substitution is reported by the code that performs it,
    /// which is why the old boot-time warning is gone.
    ```
  - `src/browser/playwright_cli_backend.rs:170-173`：注释里的 `playwright_launch::open_argv` 改成 `chromium_launch::ChromiumLaunchSpec::argv`（headedness 现在住在那里）。

- [ ] **跑到绿。** 本任务里 `manager.rs:96-97` 的 `PlaywrightCliDriver::new(...)` 少一个参数，**先把它改掉**（`PlaywrightCliDriver::new(config.playwright_cli.clone(), config.runtime.clone())`），其余 manager 改动留给 Task 6。然后：
  ```
  cargo test -p alephcore --lib browser::
  cargo test -p alephcore --lib --no-run
  ```
- [ ] **证伪四次。**
  1. 把新加的 attach 分支注释掉 → `a_refused_attach_classifies_as_attach_failed` 必须变红。
  2. 把它的 `&&` 改成 `||` → `one_half_of_the_attach_signature_is_not_enough` 必须变红。
  3. 把它的条件改成 `s.contains("error")` → `an_unrelated_failure_is_not_read_as_a_refused_attach` 与 `both_not_open_phrasings_still_produce_no_session` 必须变红（第二条会红是因为 not-open 的 stderr 那句以 `Error:` 开头——这正是为什么锚点必须窄）。
  4. 把 `needs_relaunch` 的 `AttachFailed` 臂改成 `true`（不看 `chromium_alive`）→ `only_a_dead_browser_earns_a_relaunch` 必须变红。**这一条守的是 D.9.10**：一个活着的浏览器不许被第二次启动。
  四次都恢复。
- [ ] `rustfmt src/browser/playwright_cli.rs src/browser/playwright_launch.rs src/browser/manager.rs src/browser/profile.rs src/browser/playwright_cli_backend.rs`（五个都是叶子文件；⚠️ 仍然**不要**碰 `src/browser/mod.rs`）
- [ ] **提交（Task 4 + Task 5 一起）。**
  ```
  git add src/browser/playwright_launch.rs src/browser/playwright_cli.rs \
          src/browser/playwright_cli_backend.rs src/browser/manager.rs src/browser/profile.rs
  git commit -m "browser: attach playwright-cli to Aleph's chromium over cdp

  The lazy launch now ensures Aleph's own Chromium is alive and runs
  'attach --cdp <http-url>' instead of 'open'. open is never emitted again: it
  issues goto('about:blank') on the page it reuses. A dead browser is caught
  two ways - a cheap try_wait before every verb, and an AttachFailed
  classification afterwards - and only a dead one earns a relaunch, because a
  second Chromium on a live profile is D.9.10's double-open in a new costume.
  The attach-refused anchor is a conjunction: this classifier runs on output
  that can carry page text, and ECONNREFUSED alone is a sentence any program
  writes. browser_flag_value and open_argv are cut as dead code;
  unhonored_managed_fields goes because the substitution it warned about is
  now reported by the resolver that performs it, not because it was empty.

  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01TKV5PtutzoBvbT4yTpsyRY"
  ```

---

### Task 6: `ProfileManager` —— `live_endpoint`、收割器杀浏览器、boot 收孤儿

**Files:**
- Modify `src/browser/manager.rs`：`new`（:96-97，Task 5 已改一行）· `spawn_idle_reaper`（:208-263）· `reap_idle`（:306-350）· `idle_managed_profiles`（:352-370）· `session_active`（:372-384）· 新增 `live_endpoint` / `shutdown_browsers` / `shutdown_browsers_global` · **改写现有测试 `test_get_profile_state_removed_in_favor_of_session_active`（:631-643）** · 测试模块新增三条
- Modify `src/browser/mod.rs`（一行 `pub(crate) use chromium_launch::CdpEndpoint;`，crate 内可见即可——`live_endpoint` 本身也是 `pub(crate)`，Plan 2 的视图与它同 crate；**手写，不交给 `rustfmt`**）
- Modify `src/bin/aleph-server/commands/start/mod.rs`：有序停机段落（`:3642` 的 `run_until_shutdown` 之后、`:3658` 的 `kill_all_running_background()` 旁边）加一次浏览器停机

**Interfaces:**
- Consumes: `PlaywrightCliDriver::{endpoint, chromium_alive, shutdown_chromium}`（Task 5）· `chromium_launch::reap_orphans_now`（Task 1，**不再取参数**——它自己解析注册表目录）
- Produces:
  ```rust
  impl ProfileManager {
      pub(crate) fn live_endpoint(&self, profile: &str) -> Option<CdpEndpoint>;
      pub fn shutdown_browsers(&self) -> usize;
  }
  pub fn shutdown_browsers_global() -> usize;   // free fn, mirrors bash_exec::kill_all_running_background
  ```

#### Steps

- [ ] **先改掉本任务会弄红的那条既有测试。** `src/browser/manager.rs:631-643` 的 `test_get_profile_state_removed_in_favor_of_session_active` 最后两行逐字是
  ```rust
        // Managed approximation: tracked tabs imply a live session.
        manager.touch_tab("default", "1");
        assert!(manager.session_active("default"));
  ```
  也就是本任务要反转的那一句。**保留它前面的三条断言**（三个 profile 在没有浏览器时都报 inactive），把最后三行换成：
  ```rust
        // Not an approximation any more: Aleph owns the browser process, so
        // `session_active` asks it. A tracked tab says a tab was USED, which is
        // a different fact and no longer stands in for a live browser.
        manager.touch_tab("default", "1");
        assert!(
            !manager.session_active("default"),
            "a tracked tab must not imply a browser that was never launched"
        );
  ```
  ⚠️ 这条改动**必须和实现同一笔**：不改它，Task 6 的实现会让一条既有测试变红，而红的原因写在另一个文件里。

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

    /// spec §3.6 「退出时杀」. `std::process::Child` does NOT kill on drop, and
    /// under `attach --cdp` the CLI was never the browser's parent — so without
    /// an explicit stop every restart leaves a browser behind until the next
    /// boot sweep finds it.
    ///
    /// The fake browser is a real `sleep` subprocess, because the thing being
    /// tested is that a live pid stops being live. A mock child would assert
    /// that a method was called (判据 §4: assert the effect arrived, not that
    /// the call happened).
    #[tokio::test]
    async fn shutdown_browsers_kills_what_it_launched_and_says_how_many() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let manager = ProfileManager::new(BrowserSystemConfig::default());

        // Nothing launched → nothing to stop, and it must not pretend otherwise.
        assert_eq!(manager.shutdown_browsers(), 0);

        // A stand-in browser: long-lived, harmless, and observable by pid.
        let child = std::process::Command::new("sleep")
            .arg("120")
            .spawn()
            .expect("spawn the stand-in browser");
        let pid = child.id();
        manager.insert_test_child("default", child);
        assert!(
            crate::utils::process_alive::is_process_alive(pid as i32),
            "precondition: the stand-in is running"
        );

        assert_eq!(manager.shutdown_browsers(), 1);
        // Give the OS a moment to reap; `shutdown` already waited, so this is
        // belt and braces rather than a race the assertion depends on.
        assert!(
            !crate::utils::process_alive::is_process_alive(pid as i32),
            "the stand-in browser is still running after shutdown_browsers"
        );
        // Idempotent: a second stop finds nothing and says so.
        assert_eq!(manager.shutdown_browsers(), 0);
    }
```

  ⚠️ `insert_test_child` 是一个 `#[cfg(test)]` 的注入口，写在 `PlaywrightCliDriver` 上（`ChromiumChild` 的字段是私有的，测试要能放进去一个）。它与 `has_tracked_tabs`（`manager.rs:544-547`）同性质、同门控：
```rust
    /// Test-only: hand the driver a browser it did not launch, so the stop path
    /// can be exercised against a REAL pid without a real Chromium.
    #[cfg(test)]
    pub(crate) fn insert_test_child(&self, session_key: &str, child: std::process::Child) {
        let endpoint = CdpEndpoint {
            http_url: "http://127.0.0.1:1".into(),
            ws_url: "ws://127.0.0.1:1/devtools/browser/test".into(),
            pid: child.id(),
        };
        self.chromium.lock().unwrap_or_else(|e| e.into_inner()).insert(
            session_key.to_string(),
            ChromiumChild::from_parts(child, endpoint, std::path::PathBuf::from("/tmp/test-udd"), session_key),
        );
    }
```
  配一个同样 `#[cfg(test)]` 的构造器 `ChromiumChild::from_parts(child, endpoint, user_data_dir, session_key)`（Task 1 的模块里加），以及 `ProfileManager::insert_test_child` 转发一行。**不要**把这两个做成 `pub`：一个能从外面塞进浏览器的口子，就是一个能绕开启动链的口子。

  ⚠️ `AlephHomeEnvGuard` 的真实路径是 `crate::utils::paths::AlephHomeEnvGuard`（`src/tasks/cron/mod.rs:819`、`src/config/save.rs:17` 都这样引用，它本身是 `#[cfg(test)]` 的）。`manager.rs` 的测试模块里加 `use crate::utils::paths::AlephHomeEnvGuard;`。

- [ ] **跑它，看红。** `cargo test -p alephcore --lib browser::manager` → `live_endpoint` 未解析；`a_tracked_tab_no_longer_fakes_a_live_managed_session` 断言失败（现状 `session_active` 就是 `has_tabs`）。

- [ ] **最小实现。**
  1. `spawn_idle_reaper`（:208-263）：在 `*slot = Some(Arc::downgrade(self));`（:243）**之前**插入 boot 孤儿回收。它走**注册表目录**，所以配了自己 `user_data_dir` 的 profile 也在覆盖范围内（初稿只扫派生根，本仓 QA 自己配的 udd 就在根之外，会被整个漏掉）：
```rust
        // Boot hook, and the only one that runs exactly once per SERVED
        // manager (a `ProfileManager` built by a test or a CLI never claims the
        // slot above). Anything Aleph launched before a crash is still running:
        // Chrome does not exit when its parent does, and under `attach` the CLI
        // was never its parent anyway.
        //
        // Off the async worker: the sweep does a `read_dir`, a `sysinfo`
        // refresh per record and possibly a kill, and `with_process_specifics`
        // is documented as syscall-heavy. Detached because boot must not wait
        // for it — nothing downstream reads the count.
        tokio::spawn(async {
            match tokio::task::spawn_blocking(super::chromium_launch::reap_orphans_now).await {
                Ok(0) => {}
                Ok(n) => tracing::info!("reaped {n} orphaned chromium process(es) from a previous run"),
                Err(e) => tracing::warn!(error = %e, "the orphaned-chromium sweep did not complete"),
            }
        });
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
            // second half is the one that actually reclaims anything — and the
            // count below is only earned if it did something. Reporting a
            // reaped profile over a browser that never went away is the
            // "success reported for a no-op" shape (判据 §11).
            if !self.playwright_cli_driver.shutdown_chromium(&name) {
                tracing::warn!(profile = %name, "reap_idle: no chromium to stop for an idle managed profile");
            }
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

  6. `src/browser/mod.rs`：在 `pub use error::BrowserError;`（:21）之后加一行（**手写**，这个文件声明 18 个子模块，`rustfmt` 会递归进去）：
```rust
// Crate-internal: `live_endpoint` is `pub(crate)` too, and its first real
// consumer (the live view, Plan 2) lives in this crate.
pub(crate) use chromium_launch::CdpEndpoint;
```

  7. 停机路径。先在 `manager.rs` 加两个函数（`shutdown_browsers` 放在 `live_endpoint` 之后，自由函数放在文件里 `apply_policy_live` 旁边——它们是同一族：一个进程级句柄，从别处戳一下，没有 manager 时诚实地说 no-op）：
```rust
    /// Kill every browser this manager launched. Returns how many were stopped.
    ///
    /// spec §3.6「退出时杀」. `std::process::Child` does not kill on drop, and
    /// under `attach --cdp` the CLI was never the browser's parent — so without
    /// this every restart leaves a Chromium running until the next boot sweep.
    pub fn shutdown_browsers(&self) -> usize {
        self.playwright_cli_driver.shutdown_all_chromium()
    }
```
```rust
/// Stop the browsers of the manager the running daemon serves.
///
/// Shaped exactly like [`crate::builtin_tools::bash_exec::kill_all_running_background`],
/// for the same reason its comment gives at the shutdown call site: an
/// automatic teardown is best-effort once the runtime itself is being torn
/// down, so the daemon calls this explicitly. Returns 0 — honestly — when no
/// manager is published (a CLI process, a test, or before boot wired one up).
pub fn shutdown_browsers_global() -> usize {
    let handle = LIVE_MANAGER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    handle
        .as_ref()
        .and_then(Weak::upgrade)
        .map_or(0, |mgr| mgr.shutdown_browsers())
}
```
  以及 `PlaywrightCliDriver::shutdown_all_chromium`（Task 5 的模块里加，与 `shutdown_chromium` 并列）：
```rust
    /// Kill and forget every Chromium this driver launched.
    pub(crate) fn shutdown_all_chromium(&self) -> usize {
        let taken: Vec<ChromiumChild> = std::mem::take(
            &mut *self.chromium.lock().unwrap_or_else(|e| e.into_inner()),
        )
        .into_values()
        .collect();
        let n = taken.len();
        for child in taken {
            child.shutdown();
        }
        n
    }
```
  然后在 `src/bin/aleph-server/commands/start/mod.rs` 的有序停机段落里，紧跟 `:3658` 的 `kill_all_running_background()` 那一段之后加：
```rust
    // Same shape and the same reason as the background-bash reap above: the
    // browsers are OUR child processes, `Child` does not kill on drop, and
    // under `attach --cdp` playwright-cli was never their parent. Reached by
    // both signal paths and by a fatal `run_until_shutdown` error. Without it
    // every restart leaves a Chromium behind for the next boot's sweep to
    // find — and on a host where argv is unreadable that sweep declines to
    // act, by design, so the leak would be permanent.
    let browsers = alephcore::browser::manager::shutdown_browsers_global();
    if browsers > 0 {
        tracing::info!(count = browsers, "stopped managed browsers on shutdown");
    }
```

- [ ] **跑到绿（分开跑）。**
  ```
  cargo test -p alephcore --lib browser::manager
  cargo test -p alephcore --lib gateway::handlers::browser_config
  cargo check --bin aleph-server
  ```
- [ ] **证伪三次。**
  1. 把 `session_active` 的 Managed 臂改回 `self.tab_registry.has_tabs(name)` → `a_tracked_tab_no_longer_fakes_a_live_managed_session` **和**改写后的 `test_get_profile_state_removed_in_favor_of_session_active` 必须同时变红。
  2. 把 `live_endpoint` 的 `ExistingSession` 臂改成也去问 driver → `live_endpoint_is_none_without_a_browser_and_never_answers_for_existing_session` 仍会绿（因为没有浏览器）——**这说明第二条守卫此刻是空的**。把它补强：在那条测试里额外断言 `manager.get_driver("user") == Some(BrowserDriver::ExistingSession)`，让「问的是哪个 driver」成为断言的一部分，然后重做这次变异并确认变红。（判据 §3：一条没被证伪过的守卫不算守卫。）
  3. 把 `shutdown_all_chromium` 的 `child.shutdown()` 换成只 `drop(child)` → `shutdown_browsers_kills_what_it_launched_and_says_how_many` 必须变红。**这一条守的正是 `Child` 不在 drop 时 kill 这件事**，也就是这个函数存在的全部理由。
  三次都恢复。
- [ ] `rustfmt src/browser/manager.rs src/browser/playwright_cli.rs src/bin/aleph-server/commands/start/mod.rs`（三个都是叶子文件——`start/mod.rs` 虽然叫 `mod.rs`，`grep -n "^pub mod\|^mod " src/bin/aleph-server/commands/start/mod.rs` 确认它是否声明子模块；**若声明了就不要格式化它**，那一笔只加了五行，手写即可）
- [ ] **提交。**
  ```
  git add src/browser/manager.rs src/browser/mod.rs src/browser/playwright_cli.rs \
          src/bin/aleph-server/commands/start/mod.rs
  git commit -m "browser: the manager owns chromium's lifetime, not the CLI

  reap_idle kills Aleph's Chromium after the close that now only disconnects,
  and stops counting a profile reaped when there was nothing to stop.
  session_active and the reap candidates ask the child process instead of the
  tab registry's self-described approximation. spawn_idle_reaper sweeps the
  previous run's orphans out of the one sidecar registry, so a profile with a
  configured user_data_dir is covered by construction. The daemon now stops its
  browsers on the way out, beside the background-bash reap and for the same
  reason: Child does not kill on drop. live_endpoint is the accessor the live
  view will consume, and answers None for existing-session profiles.

  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01TKV5PtutzoBvbT4yTpsyRY"
  ```

---

### Task 7: Chromium 供给（`PLAYWRIGHT_DOWNLOAD_HOST` 透传）+ doctor 哨兵 `browser/chromium-missing`

> **偏离（前言 §5）**：**不新增 `chromium` 台账 capability**。`src/runtimes/probe.rs:65-83` 的探测只走 PATH，而 Playwright 的 Chromium 永远不在 PATH 上——加一条 spec 得到的是一个恒 `Missing` 的条目（判据 §2 的第四张脸：没装上）。供给已经存在：`src/runtimes/specs.rs:173-176` 的 `PostInstallAction::RunSubcommand { args: &["install-browser", "chromium"], target_dir: None }` 是 `playwright-cli` 的 post-install。本任务给它加镜像透传，并把「没有 Chromium」变成一件看得见的事。

**Files:**
- Modify `src/runtimes/specs.rs:47-59`（`PostInstallAction`）与 `:173-176`（playwright-cli 的 post_install）· `:460-490` 附近的那条断言 post-install 形状的测试（先 `grep -n "install-browser" src/runtimes/specs.rs` 定位，本计划读到的是 `:464` 与 `:480-485`）
- Modify `src/runtimes/post_install.rs:68-78`（`run` 的分派——⚠️ 不是 `:47-59`，那里是 `POST_INSTALL_TIMEOUT_SECS` 与 `run_cmd_with_timeout`）· `:80-107`（`run_subcommand`）· 新增 `config_env` / `config_env_from` 与测试
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

    /// The three budgets that must stay nested, asserted rather than described.
    /// If any one of them moves, this test names which invariant broke instead
    /// of leaving an unreachable arm and an amber doctor to be discovered.
    #[test]
    fn the_check_answers_before_the_engine_abandons_it() {
        assert!(
            crate::browser::chromium_resolve::DRY_RUN_TIMEOUT < RESOLVE_TIMEOUT,
            "the inner probe must finish before this check's own deadline"
        );
        assert!(
            RESOLVE_TIMEOUT < crate::diagnostics::check::DEFAULT_CHECK_TIMEOUT,
            "this check must answer before the engine abandons it and emits its \
             own Warning — otherwise the timeout arm here is unreachable"
        );
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
//! [`RESOLVE_TIMEOUT`], and a probe that does not answer in time produces
//! [`crate::diagnostics::check::unknown_finding`] — the house style for "this
//! check could not determine its own subject" (`src/diagnostics/check.rs:205-225`:
//! `Severity::Warning`, titled `"<subject> unknown"`, spelled once so unknown
//! keeps meaning the same severity everywhere). Never "not installed": unknown
//! is neither healthy nor failed (判据 §8).

use async_trait::async_trait;

use crate::browser::chromium_resolve::{resolve_binary, ChromiumSource};
use crate::browser::profile::{BrowserRuntimeConfig, BrowserType};
use crate::diagnostics::check::{unknown_finding, HealthCheck, Posture};
use crate::diagnostics::finding::Finding;

const ID: &str = "browser/chromium-missing";
const SUBJECT: &str = "Managed browser";

/// The outer bound on the whole resolution, and the number is chosen by two
/// constraints, not by taste.
///
/// **Below** it: `chromium_resolve::DRY_RUN_TIMEOUT` is 6 s, the only thing in
/// the resolution that can block. **Above** it: `check::DEFAULT_CHECK_TIMEOUT`
/// is 20 s (`src/diagnostics/check.rs:27`), and past that the ENGINE abandons
/// the check and emits a `Warning` of its own. A check whose inner deadline
/// sits at or above the engine's is a 恒假 arm (判据 §2) plus an amber
/// `doctor` on every slow probe — and `src/diagnostics/checks/mod.rs:6-10`
/// names exactly that as the way this command's exit code becomes a constant.
/// Three budgets, strictly nested: 6 < 8 < 20, so this check always gets to
/// answer for itself and never needs a `HealthCheck::timeout()` override.
const RESOLVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

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

/// The fix-hint sentence, reachable from `builtin_tools::runtime_manage`'s test
/// so the tool it names can be pinned to a tool that exists. Exposing the
/// finding rather than the string keeps one author for the sentence.
#[cfg(test)]
pub(crate) fn missing_finding_for_test() -> Finding {
    missing_finding("no system browser")
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
        // Off the async worker, mirroring the twin probe at
        // `browser_runtime.rs:230-236`, which wraps the identical call for the
        // identical reason: it does a `which` PATH walk plus a JSON file read
        // (判据 §16 — fix it on both sides).
        let cli = match tokio::task::spawn_blocking(
            crate::tools::probes::browser::managed_cli_path,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                return vec![unknown_finding(
                    ID,
                    SUBJECT,
                    format!("the playwright-cli lookup did not come back: {e}"),
                )]
            }
        };
        let Some(cli) = cli else {
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
            Ok(Ok(r)) => found_finding(&r.path, r.source),
            Ok(Err(crate::browser::BrowserError::ChromiumUnavailable { tried })) => {
                missing_finding(tried)
            }
            // Any other error is the resolver failing to look, not a verdict.
            Ok(Err(e)) => unknown_finding(ID, SUBJECT, format!("the lookup failed: {e}")),
            // The check's OWN "could not verify" answer, which is why
            // RESOLVE_TIMEOUT sits under the engine's ceiling: if the engine
            // got here first, this arm would be unreachable and the operator
            // would read the engine's abandonment Warning instead of a sentence
            // naming what was being probed.
            Err(_) => unknown_finding(
                ID,
                SUBJECT,
                format!(
                    "the chromium lookup did not answer within {}s (engine ceiling is {}s)",
                    RESOLVE_TIMEOUT.as_secs(),
                    crate::diagnostics::check::DEFAULT_CHECK_TIMEOUT.as_secs()
                ),
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

- [ ] **跑到绿（分开跑）。**
  ```
  cargo test -p alephcore --lib diagnostics::
  cargo test -p alephcore --lib runtimes::
  ```
- [ ] **证伪两次。** ① 把 `missing_finding` 的 `fix_hint` 删掉 → `the_missing_finding_names_every_way_out` 必须变红。② 把 `RESOLVE_TIMEOUT` 改回 25 s → `the_check_answers_before_the_engine_abandons_it` 必须变红。两次都恢复。
- [ ] **手工跑一次真 doctor。** `cargo run --bin aleph-server -- doctor 2>&1 | grep -A 3 -i chromium`。本机预期：`playwright-cli` 在 PATH 上、系统 Chrome 在 `/Applications` 下 ⇒ `Managed browser available … a system Chromium-family browser`。把这一行原样贴进 Task 10 的 FEATURE_LOCATOR 条目。⚠️ `aleph-server doctor` 是冷进程，`default_registry` 里任何恒红的检查都会把它的退出码变成常数——本检查的两条 Info 与一条 Warning（unknown）里，只有 unknown 是 Warning，且它只在「读不到配置 / 探针不答」时出现。**跑一次确认退出码仍是 0。**
- [ ] `rustfmt src/runtimes/specs.rs src/runtimes/post_install.rs src/diagnostics/checks/chromium_missing.rs`
  ⚠️ **`src/diagnostics/checks/mod.rs` 与 `src/diagnostics/mod.rs` 不在这一行里，也不许加进来。** 两者都声明子模块（`checks/mod.rs:16-33` 一口气 18 个 `pub mod`），而 `rustfmt <file>` 会**递归进它声明的每一个子模块**，把整棵 `src/diagnostics/` 重排进本次提交——正是本计划 Global Constraints 那条禁令要防的事。这一笔对它们只加了三行（一个 `pub mod`、一个 `pub use`、一个注册向量条目），**手写**，没有可格式化的东西。
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
- Modify `src/builtin_tools/mod.rs`（`pub mod` 按字母序插在 `remember` 与 `scratchpad` 之间；若同文件有对应的 `pub use` 区块，一并加）—— **手写，不交给 `rustfmt`**：这个文件声明每一个 builtin-tool 模块，`rustfmt` 会递归重排整棵 `src/builtin_tools/`
- Modify `src/gateway/method_authz.rs:31-…`（`OPERATOR_TOOLS`）与它的测试（`:251-260` 的 `operator_tools_has_no_duplicates`、`:261-290` 的 `chat_safe_tools_stay_open`）
- Modify `src/executor/builtin_registry/definitions.rs`：`BUILTIN_TOOL_DEFINITIONS` 加条目（照 `:248-252` 的 `list_models` 形状）· `standalone` 的 `=> None` 臂区（`:1170-1186` 一带）· `REGISTRY_SCHEMA_BASELINE`（`:3027+`）加一行 · 必要时抬 `CATALOG_DESCRIPTION_CEILING_BYTES`（`:2603`）与 `REGISTRY_SCHEMA_CEILING_BYTES`（`:2999`）
- Modify `src/executor/builtin_registry/groups.rs`（`:100-137` 的自管理组，`"doctor"` 附近 —— ⚠️ 这张表**只用于展示**，加进去不构成任何授权判断；授权在 `method_authz.rs`，见下）
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

**授权裁定（review 采纳，初稿答错了表）：`runtime_manage` 进 `method_authz::OPERATOR_TOOLS`。** 初稿只说了 `groups.rs`「按它自己的模块 doc 只用于展示，不带授权含义」——那句话是真的，但它不是这个问题的答案。真正的闸是 `src/gateway/method_authz.rs:31` 的 `OPERATOR_TOOLS`（chat-tier 通道在工具派发处被拒），而这个工具最近的三个兄弟 `skill_install` / `skill_manage` / `hub_install_run` 全在里面。`runtime_manage{install}` 会跑 `ensure_capability`，也就是台账的 bootstrap 安装器（npm 全局安装、`curl | sh` 脚本）与 post-install 子命令——一次调用在宿主机上装软件。所以它进 `OPERATOR_TOOLS`。⚠️ 不按动作拆（`list` 开放、`install` 收紧）：那张表按**工具名**判定，拆开要动判定机制本身，而 `list` 的价值不足以换那个改动；chat tier 想知道装了什么，`doctor` 仍然开放。

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

    /// A locator that answers from memory. The production one shells out to
    /// `playwright-cli install-browser --dry-run`, and a unit test that reached
    /// it would spawn a real node subprocess inside `cargo test --lib` — the
    /// exact discipline `src/browser/playwright_cli.rs:150-165` seals its own
    /// `provision_binary` to enforce, in its own words "so no future test can
    /// forget to seal itself", because otherwise "their green was a property of
    /// the environment, not of the code". It would also pass on a machine with
    /// no `playwright-cli` for a different reason than on one with it.
    struct StubLocator(&'static str);

    #[async_trait]
    impl ChromiumLocator for StubLocator {
        async fn locate(&self) -> RuntimeRow {
            RuntimeRow {
                name: "chromium".into(),
                status: self.0.into(),
                path: None,
                version: None,
                purpose: None,
                supported_here: true,
            }
        }
    }

    /// The catalogue face and the RPC face answer from the same table. A tool
    /// that listed a different set than `runtimes.list` would be the second
    /// answer to "what runtimes are there" (判据 §9).
    #[tokio::test]
    async fn list_answers_from_the_same_spec_table_as_the_rpc() {
        let out = RuntimeManageTool::with_locator(Arc::new(StubLocator("Ready (stub)")))
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

    /// The doctor's fix hint (Task 7) names this tool by string. Nothing
    /// otherwise ties the two together, so a rename would quietly turn that
    /// hint into a lie — the "same fact, two expressions" shape, with the
    /// expensive copy in the text a human is told to act on (判据 §1).
    #[test]
    fn the_doctor_fix_hint_names_a_tool_that_actually_exists() {
        let hint = crate::diagnostics::checks::chromium_missing::missing_finding_for_test()
            .fix_hint
            .expect("the missing finding carries a fix hint");
        assert!(hint.contains(<RuntimeManageTool as AlephTool>::NAME), "{hint}");
        assert!(
            crate::executor::BUILTIN_TOOL_DEFINITIONS
                .iter()
                .any(|d| d.name == <RuntimeManageTool as AlephTool>::NAME),
            "the tool the doctor points at must be in the catalogue"
        );
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

/// Where the tool learns about Chromium.
///
/// Injected because the production answer shells out to `playwright-cli
/// install-browser --dry-run`, and `cargo test --lib` must not spawn node.
#[async_trait]
pub(crate) trait ChromiumLocator: Send + Sync {
    async fn locate(&self) -> RuntimeRow;
}

/// The production locator: the resolver the browser driver itself uses.
pub(crate) struct RealChromiumLocator;

#[async_trait]
impl ChromiumLocator for RealChromiumLocator {
    async fn locate(&self) -> RuntimeRow {
        chromium_row().await
    }
}

#[derive(Clone)]
pub struct RuntimeManageTool {
    locator: Arc<dyn ChromiumLocator>,
}

impl Default for RuntimeManageTool {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for RuntimeManageTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeManageTool").finish_non_exhaustive()
    }
}

impl RuntimeManageTool {
    #[must_use]
    pub fn new() -> Self {
        Self { locator: Arc::new(RealChromiumLocator) }
    }

    #[cfg(test)]
    pub(crate) fn with_locator(locator: Arc<dyn ChromiumLocator>) -> Self {
        Self { locator }
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

    async fn list(locator: &Arc<dyn ChromiumLocator>) -> RuntimeManageOutput {
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
        runtimes.push(locator.locate().await);
        RuntimeManageOutput {
            ok: true,
            message: format!("{} runtime(s).", runtimes.len()),
            runtimes,
        }
    }

    async fn install(
        capability: Option<String>,
        locator: &Arc<dyn ChromiumLocator>,
    ) -> RuntimeManageOutput {
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
        let mut out = Self::list(locator).await;
        out.message = message;
        out
    }
}

/// The chromium row, derived from the driver's own resolver.
async fn chromium_row() -> RuntimeRow {
    // Off the async worker, same as the doctor's twin probe: a `which` PATH
    // walk plus a JSON read (判据 §16).
    let cli = tokio::task::spawn_blocking(crate::tools::probes::browser::managed_cli_path)
        .await
        .unwrap_or(None);
    let (status, path) = match cli {
        None => ("Unknown (no playwright-cli)".to_string(), None),
        Some(cli) => match crate::config::Config::load() {
            // A config we cannot read is NOT a config with default settings: a
            // pinned `binary_path` we failed to see would make this row say
            // "Missing" on a host that has a browser. The doctor answers this
            // condition with `unknown`; the tool must say the same thing, or
            // the two faces disagree about the same fact (判据 §16).
            Err(e) => (format!("Unknown (the config could not be read: {e})"), None),
            Ok(cfg) => {
                match crate::browser::chromium_resolve::resolve_binary(
                    &cfg.general.browser.runtime,
                    &crate::browser::profile::BrowserType::default(),
                    &cli,
                )
                .await
                {
                    Ok(r) => (
                        format!("Ready ({})", r.source.label()),
                        Some(r.path.display().to_string()),
                    ),
                    Err(e) => (format!("Missing ({e})"), None),
                }
            }
        },
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
        // Exit 0 is not the claim. The claim is that the NEXT browser call
        // works, and the resolver is one await away — so ask it (判据 §4:
        // assert the effect arrived, not that the call happened). This CLI has
        // produced exit-0-and-nothing-happened before: appendix D.9.11 records
        // `browser_pdf` answering "Saved PDF to <path>" over a file it had been
        // refused permission to write.
        Ok(Ok(out)) if out.status.success() => match chromium_row().await {
            row if row.path.is_some() => format!(
                "chromium installed and resolves at {}.",
                row.path.unwrap_or_default()
            ),
            row => format!(
                "`install-browser chromium` exited 0 but no browser resolves afterwards ({}). \
                 Check [browser.runtime] binary_path and download_host.",
                row.status
            ),
        },
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
            RuntimeAction::List => Self::list(&self.locator).await,
            RuntimeAction::Install => Self::install(args.capability, &self.locator).await,
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
  3. `groups.rs`：把 `"runtime_manage"` 加进含 `"doctor"` / `"tool_usage"` 的那一组（`:100-137`）。这张表按它自己的模块 doc **只用于展示**，所以这一步不回答任何授权问题——授权在下一步。
  3b. `src/gateway/method_authz.rs`：把 `"runtime_manage"` 加进 `OPERATOR_TOOLS`（:31 起），紧挨 `"skill_install"` / `"skill_manage"` / `"hub_install_run"`，并按那张表的行文写下理由：
```rust
    // `runtime_manage{install}` runs `ensure_capability`, i.e. the ledger's
    // bootstrap installers — an npm global install, a `curl … | sh` script, a
    // winget invocation — plus their post-install subcommands. One call
    // installs software on the host. Its three nearest siblings
    // (`skill_install`, `skill_manage`, `hub_install_run`) are already here for
    // the same reason. Deliberately NOT split so `list` stays open: this table
    // matches on the tool NAME, and a chat-tier run that wants to know what is
    // installed has `doctor`, which is open.
    "runtime_manage",
```
  该文件已有的两条测试（`operator_tools_has_no_duplicates` :251-260、`chat_safe_tools_stay_open` :261-290）会自动覆盖新成员；再加一条断言它确实被闸住：
```rust
    #[test]
    fn installing_a_runtime_is_operator_only() {
        assert!(
            tool_requires_operator("runtime_manage"),
            "runtime_manage installs software on the host; it sits with \
             skill_install and hub_install_run, not with the read-only tools"
        );
    }
```
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

- [ ] **先量一次每请求字节，再动棘轮。** R9 点名的量具是 `aleph-server prompt-size`，所以在加工具**之前**与**之后**各跑一次并记下两个数：
  ```
  cargo run --bin aleph-server -- prompt-size
  ```
  没有这两个数，一个手打错的 ceiling 与一个实测出来的 ceiling 在提交里长得一模一样（判据 §18）。
- [ ] **跑两条棘轮，按它们印出来的数字改。** `cargo test -p alephcore --lib executor::builtin_registry::definitions` → 预期 `catalog_description_bytes_ratchet` 与 `registry_schema_bytes_ratchet` 双红。**先按 R9 的两把尺量这段 `DESCRIPTION`**：① 这是模型做不到的运行时事实吗？——是：「哪些运行时存在、`install` 要一个 `capability`、chromium 是那个浏览器」在 schema 的枚举里看不出来。② 有没有别的工具拥有这句话？——没有：`doctor` 报告健康但不装东西。**先修剪再抬**：如果实测增量超过 400 B，把描述里能由 `RuntimeAction` 枚举 doc 说出来的部分删掉再量一次。然后把两个 ceiling 常量改成测试**打印出来的**那个数（flush，不留 headroom——那条常量的 doc `:2369-2384` 逐字论证过为什么 headroom 是已经发出去的额度），并在常量 doc 里追加一行注明本轮增量与归因。同样把 `REGISTRY_SCHEMA_BASELINE`（`:3027+`）加一行 `("runtime_manage", <测出来的数>)` —— **不要手编这一行**，用测试打印的值。
- [ ] **跑到绿（三条命令，别合并——`cargo test` 只收一个 TESTNAME）。**
  ```
  cargo test -p alephcore --lib builtin_tools::runtime_manage
  cargo test -p alephcore --lib executor::builtin_registry
  cargo test -p alephcore --lib gateway::method_authz
  ```
- [ ] **证伪三次。**
  1. 把 `is_installable` 改成只 `find_spec(name).is_some()` → `chromium_is_installable_and_is_deliberately_not_a_ledger_spec` 必须变红。
  2. 把 `OPERATOR_TOOLS` 里那一行删掉 → `installing_a_runtime_is_operator_only` 必须变红。
  3. 把 `RuntimeManageTool` 的 `NAME` 改成 `"runtimes_manage"` → `the_doctor_fix_hint_names_a_tool_that_actually_exists` 必须变红。**这条守的是 doctor 那句 fix hint 不许变成谎话。**
  三次都恢复。
- [ ] `rustfmt src/builtin_tools/runtime_manage.rs src/executor/builtin_registry/definitions.rs src/executor/builtin_registry/groups.rs src/executor/builtin_registry/registry/struct_def.rs src/executor/builtin_registry/registry/tool_registry_impl.rs src/executor/builtin_registry/builder/core_tools.rs src/gateway/method_authz.rs`
  ⚠️ **`src/builtin_tools/mod.rs` 与 `src/executor/builtin_registry/builder/constructor/mod.rs` 不在这一行里。** 前者声明每一个 builtin-tool 模块，`rustfmt <file>` 会递归重排整棵 `src/builtin_tools/`；后者先 `grep -n "^pub mod\|^mod " src/executor/builtin_registry/builder/constructor/mod.rs` 确认，**声明了子模块就手写那两行**（一个 `let` 与结构体字面量里的一行），它们没有可格式化的东西。
- [ ] `cargo test -p alephcore --lib --no-run` · `cargo test -p alephcore --bins` · `cargo test -p alephcore --features test-helpers --test '*' --no-run` 全绿。
- [ ] **提交。**
  ```
  git add src/builtin_tools/runtime_manage.rs src/builtin_tools/mod.rs \
          src/executor/builtin_registry src/gateway/method_authz.rs
  git commit -m "tools: runtime_manage puts the runtime ledger in the conversation

  The runtimes.* family had a Panel face and no tool face, so 'chromium is not
  installed' was a dead end for the model. runtime_manage lists the same spec
  table the RPC lists and installs by name. chromium is installable without
  being a RuntimeSpec: the ledger probes PATH and Playwright's browser is never
  on it, so a spec would sit at Missing forever; the install re-runs the very
  argv the playwright-cli post-install already uses, with the same mirror env,
  and reports whether a browser resolves afterwards rather than trusting exit 0.
  It joins OPERATOR_TOOLS beside skill_install and hub_install_run: one call
  installs software on the host. The Chromium lookup is injected so no unit
  test spawns node.

  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01TKV5PtutzoBvbT4yTpsyRY"
  ```

---

### Task 9: 真机 QA —— `qa/browser_managed/run.sh attach` + 既有九场景回归

**Files:**
- Modify `qa/browser_managed/run.sh`：用法头（:1-25）· 场景白名单（:29-32）· `add_browser_config.py` 参数装配（:207-261）· 驱动分派（:350-400）
- Modify `qa/browser_managed/add_browser_config.py`：新增 `--runtime-binary-path` / `--prefer-system-browser` **两个**参数与对应 `[general.browser.runtime]` 写入（照 `:126-135` 的 `set_key` 列表）。⚠️ 初稿在这里还写过一个 `--chromium-udd-root`，**它不存在也不需要**：`attach` 场景的 udd 就是 `run.sh:85` 的 `UDD="$QA_ROOT/browser-profile"`，经既有的 `--user-data-dir` 传下去；sidecar 注册表在 `$ALEPH_HOME/data/browser/chromium/`，由 scratch HOME 决定。这一行已删。
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
import signal
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
ap.add_argument("--server-pid", type=int, required=True,
                help="the aleph-server to stop, so the exit-time browser kill can be observed")
args = ap.parse_args()

_led = Ledger()
log = Ledger.log
check = _led.check

PORT_FILE = os.path.join(args.expect_user_data_dir, "DevToolsActivePort")
# The registry, NOT a file inside the udd: a profile may point its
# `user_data_dir` anywhere, so the record that lets a boot sweep find the
# browser lives in one place derived from ALEPH_HOME.
SIDECAR = os.path.join(
    os.environ["ALEPH_HOME"], "data", "browser", "chromium", "default.json"
)


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
        # Attributable, not merely failing: the refusal has to be ABOUT the
        # missing browser. `browser_navigate` on a fresh profile would fail for
        # want of a tab too, and a control that passes for the wrong reason is
        # not a control.
        text = json.dumps(body)
        check(
            "a non-launching verb fails on a fresh profile, and says the browser is not open (control)",
            (not ok) and ("not open" in text.lower() or "no tabs" in text.lower()),
            text[:200],
        )
        check("no port file before anything launched (control)", read_endpoint() is None, PORT_FILE)
        check("no sidecar before anything launched (control)", not os.path.exists(SIDECAR), SIDECAR)

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

        # 5b. The record that makes an orphan reclaimable. No unit test can see
        #     this: `write_sidecar` is best-effort and a launch that skipped it
        #     still reports success, so the only place the omission shows up is
        #     here (the plan's Task 1 says so, and this is that claim).
        sidecar = {}
        try:
            with open(SIDECAR) as fh:
                sidecar = json.load(fh)
        except OSError as e:
            log("  sidecar unreadable:", e)
        check("the sidecar registry holds this profile's record", bool(sidecar), SIDECAR)
        check(
            "its pid is one of the live Chrome processes",
            str(sidecar.get("pid")) in pids,
            f"sidecar pid={sidecar.get('pid')} pgrep={pids}",
        )
        check(
            "it records the user-data-dir, which is how the boot sweep matches argv",
            sidecar.get("user_data_dir") == args.expect_user_data_dir,
            str(sidecar.get("user_data_dir")),
        )

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

    # 8. spec §3.6 「退出时杀」. The websocket is closed by now; stop the daemon
    #    the way an operator does and require the browser to go with it.
    #    `Child` does not kill on drop, so without the explicit shutdown hook
    #    this claim fails and the browser survives every restart.
    os.kill(args.server_pid, signal.SIGTERM)
    for _ in range(60):
        if not chrome_pids(args.expect_user_data_dir):
            break
        await asyncio.sleep(0.5)
    check(
        "SIGTERM to aleph-server leaves no Chrome carrying its user-data-dir",
        not chrome_pids(args.expect_user_data_dir),
        " ".join(chrome_pids(args.expect_user_data_dir)),
    )
    check(
        "and the sidecar record is gone with it",
        not os.path.exists(SIDECAR),
        SIDECAR,
    )

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
      # machine happens to have, and so the RED control has one thing to break.
      # `find_chromium`'s own first macOS path; on Linux/Windows set
      # ALEPH_QA_CHROME to override.
      CHROME_BIN="${ALEPH_QA_CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
      # The precheck is skippable BY A DOCUMENTED FLAG, not by editing this
      # file. The RED control needs to reach the fail-closed message with a
      # deliberately broken pin, and a control that only exists while a file is
      # locally modified is not repeatable and will not exist next round.
      if [ ! -x "$CHROME_BIN" ] && [ -z "${ALEPH_QA_ALLOW_MISSING_CHROME:-}" ]; then
        echo "no browser at $CHROME_BIN; set ALEPH_QA_CHROME, or set" >&2
        echo "ALEPH_QA_ALLOW_MISSING_CHROME=1 to drive the fail-closed path on purpose" >&2
        exit 69
      fi
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
          --expect-user-data-dir "$UDD" \
          --server-pid "$SERVER_PID" || RC=$?
        # The driver SIGTERMs the server as its last claim, so the trap's own
        # kill would report a dead pid. Clear it rather than let cleanup print
        # a confusing failure.
        SERVER_PID=""
        ;;
    ```

- [ ] **搬 `drive_browser.py` 的预言机。** 把它里面用 `--expect-user-data-dir` 对 `cli_sessions(...)` 输出做的断言，改成对 `<udd>/DevToolsActivePort` + `/json/version` 的断言（把 `drive_attach.py` 的 `read_endpoint` / `http_json` 两个函数提到 `qa_rpc.py` 里共用，**不要**复制第二份——两个驱动可以对断言什么有分歧，不许对怎么读端点有分歧，这正是 `qa_rpc.py` 模块 doc 写的纪律）。同时更新 `drive_browser.py` 的 docstring「The oracle」那一段，说明为什么换了。

- [ ] **跑绿。**
  ```bash
  ./qa/browser_managed/run.sh attach
  ```
  预期 `VERDICT: PASS`。**claim 的条数由脚本自己数**——不要在这里写死一个数字然后照抄（判据 §18：数字要带着它测的谓词和它测于哪个 commit）。把实际条数记进 Task 10 的验证行。
- [ ] **跑红（控制组，一条可重复的命令，不要改文件）。**
  ```bash
  ALEPH_QA_CHROME=/nonexistent/chrome ALEPH_QA_ALLOW_MISSING_CHROME=1 ./qa/browser_managed/run.sh attach
  ```
  期望 `browser_open` 失败，且错误文本里出现 `playwright-cli install-browser chromium`（`ChromiumUnavailable` 的 fail-closed 文案），`VERDICT: FAIL`。把那一行错误原样记进 Task 10。
- [ ] **给 `reap` 场景补一条 claim（它现在没有）。** 在 `drive_tools.py` 的 `reap` 分支末尾加：收割之后，`pgrep -f "--user-data-dir=<被收割 profile 的 udd>"` 必须为空。没有这一条，「收割器杀掉了浏览器」在真机上无人证明——而 `close` 在 attach 之下只是断开，所以这正是那半个新行为。
- [ ] **变异验证（三处，spec §6.4 的纪律）。**
  1. 去掉 `chromium_launch.rs` 里 `me.write_sidecar().await;` → `run.sh attach` 的 claim 5b 三条必须变红。（单测覆盖不到这条线：`reap_orphans_*` 自己写 sidecar，`write_sidecar` 是 best-effort 且失败时启动照样成功。这就是为什么这条断言在真机上。）
  2. 去掉 `reap_idle` 里的 `shutdown_chromium(&name)` → `run.sh reap` 上一步新加的那条 claim 必须变红。
  3. 去掉 `start/mod.rs` 里新加的 `shutdown_browsers_global()` → `run.sh attach` 的第 8 组两条 claim 必须变红。**这一条是 spec §3.6 唯一的真机证据。**
  三处都恢复。
- [ ] **既有场景回归（九个，`run.sh:30` 的白名单逐字数过——spec §6.4 写「八场景」，实际是九个，判据 §6「先数一遍」），逐个跑，逐个记结果。**
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
  - **⑤ 一条告警搬家，而不是一条恒空的谓词——本轮自查里改掉的一个错判。** 初稿把 `unhonored_managed_fields` 的删除写成「它恒空了」，那是错的：它的真实条件是 `browser_flag_value(&cfg.browser).is_none() && cfg.browser != BrowserType::default()`（`manager.rs:585-587`），即**恰好 Brave**，删掉 `browser_flag_value` 之后这个条件仍表达得出来。删它的真理由是它保护的事换了位置。而且「我们自己起 Brave 所以 honor 了」只是**半个替换**：`find_chromium_preferred` 在首选引擎不存在时**静默降级**（`prefer_paths` 只重排，回落只打 `debug!`），那条路径原本就没人报。现在 `chromium_resolve::resolve_binary` 返回**实际解析出的引擎**（`discovery::engine_of`，与排序共用同一张 `engine_hints` 表），`ensure_chromium` 在与请求不一致时 `warn!`——告警由**执行替换的那一段代码**发出，并且覆盖了旧告警够不着的那条路径。`browser_flag_value` / `open_argv` 与它们的三条测试一起 CUT（纯死码）。判据：**删一个东西之前先说出它的谓词，再说出谁接手它保护的那件事**。
  - **⑥ 第三种「拒绝」的措辞（接 附录 D.9.13）。** 惰性启动的触发从两句「没开」变成三句：新增的一句是 attach 被拒。实测（0.1.8 / node 24.14.1）：退出 1、**stdout 空**、stderr 是 node 异常 `Error: connect ECONNREFUSED 127.0.0.1:1` 加一行 `- <ws preparing> retrieving websocket url from http://127.0.0.1:1`。它与两句旧锚点**零公共子串**，所以分类器新增一支不会遮蔽旧的；两条新锚点都留着，因为第四种措辞更可能像其中之一。分类结果**不是** `NoSession`（那会朝同一个死端点再 attach 一次），是 `AttachFailed` → 忘掉子进程 → 重启一次 → 再 attach，界限恰好一次。
  - **⑦ 一个平台路径表，问装它的人比自己猜便宜。** 提议里的解析器要硬编码 `~/.cache/ms-playwright/chromium-*/`。本机实测两处都是错的：macOS 的缓存根是 `~/Library/Caches/ms-playwright/`，可执行文件叫 `chromium-1228/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing`（不是 `Chromium.app`）。改为跑 `playwright-cli install-browser chromium --dry-run`，它逐行打印 `Install location:`；再在那**一个**目录里按候选名找可执行文件。判据 §1：装它的那个二进制既装它又说它在哪，只有一份推导；三张平台表会各自腐烂。⚠️ 解析要锚在段头 `(playwright chromium v`——`chromium-headless-shell` 以 `chromium` 开头，子串匹配会选中一个没有浏览器的目录。
  - **⑧ 台账早就在装 Chromium，只是名字变过。** spec §9 曾把「现有运行时台账是否已经会跑 `playwright install`」列为未验证。答案是**会**：`src/runtimes/specs.rs` 里 `playwright-cli` 的 post-install 就是 `install-browser chromium`（v0.1.14 改的名）。所以本轮**没有**新增 `chromium` capability——`runtimes::probe` 只走 PATH，而 Playwright 的浏览器永远不在 PATH 上，加一条 spec 只会得到一个恒 `Missing` 的条目（判据 §2 第四张脸：没装上）。做的是给那条已有动作加 `PLAYWRIGHT_DOWNLOAD_HOST` 透传（空串读作「没有镜像」，不是「镜像是空主机」）、一个 doctor 哨兵 `browser/chromium-missing`（**跑与启动路径同一个解析器**，判据 §9），与一个 R8 工具面 `runtime_manage{list,install}`（`runtimes.*` RPC 家族此前只有 Panel 一张脸）。
  - **⑨ 一个行为变更，写下来而不是留给人发现：Managed profile 不能再「在内存里」。** `DevToolsActivePort` 写在 user-data-dir 里，没有 profile 目录就没有可发现的端点。所以没配 `user_data_dir` 的 profile 现在拿到一个派生目录 `~/.aleph/data/browser/chromium-udd/<key>`，浏览状态（cookie / localStorage）从此对**每个** Managed profile 跨重启存活，而不只是主动要求过的那些。⚠️ **连带后果**：`Child::kill()` 在 unix 上是 SIGKILL，被 SIGKILL 的 Chrome 一定在 profile 目录里留下 `SingletonLock` / `SingletonSocket` / `SingletonCookie` 与一个「did not shut down correctly」标记，下次启动可能弹恢复提示。本轮**没有**做 SIGTERM-然后-SIGKILL 的宽限，也没有清理陈旧 singleton 锁——记在这里，因为它现在会发生在**每一个** Managed profile 上，而不再只是配过目录的那几个。
  - **⑩ sidecar 放一个注册表目录，不是放各自的 udd 里。** 初稿把记录写进每个 profile 的 user-data-dir，于是 boot 清扫只能扫「派生出来的那个根」，而配了 `user_data_dir` 的 profile（本仓 QA 自己就配）的记录落在根之外、**永远扫不到**。改为 `~/.aleph/data/browser/chromium/<profile>.json`，每份记 `{pid, http_url, user_data_dir, aleph_version}`；清扫只走那一个目录。判据 §12：「有哪些浏览器要收」这件事只能有一个推导点。
  - **⑪ 拿来比对的那个东西本身就选错了——「读哪一份 argv」比「怎么比对」更靠前。** 孤儿清扫要按 pid 读 argv 再比对我们的 `--user-data-dir`。最顺手的读法是 `gateway::pty::foreground::fact_for_pid(pid).cmdline`，而它的 doc 逐字写着「The whole command line, **space-joined**」（`foreground.rs:142-143`）——在那个字符串上比对**只能**是 `str::contains`，token 相等在那条路上根本表达不出来。而这条谓词授权的动作是 SIGKILL。两条假阳性，都不需要什么巧合：① **前缀撞车**，清扫遍历同一个根下的各个 profile，它构造的 flag 天然互为前缀，`--user-data-dir=<root>/default` 是活着的 `--user-data-dir=<root>/default-2` 的子串，于是**邻居 profile 的浏览器被杀**——正是这条检查要防的事，栽在它最可能的邻居上（`sanitize_session_key` 产出 `work` / `work-archive` 这类名字是常态）；② **macOS 的 argv/env 渗漏**，本仓早已实测并钉住：`crates/agent-detect/src/engine.rs:427-431` 逐字记着「改写 process.title 的进程（每个 Node CLI 都改）会让 `sysinfo::cmd()` 读过 argv 区进环境」，`:938-957` 钉着一条真实读数，一个值里带空格的环境变量把 `prefer` / `modern` / `like` 几个裸词撒进了命令行。那个模块的防御是**分词 + 跳过 `VAR=value` + 取第一个操作数而不是扫描**（`:944-948` 写明「argv 在环境之前，所以第一个操作数总是先到；扫描会找到运维随手写进 prompt 的某个词」）。判据 §16：孪生子系统已经回答过这个问题，答案要搬过来。
    修法是**换读者**，一次修好两个：读 `sysinfo::Process::cmd()`（`Vec<OsString>`，就是 argv 向量本身）而不是那一行拼好的字符串，比对用整 token 相等（并且两种写法都认——`--user-data-dir=<path>` 与两 token 的 `--user-data-dir <path>`，Chrome 两种都收）。顺带把状态从 `Option` 变成三态 `ArgvProbe{Absent, Unreadable, Argv}`：`Option` 分不出「没有这个进程」与「有但读不出 argv」（Windows 上后者是常态），而初稿两种都删记录——第二种下浏览器还活着而唯一能再找到它的东西没了，判据 §8 撞上 §15（一次性的闩漏一次就是永远）。现在四条臂：匹配 → 杀并删记录；`Argv` 但不匹配（pid 被回收）→ 不杀、删记录；`Absent` → 删记录；`Unreadable` → **什么都不做，记录留着**。⚠️ 读者建在 `utils::process_alive::with_process_specifics` 之上而不是自己新起一个 `System`——那个 helper 的 doc 自己写着它是本仓单 pid `sysinfo` 惯用法的唯一所有者。
  - **⑫ 「Chrome 中途死」有两种说法，初稿只接住一种。** `run()` 原本只在 `NoSession` 上重启；但浏览器死掉时 CLI 报的可能是 `ECONNREFUSED`（分类为 `AttachFailed`），那就一路透传给模型，浏览器**这个进程生命周期内再也起不来**。现在两道：每个动词**之前**一次 `try_wait`（我们自己的子进程，便宜），以及事后 `needs_relaunch(err, alive)` —— `NoSession` 恒重启，`AttachFailed` **只在浏览器已死时**重启。后半句是 D.9.10 换了身衣服：当年是第二次 `open` 丢掉全部 tab，现在是第二个 Chromium 写同一个 `DevToolsActivePort`。
  - **⑬ 退出时杀，与 `bash {background}` 同形。** `std::process::Child` 不在 drop 时 kill，而 `attach --cdp` 之下 playwright-cli 从来不是浏览器的父进程——所以没有显式停机钩子的话，每次重启都留一个浏览器。`shutdown_browsers_global()` 挂在 `start/mod.rs` 有序停机段落里 `kill_all_running_background()` 旁边，那一行的注释逐字论证过为什么自动机制不够、为什么必须显式、为什么放在这个位置（两条信号路径都经过它，致命错误退出也经过它）。
  - **⑭ 三个超时必须严格嵌套，否则最里面那条臂恒假。** doctor 的 `browser/chromium-missing` 跑的是启动路径同一个解析器，而 `DiagnosticEngine` 在 `DEFAULT_CHECK_TIMEOUT = 20s`（`check.rs:27`）就**放弃这个检查并发它自己的 `Warning`**。初稿把检查内部预算设成 25 s，于是它自己那条「could not verify」永远跑不到，而任何慢探针都把 `aleph-server doctor` 变成琥珀色——正是 `diagnostics/checks/mod.rs:6-10` 点名的「退出码变成常数」。现在 6 s（`--dry-run`）< 8 s（检查自己）< 20 s（引擎），并且这个嵌套关系由一条测试断言，不是由注释描述。
  - **⑮ [装置] `qa/browser_managed/run.sh attach`，以及一条必须搬家的预言机。** 新阶段证明 §6.4 的 `launch` 一句。同时 `open`/`ambient`/`headed` 三个场景原来的预言机——`playwright-cli list` 打印出我们的 `user-data-dir`——在 attach 之后不可能再成立（CLI 不拥有那个目录）。搬到 `<udd>/DevToolsActivePort` + `curl /json/version`，**更强**：旧的证明「CLI 转述了我们写给它的配置」，新的证明「浏览器确实是用那个目录被我们起起来的」。
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
not offer one), and no attempt to hide the port — Aleph records it on purpose,
in `~/.aleph/data/browser/chromium/<profile>.json`, because a browser Aleph
cannot find after a crash is a browser Aleph cannot kill. That file is
readable by the same local user who could already drive the port, so it widens
nothing.
```

- [ ] **qa/README.md**：`:19-27` 的清单里，在 `open` 那一行**之前**插入
  ```
  ./qa/browser_managed/run.sh attach   # Aleph starts Chrome; playwright-cli joins over CDP (unix only: pgrep)
  ```
  并在 `:1005` 的「`browser_managed` — 改 `src/browser/` 或 `src/builtin_tools/browser_tools/` 前跑」那一行后面补：「**改启动链（`chromium_launch` / `chromium_resolve` / `playwright_launch` / `playwright_cli`）必须跑 `attach`**——它是唯一证明「Aleph 起的浏览器」而不是「某个浏览器」的阶段。它用 `pgrep -f`，所以和这个目录里其它场景一样**只在 unix 上跑得动**；Windows 上它不是坏了，是没覆盖。」
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

按要求做了四轮自查（覆盖 / 占位符 / 跨任务签名 / 锚点），并在 2026-09-05 的评审之后重做了一遍。

1. **Spec 覆盖对照。** §3.1 → Task 1/4/5（spawn + 端口文件 + `attach --cdp` 永不 `open` + 生命周期归 Aleph + 惰性 re-attach）与 Task 6（收割器杀浏览器、退出时杀）。§3.2 的访问器 → Task 5 的 `PlaywrightCliDriver::endpoint` + Task 6 的 `ProfileManager::live_endpoint`（**只有访问器，没有视图**）。§3.6 「退出时杀 / 崩溃后按 udd 收孤儿 / boot 清上次残留」→ Task 6 的 `shutdown_browsers_global` + Task 1 的注册表清扫 + Task 9 的两条真机 claim。§6.1 → Task 2（三个配置键）+ Task 3（解析顺序、fail-closed 文案、**明写不在首次使用时安装**、明写不取 headless-shell）+ Task 7（`download_host` 透传 + doctor 哨兵）+ Task 8（R8 工具 + 授权）。§6.2 的四行 → 端口超时 = `LaunchFailed{stage:"devtools-port"}`；Chrome 中途死 = Task 5 的**两道**（动词前 `try_wait` + 事后 `needs_relaunch`）；CLI 崩/被收割 = `AttachFailed` → 重启一次 → 再 attach；Chromium 未装 = `ChromiumUnavailable` + doctor + 工具。§6.3 → Task 2（`[browser.live]` 三键属于 Plan 2）。§6.4 的 `launch` 一句 → Task 9。§6.5 第 1 步的既有场景回归 → Task 9 最后一步（**九个**，不是八个，白名单数过）。
2. **占位符扫描。** 全文无 `TBD` / 「类似 Task N」/「加上错误处理」。仅有的尖括号占位在 Task 10 的「验证」行与两条 ceiling 数值，都**必须由实测填入**，并各自带一句「不许抄，要测」的说明——这是判据 §18 要的形状，不是占位符。Task 9 的 claim 条数也刻意不写死。
3. **跨任务签名一致性（评审前改过三处，评审后又改三处）。** 评审前：`PlaywrightCliDriver::new` 加 runtime 参数；子进程映射从 manager 挪到 driver；`resolve_binary` 带上来源。评审后：① `resolve_binary` 从 `(PathBuf, ChromiumSource)` 变成 `ResolvedChromium{path, source, engine}`，三个消费者（Task 5/7/8）同批更新；② `ChromiumChild::spawn` 多收一个 `session_key`（sidecar 进注册表，不再由 udd 定位）；③ `reap_orphans` 的注入效果先变成三个（argv / present / kill）、再随 addendum 收回两个（`ArgvProbe` 三态自带 present），`reap_orphans_now` 不再取参数；`argv_names_dir` 的入参从 `&str` 变成 `&[String]`。
4. **锚点复核。** 评审逐条核过 24 个锚点：20 个精确、4 个差一两行、2 个真错。本轮全部修正 —— `post_install.rs` 的 `run` 分派改 `:68-78`；`browser_flag_value` 改 `:106-123`（`:110-121` 会留下四行悬空）；`managed_profiles_name_the_fields_their_driver_drops` 改 `:830-875`（`:838` 在函数体中段）；Task 9 的 `--chromium-udd-root` 整个删掉（它从未被定义过）。新引入的锚点（`method_authz.rs:31`、`check.rs:27` / `:205-225`、`manager.rs:631-643` / `:585-587`、`start/mod.rs:3642` / `:3658`、`discovery.rs:85-98`、`playwright_launch.rs:458-475`、`bash_exec.rs:476`）全部在本次会话里带行号读过。
5. **评审 addendum（5b）落实。** 孤儿清扫的**读者**换了：从 `fact_for_pid(pid).cmdline`（space-joined 字符串，只支持子串扫描，且是 pty 侧类型的另一种契约）换成直接读 `sysinfo::Process::cmd()` 的 argv 向量，比对改为整 token 相等并同时认 `--user-data-dir=<path>` 与两 token 写法；`Option` 换成三态 `ArgvProbe{Absent, Unreadable, Argv}`，`present` 闭包随之取消（三态自带那个答案）。测试注入了 addendum 点名的五组向量：带 env 渗漏的真匹配、`default` vs `default-2` 的前缀兄弟、`Unreadable` 保留记录、`Absent` 删记录、被回收的 pid 不杀不留；外加 flag 整串出现在某个 env 值里的那一条。证伪清单里加了「改回子串扫描 → 两条测试同时红」。⚠️ 读者建在 `utils::process_alive::with_process_specifics` 之上，而不是 addendum 字面写的 `System::new_with_specifics(...)`——后者会在 `chromium_launch.rs` 造出本仓第二份 `sysinfo` 惯用法，而 `process_alive.rs:118-127` 的 doc 明写它是唯一所有者、第二份会在「刷新哪些字段」上漂移；那个 helper 的 `Option` 返回恰好就是 `Absent` 与「在表里」的分界，所以三态照样拿得到，而且刷新只作用于一个 pid 而不是全表。
6. **本轮修掉的判据错误（自查 + 评审各一半）。** ① `ChromiumChild::alive` 把 `try_wait` 的 `Err` 读成「死了」——把「我不知道」当值花掉（§8），已改成读成「活着」，由随后的 attach 结算。② doctor 读不到配置时回落 `Default::default()`——已改成 `unknown_finding`，**并且同一笔把 `runtime_manage` 的 `chromium_row` 也改了**（评审指出孪生没跟上，§16）。③ `reap_orphans` 把「argv 读不出」当「进程没了」删记录——不可逆，§8×§15，已三态分开。④ attach 锚点从 `||` 改成 `&&`：`classify_failure` 跑在可能含页面文本的输出上，`ECONNREFUSED` 是任何程序都会写的句子。⑤ `unhonored_managed_fields` 的「恒空」论断是错的，已改写并把它保护的事真正接管。⑥ 三个超时不嵌套导致 doctor 的超时臂恒假，已改成 6 < 8 < 20 并**用测试断言这个嵌套**。⑦ Task 8 的 `list` 测试会在 `cargo test --lib` 里 spawn node，违反 `playwright_cli.rs:150-165` 那条封印纪律，已改为注入 locator。

---

## Review disposition

评审给出 46 条（HIGH 12 / MEDIUM 19 / LOW 15）。12 条 HIGH 与两条 STRUCTURAL 裁定全部落实（见上）。以下是**被驳回**的 MEDIUM/LOW，每条一句理由：

- **MEDIUM「`resolve_binary` 应在首次使用时尝试安装（spec §6.1）」——不做，但已明写。** 150 MB 下载落在第一次浏览器工具调用上，而那条路径的硬预算是 180 s（`WAIT_MAX_TIMEOUT_SECS=170`，CLAUDE.md 明写不许扩展）。Task 3 的函数 doc 与本计划都写下了这个取舍与它的代价（干净 Linux 上第一次调用会失败一次）。
- **MEDIUM「Linux 无 root 时接 `chromium-headless-shell` 降级」——本轮不做，改为明确不取它。** 半接的代价比不接大：`headless=false` 的 profile 解析到它就得到一个永远开不出窗口的浏览器。Task 3 把它从候选表里拿掉并配了一条「即使它是目录里唯一的二进制也不取」的测试。
- **MEDIUM「doctor 与 `runtime_manage{list}` 的解析结果应当 memoize」——不做。** 缓存要有失效条件（配置换了、浏览器被删了），而这一轮没有任何一处能说出那个条件；一个不会失效的缓存会把「Chromium 装好了」这件事按住不放。两个面的成本各是一次 6 s 上限的 `--dry-run` 加一次目录走，且都只在没有 pin、没有系统浏览器时才走到。留给 Plan 2 与视图一起决定。
- **MEDIUM「`shutdown` 应先 SIGTERM 再 SIGKILL，或走 CDP `Browser.close`」——不做，但后果已记录。** 优雅关闭要引入一个宽限计时器与一条 CDP 调用，而本计划刻意不引入 CDP 客户端（那是 Plan 2 的 `src/browser/live/cdp.rs`）。代价（singleton 锁残留、恢复提示）写进了 FEATURE_LOCATOR ⑨。
- **LOW「两个 profile 配同一个 `user_data_dir` 会在端口文件上撞车」——只记录，不修。** 这是既有形状（旧的 `--config userDataDir` 同样共享），修它要给 profile 之间加一条互斥规则，超出本轮范围。sidecar 已按 profile 键控，所以清扫不会互相误伤。
- **LOW「`ChromiumSource` 应参与 `--headless=new` 还是 `--headless` 的选择」——不做。** 取掉 headless-shell 之后候选只剩 Chrome / Chromium / Edge / Brave，`--headless=new` 对 112 以上全部正确；一个只有一种取值的开关不该先有分支。
