# Aleph browser subsystem — current shape (map for a dual-engine design)

- Repo scanned: `/Volumes/TBU4/Workspace/Aleph` (as instructed). **Caveat:** global memory records that the canonical session checkout / git root is `/Volumes/TBU/Workspace/Aleph` and TBU4 is a second checkout. Everything below is read at TBU4 `HEAD = c049ef1ed` ("Merge remote-tracking branch 'origin/main'"), whose parent is `5a4a15bf1 census+chromium_resolve: close M3's residual risk`, i.e. the tip of plan 1's Task-9 work. If TBU has moved past that, re-verify.
- Read-only pass. No file modified.
- Orientation: `graphify query` run first (three queries: "browser backend trait and its implementations", "how browser tools reach the browser process", "snapshot refs element ref_id playwright aria snapshot"). The graph's browser coverage is thin — it surfaced only `src/browser/{playwright_cli,chromium_launch,playwright_launch,playwright_cli_backend}.rs` as nodes and otherwise returned the *desktop* `WindowBackend` (a different subsystem). All claims below are from reading the files.

---

## 1. The abstraction boundary today

**One trait, one file: `src/browser/backend.rs:14` — `pub trait BrowserBackend: Send + Sync`** (`#[async_trait]`). 240 lines, of which ~120 are doc comments recording rulings. There is no second trait, no engine/driver split, no capability enum. Header line 1 calls it a "**text-first unified contract**", and that is the accurate description.

### Every method

| Method | Line | Signature (returns) | Shape |
|---|---|---|---|
| `open_tab` | `backend.rs:15` | `(&self, url) -> TabId` (= `String`, `types.rs:5`) | id-as-string |
| `close_tab` | `:16` | `(tab_id) -> ()` | unit |
| `list_tabs` | `:17` | `-> String` | **text (driver stdout / MCP text)** |
| `navigate` | `:18` | `(tab_id, url) -> ()` | unit |
| `click` | `:19` | `(tab_id, ActionTarget) -> ()` | unit |
| `type_text` | `:20` | `(tab_id, ActionTarget, text) -> ()` | unit |
| `fill` | `:26` | `(tab_id, ActionTarget, value) -> ()` | unit |
| `hover` | `:32` | `(tab_id, ActionTarget) -> ()` | unit |
| `scroll` | `:33` | `(tab_id, ActionTarget, ScrollDirection) -> ()` | unit |
| `screenshot` | `:39` | `(tab_id, ScreenshotOpts) -> ScreenshotOutput` | **structured** (`{png_bytes: Vec<u8>}`, `types.rs:121`) |
| `snapshot` | `:44` | `(tab_id) -> SnapshotOutput` | **struct wrapping text** (`snapshot_text: String` + `page_url` + `page_title`, `types.rs:106`) |
| `evaluate` | `:59` | `(tab_id, js) -> String` | **text** (must be *the value*, see below) |
| `select` | `:60` | `(tab_id, ActionTarget, value) -> ()` | unit |
| `press_key` | `:70` | `(tab_id, key) -> ()` | unit |
| `history` | `:76` | `(tab_id, HistoryNav) -> ()` | unit |
| `dblclick` | `:82` | `(tab_id, ActionTarget) -> ()` | unit |
| `wait_for` | `:92` | `(tab_id, &WaitCondition, timeout_ms) -> bool` | bool; **has a real shared default** delegating to `wait_probe::poll_wait_for` (`backend.rs:98`) |
| `console_messages` | `:104` | `(tab_id) -> String` | **text** |
| `network_log` | `:109` | `(tab_id) -> String` | **text** |
| `pdf` | `:118` | `(tab_id, &Path) -> ()` | unit; **one-sided default** → `unsupported_in_existing_session("pdf")` |
| `switch_tab` | `:131` | `(tab_id) -> ()` | unit |
| `handle_dialog` | `:138` | `(tab_id, action: &str, prompt_text: Option<&str>) -> ()` | unit; action is a **stringly-typed** "accept"/"dismiss" |
| `drag` | `:149` | `(tab_id, from: ActionTarget, to: ActionTarget) -> ()` | unit |
| `upload` | `:162` | `(tab_id, Option<ActionTarget>, paths: &[String]) -> ()` | unit |
| `resize` | `:172` | `(tab_id, w: u32, h: u32) -> ()` | unit |
| `emulate` | `:180` | `(tab_id, &EmulateOptions) -> ()` | unit |
| `save_state` | `:189` | `(&Path) -> ()` | **one-sided default** (note: no `tab_id`) |
| `load_state` | `:197` | `(&Path) -> ()` | **one-sided default** |
| `cookies` | `:205` | `(&CookieOp) -> String` | **text**; one-sided default |
| `fill_form` | `:214` | `(tab_id, &[(ActionTarget, String)]) -> usize` | count; **real shared default** = a loop over `fill` |

**28 required methods + 5 with defaults (2 real shared defaults, 3+1 one-sided stubs).** The one-sided defaults all funnel through `unsupported_in_existing_session()` (`backend.rs:235`), whose message *names the Chrome DevTools MCP backend and tells the reader to switch profile*. That is a deliberate 判据-§14 ruling recorded at `backend.rs:228-234`: "every default left in this trait that is not a shared implementation is served by exactly one backend."

### Text-shaped vs structured — the honest tally

**Text-shaped (returns driver stdout or MCP text verbatim):** `list_tabs`, `evaluate`, `console_messages`, `network_log`, `cookies`, and the `snapshot_text` field of `snapshot`. Five of the six are `-> String` with **no parsed structure at all**; the caller re-parses.

**Structured:** only `screenshot` (`ScreenshotOutput{png_bytes}`), `wait_for` (`bool`), `fill_form` (`usize`), `open_tab` (`TabId`), and the two metadata fields of `SnapshotOutput`.

**Unit-returning (23 methods):** these are "the action succeeded" — success/failure is the only information crossing the boundary. There is no post-action state, no changed-DOM delta, no navigation result. The tool layer therefore has to call `snapshot` again if it wants to know what happened.

### Two load-bearing contract rulings living in this trait's doc comments

1. **`evaluate` must return the *value*, not the transcript** (`backend.rs:45-58`). The reason is a real silent failure: `wait_probe::poll_wait_for` searches `evaluate`'s return string for a sentinel that is a literal inside every probe it builds, so a backend that echoes the script back makes the search true on the first poll and **every `wait_for` on that driver silently reports "found"**. The managed Playwright driver did exactly that for its whole life (`playwright-cli eval` prints `### Ran Playwright code` under the result). Fixed by `playwright_cli::parse_result_value` (called at `playwright_cli_backend.rs:417`) and `parse_evaluate_value` (called at `chrome_mcp_backend.rs:352`). **A new engine that returns a transcript re-opens this exact hole, and nothing in the type system stops it** — the signature is `-> String`.
2. **`switch_tab`'s selection only survives if the next "which tab is active" question honours the driver's own `[selected]` marker** (`backend.rs:126-130`), which routes to `tab_registry::active_tab_id` as the single source.

### Implementors (three, all in-tree)

| Impl | File:line | Talks to |
|---|---|---|
| `PlaywrightCliBackend` | `playwright_cli_backend.rs:19` (struct), `:162` (`impl BrowserBackend`) | shells out to the `playwright-cli` binary via `Arc<PlaywrightCliDriver>` |
| `ChromeMcpBackend` | `chrome_mcp_backend.rs:19` (struct), `:117` (`impl`) | JSON-RPC to the `chrome-devtools-mcp` server via `Arc<ChromeMcpDriver>` |
| `FakeBackend` | `testkit.rs:179` (`impl`), `#[cfg(test)]` only (`mod.rs:17-18`) | nothing — records calls |

`PlaywrightCliBackend` carries `driver + session_key + ssrf_guard + launch: SessionLaunch` (`playwright_cli_backend.rs:19-26`). Every method is `self.run(&["<subcommand>", args...], timeout)` — i.e. **argv construction against a specific CLI's verb vocabulary**: `tab-new`, `tab-close`, `tab-list`, `goto`, `click`, `select`, `eval`, `snapshot`, `cookie-set`, `network-state-set`, … A deliberate split exists between `run` (`:67`, `LaunchPolicy::Refuse` — "27 of the 28 subcommands act on a page that must already exist, and letting any of them launch would make observers create the browser they were checking on") and `run_launching` (`:75`, the only path allowed to open a browser).

`ChromeMcpBackend` carries `driver + profile_name + ssrf_guard` (`chrome_mcp_backend.rs:19-23`) and maps each method to an MCP tool call: `click`/`fill`/`hover`/`take_snapshot`/`evaluate_script`/`select_page`/`upload_file`/`drag`. Its distinguishing machinery is `select_and_call` (`:73`) — take the per-profile mutex, `select_page(tab_id)`, then act — because the MCP server has one implicit "current page" and the select→act pair must not interleave (`:69-72`).

---

## 2. How tools reach a backend

### Resolution chain (there is no lease)

`ProfileManager` (`manager.rs:91`) holds `profiles: RwLock<HashMap<String, ManagedProfile>>`, one `ArcSwap<BrowserSsrfGuard>`, **exactly two long-lived driver handles** — `chrome_mcp_driver: Arc<ChromeMcpDriver>` and `playwright_cli_driver: Arc<PlaywrightCliDriver>` (`manager.rs:96-97`) — plus a `TabRegistry`.

**`get_backend` (`manager.rs:383-406`) is the whole routing layer, and it is a two-arm match on an enum with two variants:**

```rust
match cfg.driver {
    BrowserDriver::Managed         => Arc::new(PlaywrightCliBackend::new(playwright_cli_driver.clone(), profile_name, ssrf_guard.load_full(), SessionLaunch::from_profile(&cfg, headless))),
    BrowserDriver::ExistingSession => Arc::new(ChromeMcpBackend::new(chrome_mcp_driver.clone(), profile_name, ssrf_guard.load_full())),
}
```

`BrowserDriver` is `profile.rs:24-30`: `Managed` (default) | `ExistingSession`. **This enum is the engine-selection point today, and it selects a *driver protocol*, not an engine** — both arms end at Chromium.

Two structural facts that matter for a third engine:
- **Backends are constructed per call, never cached** (`manager.rs:196-199` documents why: `browser.update` hot-swaps the SSRF policy, and a boot-time snapshot meant "the RPC reported success while the running guard never changed"). So adding an arm costs nothing in lifetime management — the engine handle would go beside the two `Arc<…Driver>` fields.
- **Profiles are auto-injected**: `"default"` → `Managed` and `"user"` → `{Chrome, ExistingSession}` are inserted if absent (`manager.rs:143-167`). A new engine would need the same treatment or nobody reaches it.

**There is no control lease anywhere.** `rg` for `lease|Lease|control_|takeover|handover` across `src/browser/` and `src/builtin_tools/browser_tools/` returns nothing. The concurrency primitives that exist are: the per-profile `tokio::Mutex` in `ChromeMcpBackend::profile_guard` (`chrome_mcp_backend.rs:62-71`, held across `select_page`→act to close the interleave race) and `TabRegistry`'s `Mutex<HashMap<..>>`. The human-takeover lease from the live-view spec **does not exist yet**.

### One tool end to end: `browser_click`

1. **`click.rs:112` `AlephTool::call(args: BrowserClickArgs)`**. Args are `{profile: String (default "default", via `mod.rs::default_profile`), ref_id: Option<String>, x/y: Option<f64>, double: bool}` (`click.rs:18-31`).
2. **`resolve_target` (`click.rs:71-101`)** lowers `ref_id` → `ActionTarget::Ref{ref_id}` or `x/y` → `ActionTarget::Coordinates`. Runs **before** the approval gate on purpose ("a malformed call is a model mistake and must not consume a user approval or touch the page", `click.rs:113-114`). It also hard-rejects `double + coordinates` here, because neither driver has a coordinate double-click (`click.rs:77-92`).
3. **`super::check_browser_approval(policy, ActionType::BrowserClick, "click", &format!("{target:?}"))`** (`click.rs:135`, impl at `mod.rs:54-87`) — `Allow`/`Deny`/`Ask`, with a separate non-secret-bearing `display_target` (`mod.rs:89-95`).
4. **`super::make_backend_and_tab(&self.manager, &args.profile)`** (`click.rs:137`, impl `mod.rs:225-234`):
   - `make_backend` (`mod.rs:216-222`) → `manager.record_activity(profile)` then `manager.get_backend(profile)`.
   - `get_active_tab` (`mod.rs:204-208`) → **`backend.list_tabs()` (a network/process round trip returning a raw text blob) → `tab_registry::active_tab_id(&tabs_text)`**. So *every* interaction tool pays one `list_tabs` round trip and one text-parse before it acts.
   - `manager.touch_tab(profile, &tab_id)` resets the per-tab idle timer.
5. **`backend.click(&tab_id, target)`** (or `dblclick` if `double`).
   - Managed: `playwright_cli_backend.rs:253` → `self.run(&["click", &ref_id], action_timeout)` → `PlaywrightCliDriver::run(session_key, LaunchPolicy::Refuse, argv, timeout)` → a spawned `playwright-cli` process.
   - ExistingSession: `chrome_mcp_backend.rs:180-186` → `select_and_call(tab_id, "click", json!({"uid": element}))` → MCP JSON-RPC to `chrome-devtools-mcp`.
6. **Result is folded to `BrowserClickOutput{success: bool, message: Option<String>}`** (`click.rs:35-38`). Errors never escape as `Err` — they become `success:false` with `backend_error_text(&manager, &e)` (`click.rs:159`, impl `mod.rs:148-157`), which truncates to `MAX_BACKEND_ERROR_CHARS`, runs `manager.redact_content` (secret scrub) and `sanitize_external_text`.

Two variants of step 4 exist and the difference is a security ruling: **`make_backend_and_tab_guarded` (`mod.rs:249-262`)** additionally re-checks the *current* page URL against SSRF policy before any content read, because navigation-time guards only vet the URL navigated *to* and a redirect / JS `location` / history move never re-passes them (openclaw #78526 / GHSA-2x93-h3hg-2xfp). Content reads use the guarded one; **interaction and navigation tools deliberately use the unguarded one so the agent can always navigate away from a blocked page** (`mod.rs:247-248`).

### The element-ref contract — and why it is the hardest thing to port

`ActionTarget` (`types.rs:10-15`) has exactly two variants: `Ref{ref_id: String}` and `Coordinates{x,y}`. There is deliberately **no `Selector` variant** — `types.rs:404` has a test (`test_action_target_no_selector_variant`) pinning that a `{"type":"selector"}` payload fails to deserialize.

**Refs are produced nowhere in Rust.** They are produced *inside the driver* and reach Aleph only as opaque substrings of `SnapshotOutput::snapshot_text`:
- Managed: `playwright-cli snapshot` emits Playwright's aria-snapshot YAML with `[ref=e42]` annotations. `playwright_cli_backend.rs:385-400` reads it (from a file when `page_meta.snapshot_file` is set, else stdout) and stuffs it into `snapshot_text` **unparsed**.
- ExistingSession: `take_snapshot` returns chrome-devtools-mcp's indented tree with `uid` values like `"1_6"`. `chrome_mcp_backend.rs:326-338` calls `extract_text` and stuffs it into `snapshot_text` **unparsed**.

**Refs are consumed by being handed straight back down.** `chrome_mcp_backend.rs:37-46` `extract_element_ref` just clones the string into `json!({"uid": ...})`; `playwright_cli_backend.rs` passes it as a bare argv word (`["click", &ref_id]`). Rust never validates a ref, never maps it, never knows its lifetime.

So the actual ref contract is: **the model reads a ref out of a text blob one driver printed, and Aleph relays that string verbatim to the same driver.** The two existing drivers already disagree on the *format* (`e42` vs `1_6`) and Aleph gets away with it precisely because it never looks inside. Consequences for a third engine:

- A CDP-native engine backend **would not break the type**, because `ActionTarget::Ref{String}` is format-agnostic. It only has to (a) mint refs in its own snapshot output and (b) resolve them.
- What *does* break: **refs are per-snapshot and their invalidation rule is not modelled anywhere**. There is no snapshot generation counter, no "this ref is stale" error kind, no re-snapshot-on-stale. `browser_exec` had to state the rule in prose instead (see §9). A new engine inherits an un-enforced invariant.
- Coordinate targeting is **already one-sided**: `ChromeMcpBackend` rejects `Coordinates` outright (`chrome_mcp_backend.rs:40-45`). So the tool faces already tolerate a backend that supports only one targeting mode. That is the useful precedent: an obscura backend that supports refs-and-coordinates, or refs-only, fits without a tool-face change.

### `list_tabs` is the other structurally text-shaped seam

`tab_registry::parse_tab_line` (`tab_registry.rs:201-246`) is a **hand-written parser for two different drivers' human-readable output**, and its doc comment (`:180-199`) records that the previous description named a format no driver emits, so "every real playwright listing parsed to nothing". It currently understands:
- chrome-devtools-mcp `list_pages`: `"1: about:blank [selected]"` (trailing annotation, also accepts `[active]`)
- `playwright-cli tab-list` 0.1.8: `"- 1: (current) [Title](https://x/)"` (leading `(current)`, markdown link)
- a tolerated-but-unemitted `"Tab N: URL"`

`active_tab` (`tab_registry.rs:290-303`) is declared **"the single source for that question"** — prefer the driver's own marker, fall back to last-listed. Its doc carries a **measured known gap** (`:273-288`): under `attach --cdp`, after a close/re-attach, the CLI's `[selected]` marker names the *wrong* tab, because its idea of "current" comes from CDP target enumeration whose order differed between first attach and re-attach in the same run. Picking last-listed instead was tried and also picked wrong. The recorded fix direction is a persistent record one level up in `ProfileManager::tab_registry` — **not built**.

**A third engine has to emit a listing this parser recognises, or the parser gains a third format.** Every interaction tool's tab resolution runs through it.

---

## 10. Sizes and tool count

`src/browser/` — **20 files, 13,344 lines total**:

| Lines | File |
|---|---|
| 1728 | `playwright_cli.rs` |
| 1586 | `chromium_launch.rs` |
| 1413 | `manager.rs` |
| 1035 | `chromium_resolve.rs` |
| 1006 | `chrome_mcp.rs` |
| 970 | `chrome_mcp_backend.rs` |
| 779 | `playwright_cli_backend.rs` |
| 768 | `network_policy.rs` |
| 596 | `profile.rs` |
| 492 | `tab_registry.rs` |
| 475 | `testkit.rs` |
| 472 | `types.rs` |
| 404 | `discovery.rs` |
| 400 | `playwright_launch.rs` |
| 285 | `wait_probe.rs` |
| 275 | `post_nav.rs` |
| 240 | `backend.rs` |
| 225 | `secret_guard.rs` |
| 169 | `error.rs` |
| 26 | `mod.rs` |

**Observation:** the abstraction (`backend.rs`, 240 lines) is 1.8% of the subsystem. `playwright_cli.rs` + `playwright_cli_backend.rs` + `playwright_launch.rs` + `chromium_launch.rs` + `chromium_resolve.rs` + `discovery.rs` = **5,932 lines (44%) that exist only because the engine is Chromium driven by a Node CLI.**

`src/builtin_tools/browser_tools/` — **27 files, 9,838 lines**. Largest: `exec.rs` 1941, `mod.rs` 1105, `tabs.rs` 650, `wait_for.rs` 510, `emulate.rs` 413, `navigate.rs` 408, `fill_form.rs` 382, `click.rs` 366, `session.rs` 351, `evaluate.rs` 336, `cookies.rs` 335, `screenshot.rs` 312, `open.rs` 298, `select.rs` 260, `upload.rs` 247, `snapshot.rs` 237, `dialog.rs` 219, `type_text.rs` 217, `profile_tool.rs` 188, `hover.rs` 185, `pdf.rs` 154, `scroll.rs` 149, `resize.rs` 134, `drag.rs` 132, `press_key.rs` 115, `network.rs` 98, `console.rs` 96.

**Registered `browser_*` tools: 26** — counted at the registry, not the module (`rg -o 'name: "browser_[a-z_]*"' src/executor/builtin_registry/definitions.rs | sort -u | wc -l` = 26; the same 26 names appear as `AlephTool::NAME` constants across `browser_tools/*.rs`). Note the lead's suggested `rg -n '"browser_' src/builtin_tools/browser_tools/mod.rs` returns **zero** matches — `mod.rs` re-exports types, it does not hold the name literals.

Names: `browser_click, browser_console, browser_cookies, browser_dialog, browser_drag, browser_emulate, browser_evaluate, browser_exec, browser_fill_form, browser_hover, browser_navigate, browser_network, browser_open, browser_pdf, browser_press_key, browser_profile, browser_resize, browser_screenshot, browser_scroll, browser_select, browser_session, browser_snapshot, browser_tabs, browser_type, browser_upload, browser_wait_for`.

Construction site: `src/executor/builtin_registry/builder/constructor/mod.rs:603` onward, all fed the one `Arc<ProfileManager>` (`browser_profile_manager`). Struct fields at `src/executor/builtin_registry/registry/struct_def.rs:206+`. Schemas/descriptions at `src/executor/builtin_registry/definitions.rs:576+`.

---

## 3. What is Chromium-bound by name

Plan 1's Task 10 report (`.superpowers/sdd/2026-09-05-browser-live-view-plan1-launch-chain/task-10-report.md`) is **docs-only** — it wrote the inventory into `docs/reference/FEATURE_LOCATOR.md` §3.12 **㉑**, it did not build one. I read §3.12 ㉑ (FL lines 105-108) and re-verified each item against HEAD. **One item in that list is already wrong.**

### §3.12 ㉑ as written, verified against HEAD

**User-facing / wire (changing these breaks config files and callers):**

| ㉑ says | HEAD says | verdict |
|---|---|---|
| config section `[browser.runtime]` with keys `binary_path` / `channel` / `download_host` | The section is nested one level deeper: `general.browser.runtime` (`src/config/types/general.rs:25` — `pub browser: BrowserSystemConfig`). Struct `BrowserRuntimeConfig` (`profile.rs:191`) has **`binary_path` (`:197`), `prefer_system_browser` (`:207`), `download_host` (`:214`)**. There is **no `channel` key** — `rg -n "channel" src/browser/profile.rs` is empty. | ⚠️ **drift.** ㉑ names a key that does not exist and omits one that does. `BrowserError::ChromiumUnavailable` (`error.rs:60-68`) is consistent with HEAD, naming `[general.browser.runtime] binary_path`. |
| doctor check id `browser/chromium-missing` (`chromium_missing.rs:30`) | confirmed: `src/diagnostics/checks/chromium_missing.rs:30` `const ID: &str = "browser/chromium-missing"` | ✅ |
| `runtime_manage`'s `chromium` **argument value** (`runtime_manage.rs:30` `const CHROMIUM`), noting the schema is already engine-agnostic (`capability: Option<String>` free string) so the **value**, not the shape, has to change | confirmed: `src/builtin_tools/runtime_manage.rs:30` `const CHROMIUM: &str = "chromium"`; `:95` `name == CHROMIUM \|\| find_spec(name).is_some()`; `:103` `.chain(std::iter::once(CHROMIUM))` | ✅ |

**The second doctor id ㉑ does not mention:** `src/diagnostics/checks/browser_runtime.rs:34` `const ID: &str = "browser/runtime"` — it asks the *prerequisite* questions (Node, playwright-cli). Both ids live under the `browser/` namespace; only one names Chromium.

**Rust-internal (㉑ calls these "pure renames"):** `BrowserError::ChromiumUnavailable` (`error.rs:68`) · `ChromiumSource` + its label strings (`chromium_resolve.rs:141`, `label()` at `:148`, variants `Pinned`/`System`/`PlaywrightManaged` at `:382`/`:397`/`:425`) · the two files `chromium_launch.rs` / `chromium_resolve.rs` · `ChromiumChild` (`chromium_launch.rs:303`) · `ChromiumLaunchSpec` (`:51`) · `ChromiumSidecar` (`:165`) · `ChromiumInstallCli` (`runtime_manage.rs:375`). All verified present.

**The one ㉑ flags as a signature change, not a rename:** `ChromiumLocator::locate()` (`runtime_manage.rs:112`, impl `RealChromiumLocator` at `:117-122`), **held as a singular field** `RuntimeManageTool.locator: Arc<dyn ChromiumLocator>` (`:128`, constructed `:148`, injectable via `with_locator` `:154`). ㉑ deliberately did not generalise it, saying obscura had to be re-measured at HEAD first. That judgement still holds and this is the one item on the list that is real design work.

### What ㉑ does **not** cover, and a designer will need

These are Chromium/Playwright-shaped and none of them is a rename:

1. **`ChromiumLaunchSpec::argv()` (`chromium_launch.rs:68-106`) is a Chrome switch list**, and the doc states **order is the contract** because "Chrome resolves a duplicated switch to its LAST occurrence". The switches: `--no-first-run`, `--no-default-browser-check`, `--use-mock-keychain`, `--password-store=basic`, `--headless=new`, `--proxy-server=`, `--user-data-dir=`, `--remote-debugging-port=0`, then positional `about:blank`. `--use-mock-keychain` is load-bearing (`:74-89`): without it, on macOS with no usable login Keychain, Chrome answers `/json/version` and looks healthy while **every first navigation per page silently dies**. That is round-7's headline defect. **A second engine has an entirely different switch vocabulary and this function does not generalise.**
2. **`DevToolsActivePort` file protocol.** `const DEVTOOLS_PORT_FILE: &str = "DevToolsActivePort"` (`chromium_launch.rs:44`, "Name fixed by Chrome"), parsed by `parse_devtools_active_port` (`:129`, two lines: port then ws path), turned into `CdpEndpoint{http_url, ws_url, pid}` by `endpoint_from_port_file` (`:143`). `DEVTOOLS_PORT_DEADLINE = 30s` (`:38`), polled every 50 ms (`:41`). **Obscura would have to either write a Chrome-shaped `DevToolsActivePort` file or this discovery mechanism needs a second derivation.**
3. **`ChromiumSidecar` (`chromium_launch.rs:165-180`)** — the JSON record Aleph writes per launched browser, in **one registry directory** (`sidecar_registry_dir()` `:185`, `sidecar_path(session_key)` `:191`), because a profile's `user_data_dir` can be anywhere. Fields: `pid`, `http_url: Option<String>`, `user_data_dir`, plus the build that launched it. The orphan sweep (`reap_orphans` `:658`, `reap_orphans_now` `:839`) matches on `pid` + `user_data_dir` against **argv** (`argv_names_dir` `:257`, `ArgvProbe` `:210`). This machinery is engine-neutral in *shape* but the record has **no engine field**, so a mixed-engine host cannot tell whose orphan it reaped.
4. **`playwright_launch.rs` is 400 lines of playwright-cli JSON config**: `attach_argv` uses `--cdp` (`:169-190`), `config_json` writes `outputDir` + `allowUnrestrictedFileAccess` + `initScript` + `cdpEndpoint` + `userDataDir` (`:200`, `:308`, `:319-333`). `SessionLaunch` (`:35-41`) and `LaunchPolicy` (`:85-91`) live here; `SessionLaunch` is engine-neutral, `LaunchPolicy` is a good abstraction, the config JSON is not.
5. **`discovery.rs`** hardcodes `CHROMIUM_NAMES` (`:19-27`: google-chrome-stable, google-chrome, chromium-browser, chromium, microsoft-edge-stable, microsoft-edge, brave-browser) plus `ALEPH_CHROME_PATH` env override (`:32-45`) and platform paths. `BrowserType` (`profile.rs:13-19`: `Chromium|Chrome|Brave|Edge`) is a **Chromium-family-only enum** and is `#[serde]`-exposed in `[general.browser.profiles.<name>].browser`. **Adding an `Obscura` variant is a wire change to a user-visible enum.**
6. **`chromium_resolve.rs`** — `CHROMIUM_INSTALL_ARGS = ["install-browser", "chromium"]` (`:85`) is a **playwright-cli subcommand pair**, and it is a genuine single source: it is `format!`-interpolated into `BrowserError::ChromiumUnavailable`'s message (`error.rs:66`) and consumed by `runtime_manage.rs:38`. `resolve_binary` (`:354`) implements pinned > system > playwright-managed, `parse_install_location` (`:174`) parses playwright's `--dry-run` stdout, `engine_mismatch` (`:517`) compares `BrowserType`s.
7. **`ChromeMcpConfig` (`profile.rs:254-262`)** defaults to `npx` + `chrome-devtools-mcp` args (`default_chrome_mcp_command` `:264`, `default_chrome_mcp_args` `:268`) including `--allow-unrestricted-paths`. The `ExistingSession` arm is Chrome-only by construction — attaching to a *user's already-running* obscura is a different feature, not this one.
8. **`BrowserError` variants that name a vendor** (`error.rs`): `ChromiumNotFound` (`:29`), `ChromiumUnavailable` (`:68`), `ChromeMcpError` (`:79`), `ChromeMcpTransport` (`:87`), `PlaywrightCliError` (`:90`), `PlaywrightCliNotInstalled` (`:93`). Plus `LaunchFailed{stage}` (`:14`) whose stage strings are enumerated in its doc as `"spawn" | "chromium-exit" | "devtools-port" | "chrome-mcp"` — **an engine-named string in a `&'static str` field**.
9. **`unsupported_in_existing_session` (`backend.rs:235-240`)** hardcodes the sentence "the Chrome DevTools MCP server exposes no {op} primitive — use a managed profile such as 'default'". With three engines this message can be wrong.
10. **Files outside `src/browser/` that say Chromium**: `src/diagnostics/checks/chromium_missing.rs`, `src/diagnostics/checks/browser_runtime.rs`, `src/runtimes/specs.rs`, `src/tools/probes/browser.rs`, `src/builtin_tools/runtime_manage.rs`, `src/builtin_tools/pdf_generate/{browser_engine,args,mod}.rs`, `src/config/dead_keys.rs`, `src/sandbox/platforms/macos/seatbelt.rs`, `src/bin/aleph-server/commands/start/{mod,helpers}.rs`, `src/bin/aleph-server/cli.rs`.

**`pdf_generate` is a second, independent consumer.** `src/builtin_tools/pdf_generate/browser_engine.rs` builds its **own** `PlaywrightCliDriver` (that is why `ProfileManager::playwright_cli_config()` at `manager.rs:360` and `runtime_config()` at `:374` exist as accessors — and `:377` carries an explicit "do not add a second"). A dual-engine design must decide whether the PDF engine follows the default engine or pins Chromium; today it inherits the same CLI + runtime config as the browser tools.

---

## 5. Existing CDP code

**Aleph does not speak raw CDP anywhere.** Verified:

- `rg -n "Runtime.evaluate|Target.attach|DevToolsActivePort|json/version|chromiumoxide|cdp" src --type rust -l` returns nine files, and **none of them opens a CDP connection**. They are: `chromium_launch.rs` (writes `--remote-debugging-port=0`, reads the port file, builds `CdpEndpoint` strings), `playwright_launch.rs` / `playwright_cli.rs` (build `attach --cdp <http-url>` argv and classify its refusals), `tab_registry.rs` (a comment about CDP target-enumeration order), `manager.rs`, `exec.rs` (a comment at `:15` about hermes's raw-CDP model), `gateway/config.rs`, and the two `bin/aleph-server/commands/start/` files (shutdown ordering).
- **The only CDP artefact Aleph owns is a pair of URL strings.** `CdpEndpoint` (`chromium_launch.rs:110-119`): `http_url` = `http://127.0.0.1:<port>` ("the form `playwright-cli attach --cdp` takes"), `ws_url` = `ws://127.0.0.1:<port>/devtools/browser/<id>` — and the field's doc says outright it is "what a raw CDP client (**the live view, Plan 2**) connects to". So the ws URL is **built for a consumer that does not exist yet**.
- `mod.rs:24-26` makes this explicit: `pub(crate) use chromium_launch::CdpEndpoint;` with the comment *"its first real consumer (the live view, Plan 2) lives in this crate."*

**Crates.** `Cargo.toml:282` has `tokio-tungstenite = "0.26"` and that is the only WebSocket stack. **No `chromiumoxide`, no `headless_chrome`, no `fantoccini`, no `rust-cdp`.** Current `tokio-tungstenite` consumers are all messaging/gateway: `gateway/interfaces/{qq,nostr,mattermost,slack}`, `gateway/server/{mod,handler}.rs`, `gateway/origin_policy.rs`, `bin/aleph-server/commands/node.rs`, and — the one browser use — `src/browser/chrome_mcp.rs` (for the MCP transport, not for CDP).

**`src/browser/live/` does not exist.** `ls src/browser/live` → No such file or directory. Nothing from the spec's D-section CDP observer is built. What exists is exactly the handoff: a launched process, a parsed port, and two URL strings.

**Implication for a CDP-native engine backend:** the plumbing to *reach* a CDP endpoint is done and tested; the client is entirely unwritten. A `trait Engine` backend speaking CDP over `tokio-tungstenite` would be the first raw-CDP code in the repo, and it would be the natural consumer of `CdpEndpoint::ws_url`.

---

## 4. How the model sees a page today

### `browser_snapshot` returns one string, and it is the driver's own text

`BrowserSnapshotTool::call` (`snapshot.rs:85-141`), verbatim shape:

1. `resolve_max_chars(args.max_chars)` — clamps the model's request into `[MIN_SNAPSHOT_CHARS = 1_000, MAX_SNAPSHOT_CHARS = 120_000]` (`snapshot.rs:25-35`), defaulting to `DEFAULT_CONTENT_MAX_CHARS = 30_000` (`mod.rs:273`).
2. `make_backend_and_tab_guarded` → `backend.snapshot(&tab_id)`.
3. **`bound_content(&snap.snapshot_text, max_chars)`** (`mod.rs:282`) — cuts back to the last **line boundary** within budget specifically so a `[ref=eN]` token is never split (`mod.rs:278-280`).
4. `let ref_count = text.matches("[ref=").count();` (`snapshot.rs:96`) — **the only place in Rust that looks inside a snapshot**, and it is a literal substring count of Playwright's ref syntax, run on the *emitted* text so the number matches what the model can act on.
5. `redact_wrap` (`mod.rs:340`) — secret redaction plus `wrap_external_content(.., ContentSource::BrowserContent)`, the prompt-injection fence.
6. If truncated, `offload_full_content` (`mod.rs:414`) writes the **full** tree to the tool-result store keyed on `current_tool_call_id()` and appends a recovery footer pointing at `ctx_search`. If offload fails, the message says the tail is **not recoverable** and names `browser_evaluate` as the fallback.

Output: `BrowserSnapshotOutput{success, snapshot: Option<String>, truncated: bool, ref_count: usize, message}` (`snapshot.rs:51-57`). The tool `DESCRIPTION` is "Get an accessibility tree snapshot of the current browser page for structured understanding" (`snapshot.rs:80`).

**What the model actually receives is a single opaque text blob** — Playwright's aria-snapshot YAML (`- button "OK" [ref=e1]`) or chrome-devtools-mcp's indented tree with `uid`s — fenced and redacted. Note that `SnapshotOutput` carries `page_url` and `page_title` (`types.rs:113-116`) and **no caller renders them**; the type's own doc says so: *"the `browser_snapshot` tool returns `snapshot_text` alone, so the model cannot see which page it is looking at."* That is a live, documented gap at HEAD.

**Size:** default 30k chars, model-raisable to 120k, offloaded above that. §3.12 records the round-3 ruling that truncating *inside* the tool was wrong because `tool_output` ingress persists the un-sanitised original for `ctx_search`; the fix was to offload rather than to raise the budget.

### `browser_screenshot` gives pixels, plus two optional text layers

`BrowserScreenshotOutput` (`screenshot.rs:74-97`): `{success, image_base64, format: Option<String>, message, ocr_text: Option<String>, description: Option<String>}`.

- **`format` is load-bearing**, and its doc says why (`screenshot.rs:77-82`): `result_processing::extract_image_in_place` refuses to hoist an inline image without a recognised `format`, so without the field the base64 stayed in the **text** channel and the result budget shredded it — "the model acted on a screen it never saw". That was a CRITICAL in round 3, and round 6 found the *same* defect one layer down (`browser_exec`'s image nested in `results[]`, which `hoist_inline_images` could not reach — fixed by making it walk the whole tree with `MAX_HOIST_DEPTH = 16`).
- `describe: true` adds `ocr_text` (offline OCR, full `redact_and_wrap`) and `description` (vision-model prose, bounded at `MAX_DESCRIPTION_CHARS = 4_000` then `redact_wrap`). The doc's reasoning is worth keeping: a page that paints "ignore previous instructions" gets it relayed verbatim by any model asked to describe what it sees, so prose about the page earns the same fence as the page (`screenshot.rs:40-57`).
- Byte/edge budgets: `MAX_SCREENSHOT_EDGE = 1568` (Anthropic's server-side threshold) and `MAX_SCREENSHOT_BYTES = 5 MiB`, iteratively re-scaled at 0.7/0.5/0.35/0.25, floor 0.25.

### `browser_exec` is the multi-step face

`ExecAction` (`exec.rs:101`) has write steps (`Navigate, Click, Dblclick, Type, Fill, Hover, Scroll, Select, PressKey, Wait, Dialog`) **and read steps** (`Snapshot{max_chars}` `:186`, `Evaluate{js}`, `Screenshot{full_page}`, `Console`, `Network`). Each read step runs `read_guard()` first (`exec.rs:619`) — the same `current_page_block` predicate `make_backend_and_tab_guarded` uses — because the mid-sequence `navigate` is exactly what can park the tab on a forbidden origin. Results come back as `StepResult{step, action, status, output}` (`exec.rs:243`), never echoing typed text or eval source (labels report byte counts only). ≤50 actions, 600 s own wall clock, registered at 630 s in `BUILTIN_TOOL_BUDGETS_MS` so **the tool's own clock fires first**.

### Is there ANY structured DOM / layout / spatial representation anywhere?

**No. Verified negative.** `rg -ni "bounding|boxmodel|getBoundingClientRect|DOM.getDocument|spatial|viewport|coordinates"` over `src/browser/` and `src/builtin_tools/browser_tools/` returns only:

- `ActionTarget::Coordinates{x, y}` — an **input** pair the model supplies, rejected outright by the Chrome MCP backend (`chrome_mcp_backend.rs:40-45`) and by `target_ref` on the Playwright side for ops that need a ref (`playwright_cli_backend.rs:94-95`).
- `browser_resize` width/height in CSS pixels (`resize.rs:18-20`).
- prose in doc comments.

Across the **whole** `src/` tree, `bounding|boxmodel|getBoxModel|DOM.getDocument|spatial` matches nothing browser-related (the hits are `fnm install layout`, `monorepo layout`, `bounding the catalog`, etc.).

**So the model's entire spatial model of a page today is: a flat accessibility-tree text blob with opaque refs, optionally a PNG.** There is no element geometry, no z-order, no scroll position, no visibility, no containment tree. Nothing to diff between snapshots, and no way to answer "is this element on screen" except by asking the driver.

This is the single most consequential finding for the D6 question of **whether to skip rasterisation entirely**: there is no existing structure to extend, so a `DOM + layout → spatial-state JSON` representation would be **net-new, not a migration**, and it has no incumbent consumer to break. The only Rust code that reads inside a snapshot is one `matches("[ref=").count()`.

---

## 6. Process lifetime and runtime provisioning

### Who launches the browser

**Aleph does, since round 7 (`4c208760a`, the launch-chain flip).** `ChromiumChild::spawn` (`chromium_launch.rs:316-405`) is the one production constructor:

1. `create_dir_all(user_data_dir)` then `restrict_udd_to_owner` (`:321-329`).
2. **Delete a stale `DevToolsActivePort`** first (`:331-334`) — "a leftover file from the PREVIOUS launch would be read as this one's endpoint, a port that is either closed or, worse, somebody else's".
3. `Command::new(spec.binary).args(spec.argv())`, all three stdio to null, **secret env stripped** via `security::secret_env::is_secret_env` (`:343-348`, same discipline as the CLI child), `.no_window()`, spawn.
4. **`write_sidecar_record(session_key, pid, user_data_dir, None)` BEFORE the port is known** (`:363`) — an explicit 判据 §15 intent-stamp: `std::process::Child` does not kill on drop, so if the future is dropped at any `await` below or Aleph crashes, this record is the only trace that a Chromium exists and needs reaping. An endpoint-less record is fully reapable because the reaper decides on `pid` + `user_data_dir` alone.
5. Poll loop with three distinct exits: port file parses → `Ok(Self)` and rewrite the sidecar with the endpoint; `child.try_wait()` says exited → `LaunchFailed{stage: "chromium-exit"}`; deadline → kill, wait, `LaunchFailed{stage: "devtools-port"}`. The doc is explicit that "Chrome died before publishing" and "the file is late" are different operator problems.

`ChromiumChild` also has `alive()` (`:451`) which answers **`true` on `Err` from `try_wait`** ("I could not tell" is not "it is dead"), `kill_only()` (`:469`, kills the process but leaves every sidecar alone — for the one caller that replaces a child under the same session key), and `shutdown()` (`:492`, kill + `wait()` **only after a successful kill**, then delete the sidecar).

`playwright-cli` then joins with `attach --cdp <http_url>` (`playwright_launch.rs:169-190`). The module header (`playwright_launch.rs:1-21`) records the three measurements that forbid going back: a CLI-launched Chrome's debug port is not a contract (random per launch, caller's `--remote-debugging-port` loses to Playwright's, no port file written); `close` under `cdpEndpoint` only *disconnects* (nine Chrome processes before and after); and **`open` clobbers the page it reuses** by issuing `goto('about:blank')` while `attach` does not.

### Orphan reaping

`reap_orphans(registry, argv_of, kill)` (`chromium_launch.rs:658`) — both effects injected so the decision is testable without a browser; `reap_orphans_now()` (`:839`) is the production wiring, invoked from `ProfileManager::spawn_idle_reaper` via `sweep_orphaned_chromium` (`manager.rs:334-345`, and note the `#[cfg(not(test))]` / `#[cfg(test)]` twin at `:330`/`:341` — sealed because the task is detached and would outlive a test's `AlephHomeEnvGuard` and kill a developer's real Chromium).

**Four outcomes per parseable record, deliberately not collapsed** (`:637-651`):

| `ArgvProbe` | action |
|---|---|
| `Argv` naming our dir | ours: kill, drop the record |
| `Argv` naming something else | pid recycled: kill nothing, drop the record (determinate) |
| `Absent` | process gone: nothing to kill, drop the stale record |
| `Unreadable` | **learned nothing**: kill nothing, **keep the record** (判据 §8 × §15; routine on Windows where `sysinfo` often cannot read another process's command line) |

Plus a fifth for records that never parsed: renamed aside to `.corrupt`, counted in `ReapOutcome{reaped, corrupt_pending, corrupt_superseded}` (`:608-624`). `corrupt_pending` fires a warning on **every** boot sweep while nonzero, not only the sweep that first quarantined it — M6's fix, because "the rename was always correct; the silence was the defect".

`kill` returning `false` (still alive, refused to die) must **not** be spent as a reap (`:653-657`, 判据 §4).

### The two daemon exit sites

Both call `browser::manager::shutdown_browsers_global()` (`manager.rs:79`, which upgrades a `Weak<ProfileManager>` published by `spawn_idle_reaper`):

1. **Orderly**: `src/bin/aleph-server/commands/start/mod.rs:3672`, after the background-bash reap, before the projection barrier. Reached by both signal paths and by a fatal `run_until_shutdown` error.
2. **Wedged failsafe**: `src/bin/aleph-server/commands/start/helpers.rs:536`, inside the `SHUTDOWN_FAILSAFE` task that ends in `std::process::exit(0)`. Placed **before** the 2 s sleep, and the comment says why: this block has already spent the whole 5 s failsafe matched to `aleph-server stop`'s SIGTERM→SIGKILL window, so every line past this point may never execute; the browser stop is synchronous so putting it first costs the bash reap nothing.

There is a source-level guard for this pair at `helpers.rs:646` / `:672` (it looks for `shutdown_browsers_global(` and iterates `["kill_all_running_background", "shutdown_browsers_global"]`).

### Runtime provisioning — is it generic or Chromium-specific?

**It is generic for *runtimes*, and Chromium is explicitly carved out of it.**

The ledger is a static table: `pub const SPECS: &[RuntimeSpec]` (`src/runtimes/specs.rs:79`) with six entries — `fnm`, `node`, `uv`, `playwright-cli`, `cargo`, `git`. `RuntimeSpec` (`specs.rs:5-21`) carries `{name, binaries, version_flag, version_regex, min_version, deps, install: &[OsInstall], post_install: &[PostInstallAction], llm_hint, install_hint}`. `InstallStrategy` (`specs.rs:28-44`) is `Shell | PowerShell | Via{parent, subcommand} | NpmGlobal{package}`.

**Chromium is NOT a `RuntimeSpec`, and `runtime_manage.rs:10-16` states why:** the ledger probes PATH (`runtimes::probe::probe_system_path`) and Playwright's Chromium lives in a per-revision cache directory that is never on PATH — "a spec for it would sit at `Missing` forever and reinstall on every call". Instead Chromium rides in as `playwright-cli`'s `post_install` action: `PostInstallAction::RunSubcommand{args: ["install-browser", "chromium"], target_dir: None, env: [EnvFromConfig::PlaywrightDownloadHost]}` (`specs.rs:191-195`).

`runtime_manage` (the R8 tool face) therefore special-cases it: `is_installable(name) = name == CHROMIUM || find_spec(name).is_some()` (`runtime_manage.rs:94`), `installable_names()` chains `CHROMIUM` onto `SPECS` (`:99-105`), and it learns Chromium's *status* through the injected `ChromiumLocator` trait rather than the ledger (`:112`).

**What a second engine binary (obscura: single static binary, downloaded from GitHub releases) would need:**

- **The good news.** `RuntimeSpec` + `InstallStrategy` is a real, generic provisioning abstraction with a `Shell`/`PowerShell` strategy that could fetch a release asset, an `AssetProbe` post-install action (`specs.rs:71-74`, `{path, repair}`) for verifying a downloaded file, and `EnvFromConfig` for config-supplied env (currently one variant, `PlaywrightDownloadHost`, and `specs.rs:47-55` explains it exists so a fact is not honoured on one path and dropped on the other). A single static binary on PATH is exactly the shape the ledger's PATH probe handles well — *better* than Chromium fits.
- **The bad news, in order of cost.**
  1. `ChromiumLocator::locate() -> RuntimeRow` is **singular and held as a field** (`runtime_manage.rs:112`, `:128`). §3.12 ㉑ flags this correctly: a second engine needs a **signature change**, not a rename and not a match arm.
  2. `const CHROMIUM: &str = "chromium"` is a **wire value** in the `capability` argument. The schema is already `Option<String>` free-form, so the shape is fine and the value is the debt.
  3. `chromium_launch.rs` assumes the **Chrome DevTools port-file protocol** and the **Chrome switch vocabulary**. Neither generalises.
  4. `ChromiumSidecar` has **no engine field**, so a mixed-engine host cannot attribute an orphan.
  5. `discovery.rs`'s `CHROMIUM_NAMES` + `BrowserType` enum are Chromium-family-only and `BrowserType` is user-visible config.
  6. The whole install path routes through `playwright-cli install-browser`. A GitHub-releases download shares **none** of it — it would use `InstallStrategy::Shell`/`PowerShell` + `AssetProbe` instead, which is a new spec entry, not a modification.
- **Neutral and reusable as-is:** the sidecar registry directory + orphan sweep (once given an engine field), `LaunchPolicy` (`playwright_launch.rs:85-91`), `shutdown_browsers_global` and its two exit sites, and the `SessionLaunch` struct.

---

## 7. `browser_exec`, `network_policy`, `secret_guard` — engine-agnostic or not?

**`browser_exec` (`exec.rs`, 1941 lines) is fully engine-agnostic.** It holds only `Arc<ProfileManager>` + an approval policy and never names a driver. Its module header (`exec.rs:1-31`) states the design explicitly: it is "a scheduler, never a second, unguarded path into the browser", and every step **re-enters the same chokepoint the standalone tool uses** — `check_navigation` before a navigation and the backend's landed-URL audit after it, `check_input_secret_block` before any keystroke, `current_page_block` before any page read, `redact_wrap` on the way out, the per-`ActionType` approval gate. It reaches the engine only through `Arc<dyn BrowserBackend>`. The one thing it assumes about page representation is the ref contract — `ExecAction`'s targeting variants all carry `ref_id` (`exec.rs:439-469`) and the abort message tells the model refs may be stale. It also sits in `BUILTIN_TOOL_BUDGETS_MS` at **630_000 ms** (`src/tools/budget.rs:133`), pinned by `browser_exec_budget_outlives_its_own_wall_clock` (`budget.rs:307`) so the tool's own 600 s clock always fires before the harness's. **Verdict: a new engine changes nothing here.** The header's rejection of hermes's raw-CDP-in-Python model is worth reading before designing a CDP layer, though — the objection was never "CDP is bad", it was "a path that bypasses the six chokepoints is terminal-equivalent".

**`network_policy.rs` (768 lines) is engine-agnostic and operates purely on URL strings.** `SsrfConfig` (`:14-53`) is five booleans-plus-lists: `block_private`, `blocked_domains`, `allowed_domains`, `block_secrets_in_url`, `block_secrets_in_input`, `redact_secrets_in_content`. `BrowserSsrfGuard` (`:143`) is a thin wrapper over the core engine `crate::security::ssrf` and exposes `check_url` (`:176`), `check_navigation` (`:260`), `check_input` (`:278`), `redact_content` (`:293`). Nothing in it knows what a browser is; it takes a `&str` URL or a `&str` of text. **It is hot-swappable** via `ProfileManager::apply_policy` (`manager.rs:197`) storing into an `ArcSwap`, which works because backends are built per call. **Verdict: reusable verbatim by any engine.** The one coupling to watch is that the *post-navigation* half (`post_nav.rs`, 275 lines) needs `list_tabs` text to find the landed URL, so it inherits the `parse_tab_line` format dependency — that is a `tab_registry` coupling, not a policy one.

**`secret_guard.rs` (225 lines) is engine-agnostic and, notably, does not own its patterns.** Its header (`:1-15`) draws the axis clearly: SSRF governs "is this host allowed to be reached", this guard governs "what content is crossing the boundary". `critical_rules()` (`:31-41`) filters `crate::pii::rules::build_rules` down to `PiiSeverity::Critical` in a `OnceLock`, and the doc states the single-source rule outright: "Secret patterns are NOT duplicated here… a new credential pattern added there is automatically enforced at navigation time." Three legs share it — `scan_url_for_secrets` (navigation target, scanning both raw and percent-decoded forms), `scan_text_for_secrets` (form input, deliberately **not** percent-decoding because form input is not URL-encoded), `redact_secrets` (page-content egress). **Verdict: reusable verbatim.** Its only browser-shaped assumption is which *verbs* feed it, and that list lives in the tool layer, not here.

---

## 8. Tests and QA

### `qa/browser_managed/run.sh` — ten scenarios

From `run.sh:30-33` (the literal `case` guard): **`open | ambient | headed | tools | frames | reap | pdf | existing | exec-offload | attach`**.

Notes that matter:
- **`attach` owns `test_drive_attach.py`** as a preflight — run before the build and before a real browser launch, "fail-closed on any non-zero exit; a discovery that silently matches ZERO tests must not read as success either" (`run.sh:39-45`). That wiring is the most recent commit in this area (`b02f65937`, "wire test_drive_attach.py into the attach scenario, or nothing ever runs it").
- `exec-offload` is the only scenario that dials a provider (`run.sh:70`), so it is the only one needing a real model.
- Scratch-HOME discipline shared with `qa/busy_input/run.sh`. §3.12 ① records that **this suite's green is partly a property of `scratch_home.sh`**, orthogonal to what it tests — that is how the keychain defect shipped.
- Drivers: `drive_browser.py`, `drive_tools.py`, `drive_attach.py`, `drive_exec_offload.py`, `add_browser_config.py`, `qa_rpc.py`, plus `pages/`.
- **Only `open` and `attach` were re-measured after the round-7 keychain fix.** The other seven scenarios' readings are pre-fix, and two of their reds are known *not* to be round-7's: `pdf` reports `success: true` while writing a 0-byte PDF (it builds its own `PlaywrightCliDriver`, 判据 §11), and `existing` fails because the local npx cache drifted to `chrome-devtools-mcp 1.8.0` whose schema now requires `pageId`. **"The browser surface is green" is not a sentence anyone can say today.**

### Unit tests pinning backend behaviour

`#[test]` + `#[tokio::test]` counts per file in `src/browser/`:

| count | file |
|---|---|
| 28 | `playwright_cli.rs` |
| 25 | `manager.rs` |
| 24 | `network_policy.rs` |
| 22 | `chromium_resolve.rs` |
| 22 | `chromium_launch.rs` |
| 15 | `chrome_mcp_backend.rs` |
| 14 | `profile.rs` |
| 13 | `tab_registry.rs` |
| 12 | `secret_guard.rs` |
| 10 | `chrome_mcp.rs` |
| 9 | `wait_probe.rs` |
| 9 | `post_nav.rs` |
| 8 | `discovery.rs` |
| 7 | `types.rs` |
| 7 | `playwright_cli_backend.rs` |
| 5 | `playwright_launch.rs` |
| 4 | `testkit.rs` |
| 2 | `error.rs` |
| **0** | **`backend.rs`** |
| 0 | `mod.rs` |

**`backend.rs` has zero tests.** The trait's contract — the load-bearing "`evaluate` returns the value not the transcript" rule — is enforced only indirectly, by `wait_probe`'s guard `a_transcript_that_echoes_the_probe_does_not_read_as_found` and by each backend's own parser tests. A third implementor gets **no compile-time or test-time enforcement of that rule**; it would be free to re-introduce the always-true `wait_for`.

Also note `playwright_cli_backend.rs` has only 7 tests for 779 lines — the thinnest coverage-to-size ratio of any real backend, and the three round-4/5 CRITICALs (never sending `open`, `--headed` as a hard failure, `type` with a ref failing every time) all lived there and were invisible to unit tests.

### `testkit.rs::FakeBackend` — shape and its census guard

`FakeBackend` (`testkit.rs:57`) holds `{calls: Mutex<Vec<String>>, fail_at: Option<usize>, failure_message: String, …}` and each method appends a `verb:detail` entry then returns a trivial `Ok`. `fail_at` is a **1-based ordinal over recorded calls**, so `browser_exec`'s abort path is driven through it. `fmt_target` (`:21`) renders `Ref{ref_id:"e5"}` / `Coords{x:..,y:..}` in the format the exec tests assert on.

Builders (`with_*`) override what the fake **answers**, and each defaults to the value the fake returned before the builder existed, pinned by `builders_default_to_the_historical_answers` (`:423`) — the point of that test is "adding a builder must not quietly change the old answers". Defaults: `DEFAULT_TABS_TEXT = "1: https://example.com"` (`:29`), `snapshot_text = ""`, `evaluate` returns `wait_probe::WAIT_PROBE_FOUND` so any polling path resolves on the first probe, `DEFAULT_FAILURE_MESSAGE = "boom"` (`:32`).

**The structural guard: `fake_backend_implements_every_backend_method` (`testkit.rs:390`)** — a source-level census that `include_str!`s `backend.rs`, extracts every `async fn <name>` and asserts `testkit.rs`'s **production half** (via `utils::source_scan::production_prefix`, so a name mentioned in the test itself does not satisfy the census) contains `async fn <name>(`. It carries a self-protecting assertion (`methods.len() > 20`, "the extractor, not the trait, is broken") and is CRLF-safe. Rationale in its doc: "A method left on a trait default is a hole in the test double that the compiler cannot see."

**⚠️ A dual-engine implication.** That census pins the *fake*, not a real engine. If `BrowserBackend` grows one-sided defaults for a third engine, `FakeBackend` will still be forced to implement them while the new real backend can silently inherit `unsupported_in_existing_session(...)` — an error message that would then name the wrong backend.

---

## 9. FEATURE_LOCATOR §3.12 — round history and the rulings that bind a new engine

`docs/reference/FEATURE_LOCATOR.md:1263` to `:1374` (next heading `### 3.13`). Seven rounds plus a 2026-06-26 hardening. Fifteen bullets, weighted toward what constrains a second engine.

1. **Rounds 1–2 (2026-07-30, 2026-08-06) — dead state machine removed, batching added.** `ProfileState` had zero production writers, so `reap_idle`'s `is_running()` gate was permanently false and ExistingSession idle reaping **never once fired**; the state machine was deleted whole and liveness re-derived from the driver. `make_backend` stopped re-copying the driver→backend map and now delegates to `ProfileManager::get_backend` — **that is why routing has one source today**. Round 2 added `browser_batch` (nine write actions, abort-on-first-failure) and the full `WaitCondition` set.

2. **Round 3 (2026-08-12) — `browser_batch` → `browser_exec`, and the ruling that reversed a prior ruling.** Round 2 had excluded `goto` from batching. Round 3 overturned it, and the stated reason is the one to carry forward: **not "navigation became safe" but "the batch now re-enters that chain instead of bypassing it"** — same functions, no new `ActionType` knob. Keeping the exclusion had a structural cost: all-write actions meant the model had to leave the tool to change page, so reads were unreachable within one call. `exec.rs` has no alias; "a second name is a second source of truth".

3. **Round 3 also refused hermes-agent's `browser_exec` model, and the refusal is directly relevant to a CDP design.** hermes collapses the browser surface into one tool whose argument is **Python with raw CDP access**, defended by a regex over http literals in the model's own source — defeated by string concatenation, and conceded by its authors to be terminal-equivalent. The objection recorded was that it bypasses **all six chokepoints** (SSRF `check_url`, `post_nav` landed-URL re-check, input secret scan, output `redact_and_wrap`, per-`ActionType` approval, `tools/scoped` execution tier). §3.12 also notes the page representation is **identical on both sides** — hermes's digest calls `cdp('Accessibility.getFullAXTree')`, which is what `browser_snapshot` already returns. **The only real gap was composition.**

4. **Round 3, CRITICAL: 15 browser tools refused permanently on any install without a hand-written policy file.** `ConfigApprovalPolicy::load_from`'s **file-not-found** arm returned `safe_default()` (empty defaults) → `Ask` → a refusal string, and **nothing in the repo ever wrote that file**. Fixed by forking on cause: missing file → curated defaults; present but unparseable → still all-Ask ("a broken config never widens").

5. **Round 3, CRITICAL: screenshots never reached the model as images.** `extract_image_in_place` requires a `format` key and `BrowserScreenshotOutput` had none, so every base64 stayed in the **text** channel and got shredded by the result budget. **Rule for any new engine: `{image_base64, format}` must be paired.**

6. **Round 3, HIGH: "the active tab is the last line" was a guess that `switch_tab` falsifies.** Four separate `.next_back()` sites; `ChromeMcpBackend` re-selected on every action (making `browser_tabs{switch}` a **success-reporting no-op**); `PlaywrightCliBackend::navigate` ignored `tab_id` entirely. Consequence was security-relevant: the post-nav audit and read-time SSRF re-check could **vet tab N while the read landed on tab M**. Collapsed into `tab_registry::{parse_tab_line, active_tab_id}` as the single source.

7. **Round 3, D — progressive tool disclosure.** The 26-tool family cost **9,536 B/request**; `BROWSER_RESIDENT_CORE = [browser_open, browser_snapshot, browser_exec]` stays resident and the other 23 are lazily disclosed in work/code mode, measured at **1,590 B**, returning **7,946 B/request**. Two ratchets (`catalog_description_bytes_ratchet`, `REGISTRY_SCHEMA_CEILING_BYTES`) bound tool descriptions and schemas. **Round 6 later found the description ratchet's headroom was 0 B** — any tool description gaining a single byte turns it red, by design. A new engine's tools inherit that.

8. **Round 4 (2026-08-13) — first real-hardware QA, and it found "the default driver had never launched a browser."** `open` was not among the 28 subcommands sent, `NoSession` was constructed with **zero consumers repo-wide** (the canonical severed-wire shape). Fixed by lazy-open at the driver's single chokepoint, gated so a repeat `open` (which is **destructive** — a second open swaps pid and drops every tab) cannot happen. Round 4 also found `--headed` was a hard failure (it is an option of `open`, and `tab-new` exits 1 on it), and that **the real `tab-list` format parsed to zero lines**, which silently swallowed the SSRF landed-URL audit.

9. **Round 4, ⑦ — the sensor that created what it measured.** Lazy-open let the **idle reaper** launch browsers; a unit-test sweep started a real Chrome. Fix: `LaunchPolicy::{OpenIfNeeded, Refuse}` as an explicit **per-call** permission, with only "give me a browser" holding `OpenIfNeeded`. **This is the abstraction a second engine should keep.**

10. **Round 4, ⑨ + Round 5, ③ — the `outputDir` containment ruling, and its correction.** The CLI wrote page snapshots (full accessibility trees) and console logs to a **cwd-relative** `.playwright-cli/`, so browsed page content landed in whatever directory the server started in — caught red-handed with a full snapshot of a visited site in the repo root. Fixed by `outputDir` under `~/.aleph`. **But naming `outputDir` narrowed playwright-core's write roots to `outputDir ∪ process-cwd`**, breaking screenshot / pdf / session-save / upload simultaneously (all of which had only fake-backend tests). The correction was `allowUnrestrictedFileAccess: true`, and the reasoning is the reusable part: **it switches off a second, weaker answer to a question Aleph already answers** (its own protected-location denylist), because `outputDir ∪ cwd` permits the server's start directory and refuses `/tmp` — "a boundary nobody chose". The same call was later made for chrome-devtools-mcp's `--allow-unrestricted-paths` (`profile.rs:268-284`).

11. **Round 5 (2026-08-13), ① CRITICAL — `browser_wait_for` was permanently true on the default driver.** `wait_probe`'s sentinel `ALEPH_WAIT_FOUND` is a **literal inside every probe it builds**, and `playwright-cli eval` echoes the script back under `### Ran Playwright code`, so `out.contains(WAIT_PROBE_FOUND)` was true on the first poll. Every wait was a lie: the model asked to wait for content, was told it had loaded, and acted on an unrendered page. **The fix was refused at the probe level** (no sentinel-splitting tricks) and made at the contract level: **`evaluate` returns the value, not the transcript**. The guard uses a real transcript, not an imagined format. **This is the single most important rule for a third engine, and nothing enforces it structurally.**

12. **Round 5, ② CRITICAL — the `### Error` / exit-0 classifier.** `playwright-cli` exits 1 only on **argument** errors; a thrown eval, an element mismatch, an unhandled modal state, and every `File access denied` are `### Error` **with exit 0**. So `browser_pdf` answered "Saved PDF to <path>" for a file it had been refused. Fixed via `parse_error_section` routed through **the same** `classify_failure`, so "not open" reconstitutes to `NoSession` whichever channel says it. The discrimination rule matters: **only the FIRST `### ` section header counts** — accepting "any line" would let untrusted page text (echoed by `snapshot` / `console`) decide that the call reading it failed.

13. **Round 5, ⑤/⑥ + Round 7, ⑫ — "hang the fallback off the other side's own refusal."** Three instances of one rule. `browser_upload` never opened the file chooser; the fix issues a click **only when the CLI itself says "modal state … browser_file_upload"**, because clicking when a chooser is already open is itself refused. Lazy `open` fires only when the CLI says "is not open" (round 5 ⑥ widened the anchor to the substring `is not open` after finding **two** different phrasings on **two** different channels — stdout for an unknown session, stderr for a recorded-but-closed one, which bricked every persistent profile the reaper touched). Round 7 ⑫ added a **third** refusal wording (a refused `attach --cdp`: exit 1, **empty stdout**, `ECONNREFUSED` on stderr) and joined its two anchors with `&&`, not `||`, because `classify_failure` runs over output that may contain page text and any page can print `ECONNREFUSED`.

14. **Round 6 (2026-08-20), ④ — two faces of one verb handed the matcher different strings.** `browser_exec`'s dialog step passed only the verb as the approval target, on the argument that "prompt text is payload and payload does not enter the policy surface". **That read the wrong layer:** the human sees `approval_display_target`'s scrubbed label, but allow/blocklists **match the raw `target`**. So the narrowing hid nothing and merely made an operator's dialog-payload rule effective on `browser_dialog` and silently inert on the same answer inside a procedure. Fixed by extracting `dialog::dialog_approval_target(action, prompt_text)` as a single derivation shared by both faces, with the `browser_dialog` side a **byte-for-byte no-op**. The guard asserts **both halves** — the matcher can see the payload, the prompt still cannot.

15. **Round 7 (2026-09-05/06) — the launch-chain flip, and its headline is a process failure, not a design.** Aleph now spawns Chromium and `playwright-cli` only does `attach --cdp`. **It shipped a browser that could not navigate at all, and eight task reviews plus 18,411 unit tests caught none of it.** The four rulings it left: ① *"the browser is healthy" and "the browser can navigate" are different questions, and every sentinel asked the first* — the missing `--use-mock-keychain` (bisected to `4c208760a`, isolated variable-by-variable: real HOME 0.62 s, scratch HOME 60.06 s, plus mock-keychain 0.61 s, `--password-store=basic` alone still 60.06 s). ② **owning the launch means owning the whole ~30-switch list** — Aleph now maintains a hand-picked subset (8 today) which is a **second, born-weaker answer** to "how must this browser be configured", corrected only when a symptom surfaces; the durable fix is to *derive* the table from `playwright-core`'s own `chromiumSwitches`, **not done**, and D6 makes this debt grow. ③ **the fixture's green is controlled by a variable orthogonal to what it tests** (`scratch_home.sh`'s HOME redirect is the only reason it caught ①; remove the redirect and the whole suite goes green with the defect intact). ④ **a ceiling is policy, elapsed is observation** — printing `nav_timeout_secs` into the slot every reader parses as a measurement sent three investigators into the wrong subsystem for six hours; the parameter was renamed `timeout_ms` → `elapsed_ms` and fed from `started.elapsed()`.

**Two round-7 open items a new engine inherits:**

- **⑤ (open, unfixed): `playwright-cli`'s daemon namespace is machine-global and the QA HOME isolation does not cover its socket.** `registry.js` hashes `findWorkspaceDir(cwd) || packageRoot`; no Aleph tree has a `.playwright` dir, so **every** call falls back to the constant `packageRoot` — measured, the repo cwd and a temp-dir server produced the **same** hash `8c7961eea1c165d9`. Sockets land in the **real user's `$TMPDIR`**, and Aleph names sessions by **profile name**, most commonly the literal `default`. Consequence: **two Aleph instances on one host, or Aleph plus a developer's own `playwright-cli`, silently share one browser session** — one user's logged-in browser driven by another instance — and each one's `kill-all` kills the other's daemon. The recorded direction is `-s={instance_tag}-{session_key}` where `instance_tag` is a short stable hash of the resolved `~/.aleph` (not the pid, because it must survive a restart), plus printing the full session name into `browser_open` / `runtime_manage` output and the sidecar. **This is a playwright-cli defect, so an engine that drops the CLI drops it too.**
- **⑳ (recorded gap): tab identity does not survive a re-attach.** CDP's target enumeration order is **not stable across attach sessions** — measured, the same browser's two tabs (the launch's own `about:blank` and the profile's real page) swapped order between the first attach and the re-attach **in one run**. `tab_registry::active_tab`'s "last-listed is the best guess" heuristic picked the **wrong** one in that reproduction, and overriding with last-listed picked wrong too. The recorded fix direction is a persistent record one level up in `ProfileManager::tab_registry`. **Plan 2's whole premise is a human and an agent sharing one browser, so it must answer this head-on.**

**Refs and the "leaving the tool invalidates them" ruling** appear in rounds 3 and 6: every action can invalidate the snapshot refs the next action targets, which is *why* `browser_exec` exists and why round 6 added the visual reads (a procedure needing visual confirmation had to leave the tool mid-flow, and that hop invalidated its refs). The abort message explicitly tells the model to re-snapshot. **Nothing enforces ref staleness in code** — it is a documented convention.

---

## Seams an engine-agnostic interface could cut at

My own assessment. Ordered by ratio of value to churn.

1. **`ProfileManager::get_backend` (`manager.rs:383-406`) is the cleanest seam in the subsystem, and it is already the single routing source.** Adding a third `BrowserDriver` arm is a genuinely small change: backends are built **per call** (no cache to invalidate, `manager.rs:196-199`), the manager already holds two long-lived driver handles side by side, and **26 tools and ~39 call sites reach a backend only through `make_backend` / `make_backend_and_tab{,_guarded}`** (`mod.rs:216`, `:225`, `:249`). No tool names a driver. A CDP-native `ObscuraBackend` implementing `BrowserBackend` slots in here with **zero tool-face changes**.
   - The one wire cost: `BrowserDriver` (`profile.rs:24`) is `#[serde]`-exposed under `[general.browser.profiles.<n>].driver`, and adding a variant is a config-schema change. `BrowserType` (`profile.rs:13`) is worse — it enumerates the *Chromium family*, so an `Obscura` variant makes an enum mean two different things.
   - **Recommendation: do not add an arm to `BrowserDriver`.** Split the two axes it currently conflates. It answers "which driver protocol" (managed CLI vs MCP), while a dual-engine design needs "which engine" (obscura vs Chromium) as a separate question — today they happen to be 1:1 because both drivers end at Chromium, and adding a third arm freezes that coincidence into the wire format.

2. **Twenty-three of the trait's twenty-eight required methods return `()`, and that is the actual abstraction, not `-> String`.** For a CDP-native backend, `click`/`type`/`hover`/`scroll`/`select`/`press_key`/`resize`/`drag`/`history`/`dblclick`/`handle_dialog`/`navigate`/`close_tab`/`switch_tab` are a direct mapping onto `Input.*`, `Page.*`, `DOM.*` and need no contract change at all. **Roughly 80% of `BrowserBackend` is already engine-agnostic in substance, not just in signature.**

3. **The five `-> String` methods are the seam that leaks, and `list_tabs` leaks worst.** `list_tabs`, `evaluate`, `console_messages`, `network_log`, `cookies` return a driver's human-readable output and the caller re-parses. `list_tabs` is the load-bearing one: **every interaction tool** pays a `list_tabs` round trip and a `tab_registry::parse_tab_line` parse before it acts, and that parser is a hand-written recogniser for two specific CLI renderings (`tab_registry.rs:201-246`), already wrong once (round 4 ③: the real format parsed to zero lines and silently passed the SSRF audit on an empty list).
   - **Recommendation: change `list_tabs` to return `Vec<TabLine>` before adding an engine.** `TabLine{id, url, selected}` already exists (`tab_registry.rs:189-195`); a CDP backend produces it natively from `Target.getTargets` and the two text backends keep their parsers privately. This is the highest-value refactor in the whole map: it deletes a class of defect rather than an instance, and it costs one signature plus two call-site adaptations (`mod.rs:204-208`, `mod.rs:254-257`) — everything downstream already consumes `TabLine` or an id.

4. **Refs survive a new engine, and this is the pleasant surprise.** `ActionTarget::Ref{ref_id: String}` is format-agnostic, Rust never validates or maps a ref, the two existing drivers **already disagree** on format (`e42` vs `1_6`), and the only Rust code that looks inside a snapshot is `text.matches("[ref=").count()` at `snapshot.rs:96`. An obscura backend can mint its own ref scheme freely.
   - **The two real costs.** (a) That one `ref_count` line is a **Playwright-syntax literal** — a snapshot that does not use `[ref=` reports `ref_count: 0` while showing the model actionable elements, which reads to the model as "this page has nothing to click". Move it behind the backend. (b) **Ref staleness is a documented convention with no code behind it** — no generation counter, no `StaleRef` error variant, no re-snapshot path. A CDP engine minting refs is the first implementation that *could* enforce it, and it should.

5. **`browser_snapshot`'s single `Option<String>` is the seam for the D6 "skip rasterisation" question, and it is unusually free.** There is **no** structured DOM/layout/spatial representation anywhere in the repo (verified negative in §4), so a spatial-state JSON tree is net-new with no incumbent consumer to break. `SnapshotOutput` (`types.rs:106`) is the place to widen — it already carries two fields (`page_url`, `page_title`) that **no caller renders**, which is a live gap the same change could close.
   - Honest caveat: the model-facing output is one fenced text blob passing through `bound_content` → `redact_wrap` → optional offload. Structured JSON has to answer three questions that text answers for free — where does the line-boundary-preserving truncation cut, what does the injection fence wrap, and what does `ctx_search` retrieve from the offloaded copy. **The budget machinery is char-count-shaped and would need a second derivation for a tree.** That is the real cost, and it is not small.

6. **`chromium_launch.rs` is where an engine-neutral process-lifetime layer wants to be, but it is not one today.** Genuinely reusable: the sidecar registry directory, the four-outcome orphan sweep with its `Unreadable` = "keep the record" rule, `LaunchPolicy`, `shutdown_browsers_global` on both daemon exits, and the intent-stamp-before-the-port ordering. Genuinely Chromium-only: `argv()`'s switch list, the `DevToolsActivePort` file protocol, and `parse_devtools_active_port`'s two-line format.
   - **The clean cut is a small trait — spawn a process, tell me when it has published an endpoint, tell me how to kill it — with `ChromiumChild` as its first implementor.** `ChromiumSidecar` then needs an `engine` field, or a mixed-engine host cannot attribute an orphan it reaps.
   - **The switch-list debt (§3.12 ②) will grow, not shrink.** Aleph now maintains a hand-picked 8-of-~30 subset of Playwright's launcher switches, i.e. a born-weaker second answer to "how must this browser be configured", corrected only when a symptom appears. Every future divergence arrives as "works by hand, hangs under Aleph" and costs one investigation. Owning a second engine's launch means owning a second such table.

7. **`ChromiumLocator::locate()` is the one item §3.12 ㉑ correctly flags as a signature change, and I agree.** It is singular, held as `RuntimeManageTool.locator: Arc<dyn ChromiumLocator>` (`runtime_manage.rs:128`), and returns one `RuntimeRow`. Two engines means `locate_all() -> Vec<RuntimeRow>` or a per-engine locator map, plus the `capability: "chromium"` **value** in the tool's wire surface. The schema is already `Option<String>` free-form, so the shape is fine.

8. **Provisioning is closer to ready than the naming suggests.** `RuntimeSpec` + `InstallStrategy{Shell, PowerShell, Via, NpmGlobal}` + `PostInstallAction{RunSubcommand, FnmAlias, AssetProbe}` + `EnvFromConfig` (`src/runtimes/specs.rs`) is a real generic ledger, and **a single static binary on PATH fits it better than Chromium does** — Chromium is carved out precisely because Playwright's per-revision cache is never on PATH (`runtime_manage.rs:10-16`). An obscura spec would be a `Shell`/`PowerShell` install plus an `AssetProbe`, and it would get PATH probing, version parsing, the doctor sentinel and `runtime_manage{list, install}` for free. **This is the least-churn item on the whole list.**

9. **Six chokepoints stay put, and a CDP backend must enter through them rather than beside them.** `network_policy` and `secret_guard` operate on `&str` and are engine-neutral verbatim. `browser_exec` reaches the engine only through `Arc<dyn BrowserBackend>`. The trap is specific: a raw-CDP backend makes it *easy* to expose a `cdp_send(method, params)` escape hatch, and that is **exactly** the hermes model round 3 refused as terminal-equivalent (§3.12 round 3 A). If a raw-CDP verb ships, it needs its own `ActionType`, its own approval gate, and its own place in the input-secret scan — or the six-chokepoint invariant is gone.

10. **Two things I could not verify, stated plainly.**
    - **I ran no builds and no tests.** Every claim here is from reading files at `c049ef1ed`. §3.12's own verification line records **one red test at `775fbd48f`** — `capability::census::tests::no_conditional_capability_install_is_silent`, a false positive naming `runtime_manage.rs:753`, plan-introduced and unfixed as of that entry. I did not check whether it is still red.
    - **I could not verify obscura's capabilities at all** — it is not in this repo, and §3.12 ㉑ says explicitly that generalising the engine seam before obscura is re-measured at HEAD "amounts to drawing it from guesswork". That measurement is still the correct first task, and nothing in this map substitutes for it.
    - **Repo caveat, repeated:** I read `/Volumes/TBU4/Workspace/Aleph` as instructed, but global memory records `/Volumes/TBU/Workspace/Aleph` as the canonical session checkout. Line numbers should be re-confirmed there before anyone edits.

### One-line summary

The abstraction boundary is 240 lines and mostly sound; the Chromium coupling is 5,932 lines (44% of `src/browser/`) concentrated in launch, resolve, discovery and the playwright-CLI driver. A CDP-native backend slots into `get_backend` without touching a single tool face. The three things that need a decision rather than a rename are `list_tabs`'s text shape, `ChromiumLocator`'s singular signature, and whether `BrowserDriver` keeps conflating "which driver protocol" with "which engine".
