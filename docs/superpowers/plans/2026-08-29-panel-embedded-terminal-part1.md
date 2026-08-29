# Panel 内嵌终端 — 实施计划 Part 1（Phase 0–4）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Aleph 零客户端的 `pty.*` 子系统接通到 Panel，并在同一笔里把 VT 仿真器搬到服务端，终点是 Panel 里一个能用的单窗格终端。

**Architecture:** core 侧用已在依赖树的 `vte` 驱动每个 `PtySession` 的屏幕状态（`Grid` + scrollback + 备用屏），按 16ms 节拍把**行级差分**发上 `pty.screen` topic；客户端只是渲染器。因为 `GatewayEventBus` 是会丢帧的 `broadcast`，差分带 `seq`，客户端检测到缺口就 `pty.attach` 拉全量快照并重放在途帧。原始字节 topic `pty.output` 同批 CUT。

**Tech Stack:** Rust / tokio / `portable-pty 0.8` / `vte 0.14.1` / `serde` / Leptos 0.8 (CSR, wasm32) / canvas2d via `web-sys`

**Spec:** `docs/superpowers/specs/2026-08-29-panel-embedded-terminal-design.md`

## Global Constraints

- **分支隔离**：全部工作在 worktree 分支 `worktree-panel-embedded-terminal`，不碰 main。
- **不新增第三方依赖**：`vte` 已在 `alephcore` 依赖树（经 `strip-ansi-escapes`），只需在 `Cargo.toml` 显式声明。**不得引入 xterm.js、任何 npm 运行时依赖、任何新的 VT/终端 crate。**
- **不引入第二个 async runtime**（CLAUDE.md 禁用清单）。
- **锁安全**：全部 `.lock()` 用 `.unwrap_or_else(|e| e.into_inner())`，永不 `.unwrap()`（P7）。
- **UTF-8 安全**：字符串切片用 `char_indices()` / `.get(..n)`，不用 `&s[..n]`（P7）。
- **文件大小**：单文件 200–400 行典型，800 上限（P2）。`screen/` 因此是目录不是单文件 —— 这一点**细化了 spec §3.1 的"新增单文件"措辞**。
- **提交信息**：英文，格式 `<scope>: <description>`。
- **验证集**（每个 Task 的"Run"步骤之外，阶段收尾时跑）：
  ```
  cargo test -p alephcore --lib --no-run
  cargo test -p alephcore --bins
  cargo test -p alephcore --features test-helpers --test '*' --no-run
  cargo test -p aleph-panel --lib          # 真跑，不是 --no-run
  cargo clippy --workspace --all-targets   # 先 just _stage-shell-placeholders
  just wasm                                # 唯一编译出厂形态的命令
  ```
- **`cargo check` 不编译 `#[cfg(test)]`** —— 删 `pub fn` / 字段的同一笔里必须跑 `cargo test --no-run`。

## Part 1 显式不做的（spec 里有，归 Part 2）

自检对着 spec 逐节点名过，前三项**在 spec 里存在但 Part 1 没有任务**；第四项不是 spec 里的条目，而是这份计划自己的 File Structure 表曾经承诺、却没有任何 Task 兑现的一句话（Task 1 fix round 发现）。四项都写在这里而不是让它们静默消失：

| spec 位置 | 项 | 归属 | 理由 |
|---|---|---|---|
| §4.3 | `pty.scrollback {from,to}` RPC | Part 2 | 它的唯一消费者是"往回滚"，而 Part 1 没有滚动条。先建 RPC 就是零客户端通道 —— 正是这个子系统刚犯过的错。服务端**已经存了** scrollback（Task 2/12），Part 2 只需加读取面。 |
| §6.6 | IME（隐藏 `<textarea>` 承接 composition） | Part 2 | Task 17 只处理 `keydown`。**因此 Part 1 交付的终端不能输入中文/日文/韩文**，这写进下方完成判据。 |
| §6.2 §6.3 §6.7 | Tab 条 / 分屏树 / 布局持久化 / 选区 / 搜索 | Part 2 | D1 的 B 档结构，Part 1 交付单窗格。 |
| —（spec 未提及，本计划自身的缺口，Task 7 审查后由 controller 扫出） | Panel 不呈现会话退出 | Part 2 | 服务端一直发 `pty.exit`（Task 8 起改走 `PTY_EXIT_TOPIC`），但 Part 1 的 Panel 只订阅 `pty.screen`，没有任何一个 Task 接退出帧。**后果：用户 `exit` 或 shell 崩掉之后，终端不报错、不变灰、不说话，只是停止更新——一块死掉的矩形**，而「未知」被渲染成了「健康」。之所以划给 Part 2 而不是现在补：Task 15 的 brief 已定稿，为此扩容会在计划中途改掉一个已被扫描判定兼容的任务；而 Part 2 本来就要为 tab 条做会话生命周期。 |
| —（spec 未提及，仅本计划 File Structure 表曾承诺 `esc`） | ESC 族转义序列（`ESC 7`/`ESC 8` DECSC/DECRC 光标保存/恢复、`ESC M` RI 反向换行） | Part 2 | 没有任何 Task 接 `vte::Perform::esc_dispatch`，Part 1 落回 vte 的默认 no-op。**后果：`less` / `vim` 等全屏程序下可能出现光标位置错位**，这写进下方完成判据。 |

---

## File Structure

| 文件 | 职责 | 新建/修改 |
|---|---|---|
| `src/gateway/pty/screen/mod.rs` | `PtyScreen` 门面：`feed` / `take_patch` / `snapshot` / `resize` / `seq` | 新建 |
| `src/gateway/pty/screen/grid.rs` | `Cell` / `Row` / `Grid` / scrollback 环 | 新建 |
| `src/gateway/pty/screen/perform.rs` | `impl vte::Perform`（print / execute / csi / osc） | 新建 |
| `src/gateway/pty/screen/diff.rs` | 脏行跟踪 → `ScreenPatch`，同 SGR run 折叠 | 新建 |
| `src/gateway/pty/session.rs` | reader 喂 screen；删 `pty.output` 编码段；加 flush task | 修改 |
| `src/gateway/pty/manager.rs` | attach 表（`conn_id → 视口`）+ min-size resize + `created_by` | 修改 |
| `src/gateway/pty/jail.rs` | cwd jail：`resolve_spawn_cwd` | 新建 |
| `src/gateway/handlers/pty.rs` | `pty.attach` 新 handler；`spawn` 加闸与 seq | 修改 |
| `shared/protocol/src/pty.rs` | wire 契约（`ScreenPatch` / `PtyScreenFrame` / `AttachResponse` …） | 新建 |
| `interfaces/webchat/src/platform/wide/views/terminal/mod.rs` | 视图外壳 | 新建 |
| `interfaces/webchat/src/platform/wide/views/terminal/session.rs` | 客户端会话：attach / seq / 缺口重同步 / 在途缓冲 | 新建 |
| `interfaces/webchat/src/platform/wide/views/terminal/render.rs` | canvas2d 网格渲染 | 新建 |
| `interfaces/webchat/src/platform/wide/views/terminal/keymap.rs` | 按键 → VT 字节 | 新建 |

---

## Task 0: Worktree 起床 + Phase 0 探针

**这个 Task 不产出生产代码。** 它的交付物是一个**结论**，写进 spec §12。spec 明写：§12 为空即不得进入 Phase 1。

> 判据：「零客户端有两种成因，处置相反，而分辨只要真发一次那个调用」——"没人需要它"是 CUT 候选，"**没有人可能用得了它**"是缺陷，而后者会伪装成前者（找不到客户端的原因恰恰是它不工作）。

**Files:**
- Modify: `docs/superpowers/specs/2026-08-29-panel-embedded-terminal-design.md`（§12）
- Create: `/tmp/pty_probe.py`（一次性探针，不入库）

**Interfaces:**
- Consumes: 无
- Produces: spec §12 的结论文字；后续所有 Task 的前提

- [ ] **Step 1: 补齐 worktree 环境**

worktree 里 submodule 是空的（`skills/` `plugins/` 经 `include_dir!` 编译期嵌入，缺目录直接编译失败），且没有 `node_modules`。

```bash
git submodule update --init --recursive
ls skills/ plugins/          # 两者都必须非空
cd interfaces/webchat && npm install && cd ../..
```

- [ ] **Step 2: 确认基线可编译**

```bash
cargo test -p alephcore --lib --no-run
```
Expected: 编译通过。**若失败，先停下报告** —— 基线红意味着后面任何"红"都不可归因。

- [ ] **Step 3: 起一个 server**

```bash
cargo run --bin aleph-server &
sleep 8
```

- [ ] **Step 4: 写探针，真发一次调用**

```python
# /tmp/pty_probe.py — 一次性，不入库
import asyncio, json, base64, sys, websockets

URL = "ws://127.0.0.1:18790/ws"

async def main():
    async with websockets.connect(URL, origin="http://127.0.0.1:18790") as ws:
        async def call(method, params, rid):
            await ws.send(json.dumps({"jsonrpc": "2.0", "id": rid,
                                      "method": method, "params": params}))

        await call("connect", {}, 1)
        print("connect ->", await ws.recv())

        # 订阅必须在 spawn 之前：订阅之后发生的事才收得到
        await call("events.subscribe", {"topics": ["pty.*"]}, 2)
        print("subscribe ->", await ws.recv())

        await call("pty.spawn", {"rows": 24, "cols": 80}, 3)
        spawn = json.loads(await ws.recv())
        print("spawn ->", spawn)
        sid = (spawn.get("result") or {}).get("session_id")
        if not sid:
            print("VERDICT: pty.spawn 本身失败 —— 这是缺陷，不是无人需要")
            return

        await call("pty.input", {"session_id": sid, "data": "echo ALEPH_PROBE_OK\n"}, 4)

        got = []
        try:
            async with asyncio.timeout(5):
                while True:
                    msg = json.loads(await ws.recv())
                    if msg.get("topic") == "pty.output":
                        got.append(base64.b64decode(msg["data"]["data"]).decode("utf8", "replace"))
                        if "ALEPH_PROBE_OK" in "".join(got):
                            break
        except TimeoutError:
            pass

        blob = "".join(got)
        print("bytes received:", len(blob))
        print(repr(blob[:400]))
        print("VERDICT:", "WORKS — 零客户端是'没人需要'"
              if "ALEPH_PROBE_OK" in blob
              else "BROKEN — 零客户端是'没有人可能用得了'")

asyncio.run(main())
```

```bash
python3 -m pip install --quiet websockets
python3 /tmp/pty_probe.py 2>&1 | tee /tmp/pty_probe.log
```

- [ ] **Step 5: 记录结论到 spec §12**

把 `/tmp/pty_probe.log` 的 VERDICT 行与关键观察写进 spec §12，替换掉"待填"那段。至少回答四问：

1. `pty.spawn` 返回 session_id 了吗？
2. `pty.output` 帧到达了吗？base64 解出来是不是预期字节？
3. `attach_event_bus` 真被调到了吗（若无帧，去 `server/mod.rs:753` 确认）？
4. loopback 连接过得了 `"pty."` 的 admin 闸吗？

**若 VERDICT 是 BROKEN**：停下报告。Task 1 之后的阶段划分可能要重排 —— 先修断口再谈持屏。

- [ ] **Step 6: 收摊并提交**

```bash
kill %1
git add docs/superpowers/specs/2026-08-29-panel-embedded-terminal-design.md
git commit -m "docs: record Phase 0 pty probe verdict in the terminal spec"
```

---

## Task 1: `vte 0.14` API 尖刺（spike）

设计里所有"vte 就地解析"的话都建立在一个**未验证**的前提上：`vte::Perform` 的 trait 形状与 `Parser::advance` 的签名。0.13→0.14 之间 `advance` 改过参数形态。**这个 Task 的存在就是为了把不确定性关在这里**，让 Task 2 起建立在验证过的 API 上。

**Files:**
- Modify: `Cargo.toml`（显式声明 `vte`）
- Create: `src/gateway/pty/screen/mod.rs`（仅一个 spike 测试）

**Interfaces:**
- Consumes: 无
- Produces: 已验证的 `vte::Perform` 方法签名与 `Parser::advance` 调用形态，供 Task 2–5 使用

- [ ] **Step 1: 显式声明 vte 依赖**

它已在依赖树里（`strip-ansi-escapes` 传递引入），但传递依赖不能直接 `use`。在 `Cargo.toml` 的 `[dependencies]` 加一行（版本对齐 `Cargo.lock` 里已有的 `0.14.1`，避免拉进第二份）：

```toml
vte = "0.14"
```

```bash
cargo tree -p alephcore -i vte --depth 1
```
Expected: 只有一个 `vte v0.14.1`，没有第二个版本。

- [ ] **Step 2: 写 spike 测试**

Create `src/gateway/pty/screen/mod.rs`：

```rust
//! Server-side terminal screen state.
//!
//! The VT emulator lives here rather than in the client so that reconnect,
//! backpressure and multi-client screen sharing fall out of the architecture:
//! the server holds the screen, so `pty.attach` can hand a fresh client a full
//! snapshot, and what goes on the wire is a bounded per-frame diff instead of
//! an unbounded byte stream.

#[cfg(test)]
mod tests {
    /// Pins the `vte` API surface this module is built on. If `vte` changes
    /// `Perform`'s method signatures or how `advance` is called, this test
    /// fails first and names the change, instead of every emulator test
    /// failing at once with a confusing error.
    #[test]
    fn vte_perform_api_is_the_shape_this_module_assumes() {
        #[derive(Default)]
        struct Probe {
            printed: String,
            executed: Vec<u8>,
            csi: Vec<(Vec<u16>, char)>,
            osc: Vec<Vec<u8>>,
        }

        impl vte::Perform for Probe {
            fn print(&mut self, c: char) {
                self.printed.push(c);
            }
            fn execute(&mut self, byte: u8) {
                self.executed.push(byte);
            }
            fn csi_dispatch(
                &mut self,
                params: &vte::Params,
                _intermediates: &[u8],
                _ignore: bool,
                action: char,
            ) {
                let flat: Vec<u16> = params.iter().map(|p| p.first().copied().unwrap_or(0)).collect();
                self.csi.push((flat, action));
            }
            fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
                self.osc.push(params.iter().flat_map(|p| p.to_vec()).collect());
            }
        }

        let mut parser = vte::Parser::new();
        let mut probe = Probe::default();
        parser.advance(&mut probe, b"hi\r\n\x1b[31m\x1b]0;title\x07");

        assert_eq!(probe.printed, "hi", "print() must receive printable chars");
        assert_eq!(probe.executed, vec![b'\r', b'\n'], "execute() must receive C0 controls");
        assert_eq!(probe.csi, vec![(vec![31], 'm')], "csi_dispatch must receive SGR 31");
        assert_eq!(probe.osc.len(), 1, "osc_dispatch must fire for OSC 0");
    }
}
```

Add to `src/gateway/pty/mod.rs`:

```rust
pub mod screen;
```

- [ ] **Step 3: 跑它**

```bash
cargo test -p alephcore --lib gateway::pty::screen::tests::vte_perform_api -- --nocapture
```

Expected: **PASS**。

**若编译失败**（签名不符）：这正是这个 spike 的目的。按 rustc 报的真实签名改测试，让它通过，然后**在提交信息里写明实际签名与本处假设的差异**，并停下报告 —— Task 2–5 的代码要照真实签名调整。特别注意两处历史变化点：
- `Parser::advance` 是否收 `&[u8]`（本处假设）还是逐字节 `u8`；
- `Params::iter()` 的 item 是 `&[u16]`（子参数切片，本处假设）还是 `u16`。

- [ ] **Step 4: 提交**

```bash
git add Cargo.toml src/gateway/pty/mod.rs src/gateway/pty/screen/mod.rs
git commit -m "pty: pin the vte API surface the screen emulator is built on

vte was already in the dependency tree via strip-ansi-escapes but was never
declared, so it could not be used directly. Declaring it at the version the
lockfile already carries avoids pulling a second copy.

The test is a spike made permanent: it fails first and names the change when
vte moves, instead of every emulator test failing at once."
```

---

## Task 2: `Cell` / `Row` / `Grid` 基础网格 + 可打印字符

**Files:**
- Create: `src/gateway/pty/screen/grid.rs`
- Modify: `src/gateway/pty/screen/mod.rs`

**Interfaces:**
- Consumes: Task 1 验证过的 `vte::Perform`
- Produces:
  - `pub struct Cell { pub ch: char, pub fg: Color, pub bg: Color, pub attrs: Attrs }`
  - `pub enum Color { Default, Indexed(u8), Rgb(u8, u8, u8) }`
  - `pub struct Attrs(u8)` with `BOLD/ITALIC/UNDERLINE/REVERSE` 常量
  - `pub struct Grid { rows: u16, cols: u16, .. }`
  - `Grid::new(rows: u16, cols: u16) -> Grid`
  - `Grid::row_text(&self, row: u16) -> String`（测试用）
  - `Grid::put(&mut self, c: char, style: (Color, Color, Attrs))`（在光标处写并前进）
  - `Grid::cursor(&self) -> (u16, u16)`（`(row, col)`）

- [ ] **Step 1: 写失败的测试**

Add to `src/gateway/pty/screen/grid.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: (Color, Color, Attrs) = (Color::Default, Color::Default, Attrs::NONE);

    #[test]
    fn printing_advances_the_cursor_and_lands_in_the_row() {
        let mut g = Grid::new(3, 10);
        for c in "hello".chars() {
            g.put(c, PLAIN);
        }
        assert_eq!(g.row_text(0), "hello");
        assert_eq!(g.cursor(), (0, 5));
    }

    /// A CJK glyph occupies two columns. Getting this wrong is invisible in
    /// ASCII tests and then misaligns every table a user ever prints.
    #[test]
    fn wide_chars_take_two_columns_and_leave_a_spacer() {
        let mut g = Grid::new(2, 10);
        g.put('中', PLAIN);
        assert_eq!(g.cursor(), (0, 2), "a wide glyph advances the cursor by two");
        assert_eq!(g.row_text(0), "中", "the spacer cell must not surface as a char");
    }

    /// Writing past the last column wraps to the next row rather than
    /// silently dropping the character.
    #[test]
    fn printing_past_the_last_column_wraps() {
        let mut g = Grid::new(2, 3);
        for c in "abcd".chars() {
            g.put(c, PLAIN);
        }
        assert_eq!(g.row_text(0), "abc");
        assert_eq!(g.row_text(1), "d");
        assert_eq!(g.cursor(), (1, 1));
    }
}
```

- [ ] **Step 2: 跑它，确认失败**

```bash
cargo test -p alephcore --lib gateway::pty::screen::grid
```
Expected: FAIL —— `cannot find struct Grid` 之类。

- [ ] **Step 3: 实现**

`src/gateway/pty/screen/grid.rs`（把上面的 `mod tests` 留在文件末尾）：

```rust
//! The character grid: cells, rows, cursor, and the scrollback ring.

use unicode_width::UnicodeWidthChar;

/// A single cell's colour. `Default` means "whatever the client's theme
/// says", which is why it is a variant rather than a concrete RGB — the
/// server does not know the client's palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// Bitflags for the SGR attributes we render. Kept to one byte so a `Cell`
/// stays small — a 1000-line scrollback at 200 columns is 200k cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Attrs(pub u8);

impl Attrs {
    pub const NONE: Self = Self(0);
    pub const BOLD: Self = Self(1 << 0);
    pub const ITALIC: Self = Self(1 << 1);
    pub const UNDERLINE: Self = Self(1 << 2);
    pub const REVERSE: Self = Self(1 << 3);

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }
}

/// One cell. `ch == '\0'` marks the right half of a double-width glyph: it
/// holds no character of its own but must not be overwritten independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
}

impl Default for Cell {
    fn default() -> Self {
        Self { ch: ' ', fg: Color::Default, bg: Color::Default, attrs: Attrs::NONE }
    }
}

impl Cell {
    /// The spacer that follows a double-width glyph.
    pub(crate) const SPACER: char = '\0';

    pub(crate) fn is_spacer(self) -> bool {
        self.ch == Self::SPACER
    }
}

/// The visible screen plus its cursor. Scrollback lands in [`Grid::scrollback`]
/// as rows fall off the top.
#[derive(Debug)]
pub struct Grid {
    rows: u16,
    cols: u16,
    cells: Vec<Cell>,
    cursor_row: u16,
    cursor_col: u16,
    scrollback: std::collections::VecDeque<Vec<Cell>>,
    scrollback_limit: usize,
}

impl Grid {
    #[must_use]
    pub fn new(rows: u16, cols: u16) -> Self {
        let (rows, cols) = (rows.max(1), cols.max(1));
        Self {
            rows,
            cols,
            cells: vec![Cell::default(); rows as usize * cols as usize],
            cursor_row: 0,
            cursor_col: 0,
            scrollback: std::collections::VecDeque::new(),
            scrollback_limit: 1000,
        }
    }

    #[must_use]
    pub const fn dims(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    #[must_use]
    pub const fn cursor(&self) -> (u16, u16) {
        (self.cursor_row, self.cursor_col)
    }

    fn idx(&self, row: u16, col: u16) -> usize {
        row as usize * self.cols as usize + col as usize
    }

    #[must_use]
    pub fn row_cells(&self, row: u16) -> &[Cell] {
        let start = self.idx(row.min(self.rows - 1), 0);
        &self.cells[start..start + self.cols as usize]
    }

    /// Row rendered as text, spacers dropped and trailing blanks trimmed.
    /// Test-facing; the wire uses [`Self::row_cells`].
    #[must_use]
    pub fn row_text(&self, row: u16) -> String {
        let s: String = self
            .row_cells(row)
            .iter()
            .filter(|c| !c.is_spacer())
            .map(|c| c.ch)
            .collect();
        s.trim_end().to_string()
    }

    /// Write `c` at the cursor with `style`, advancing the cursor. Wraps to
    /// the next row at the right margin and scrolls at the bottom.
    pub fn put(&mut self, c: char, style: (Color, Color, Attrs)) {
        let width = UnicodeWidthChar::width(c).unwrap_or(0);
        if width == 0 {
            return;
        }
        let w = width as u16;
        if self.cursor_col + w > self.cols {
            self.newline();
            self.cursor_col = 0;
        }
        let (fg, bg, attrs) = style;
        let i = self.idx(self.cursor_row, self.cursor_col);
        self.cells[i] = Cell { ch: c, fg, bg, attrs };
        if w == 2 {
            let j = self.idx(self.cursor_row, self.cursor_col + 1);
            self.cells[j] = Cell { ch: Cell::SPACER, fg, bg, attrs };
        }
        self.cursor_col += w;
    }

    /// Move to the next row, scrolling the top row into scrollback when the
    /// cursor is already on the last row.
    pub fn newline(&mut self) {
        if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
        } else {
            self.scroll_up();
        }
    }

    pub fn carriage_return(&mut self) {
        self.cursor_col = 0;
    }

    fn scroll_up(&mut self) {
        let first: Vec<Cell> = self.row_cells(0).to_vec();
        if self.scrollback.len() == self.scrollback_limit {
            self.scrollback.pop_front();
        }
        self.scrollback.push_back(first);
        self.cells.rotate_left(self.cols as usize);
        let start = self.idx(self.rows - 1, 0);
        for cell in &mut self.cells[start..] {
            *cell = Cell::default();
        }
    }
}
```

在 `Cargo.toml` 确认 `unicode-width` 可用；若不在 `[dependencies]` 则加 `unicode-width = "0.2"`（它已被多个既有依赖使用，先跑 `cargo tree -i unicode-width` 对齐版本，不引入第二份）。

在 `src/gateway/pty/screen/mod.rs` 加：

```rust
pub mod grid;
pub use grid::{Attrs, Cell, Color, Grid};
```

- [ ] **Step 4: 跑测试，确认通过**

```bash
cargo test -p alephcore --lib gateway::pty::screen::grid
```
Expected: 3 passed。

- [ ] **Step 5: 提交**

```bash
git add Cargo.toml src/gateway/pty/screen/
git commit -m "pty: add the character grid with wide-glyph and wrap semantics

A CJK glyph occupies two columns; getting that wrong is invisible under
ASCII tests and then misaligns every table the user prints, so the spacer
cell is part of the model rather than a rendering detail."
```

---

## Task 3: `Perform` 实现 —— 可打印字符 · C0 控制 · SGR

**Files:**
- Create: `src/gateway/pty/screen/perform.rs`
- Modify: `src/gateway/pty/screen/mod.rs`

**Interfaces:**
- Consumes: `Grid`, `Cell`, `Color`, `Attrs`（Task 2）
- Produces:
  - `pub struct Screen { pub grid: Grid, .. }`
  - `Screen::new(rows: u16, cols: u16) -> Screen`
  - `Screen::feed(&mut self, bytes: &[u8])`
  - `Screen::title(&self) -> Option<&str>`
  - `Screen::take_bell(&mut self) -> bool`

- [ ] **Step 1: 写失败的测试**

Add to `src/gateway/pty/screen/perform.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::pty::screen::grid::{Attrs, Color};

    #[test]
    fn plain_text_and_newlines_land_on_the_grid() {
        let mut s = Screen::new(4, 20);
        s.feed(b"one\r\ntwo\r\n");
        assert_eq!(s.grid.row_text(0), "one");
        assert_eq!(s.grid.row_text(1), "two");
    }

    #[test]
    fn sgr_sets_and_resets_colour_and_bold() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b[1;31mR\x1b[0mp");
        let row = s.grid.row_cells(0);
        assert_eq!(row[0].fg, Color::Indexed(1), "SGR 31 is indexed red");
        assert!(row[0].attrs.contains(Attrs::BOLD), "SGR 1 is bold");
        assert_eq!(row[1].fg, Color::Default, "SGR 0 resets");
        assert!(!row[1].attrs.contains(Attrs::BOLD));
    }

    #[test]
    fn sgr_38_2_sets_truecolour() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b[38;2;10;20;30mX");
        assert_eq!(s.grid.row_cells(0)[0].fg, Color::Rgb(10, 20, 30));
    }

    /// OSC 0/2 is how a shell renames its tab. It reaches the client as the
    /// tab label, so it is protocol, not decoration.
    #[test]
    fn osc_zero_sets_the_title() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b]0;my-title\x07");
        assert_eq!(s.title(), Some("my-title"));
    }

    /// A title arriving in pieces across two reads must not be truncated.
    #[test]
    fn a_split_osc_sequence_still_yields_the_whole_title() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b]0;my-");
        s.feed(b"title\x07");
        assert_eq!(s.title(), Some("my-title"));
    }
}
```

- [ ] **Step 2: 跑它，确认失败**

```bash
cargo test -p alephcore --lib gateway::pty::screen::perform
```
Expected: FAIL —— `cannot find struct Screen`。

- [ ] **Step 3: 实现**

`src/gateway/pty/screen/perform.rs`（`mod tests` 留在末尾）：

```rust
//! `vte::Perform` implementation — turns the PTY byte stream into grid writes.

use super::grid::{Attrs, Color, Grid};

/// The parser plus the state it mutates. `Parser` is retained across `feed`
/// calls because escape sequences straddle read boundaries: an OSC title can
/// arrive in two chunks, and a parser rebuilt per read would lose the tail.
pub struct Screen {
    pub grid: Grid,
    parser: vte::Parser,
    state: ScreenState,
}

#[derive(Default)]
struct ScreenState {
    fg: Color,
    bg: Color,
    attrs: Attrs,
    title: Option<String>,
    bell: bool,
}

impl Screen {
    #[must_use]
    pub fn new(rows: u16, cols: u16) -> Self {
        Self { grid: Grid::new(rows, cols), parser: vte::Parser::new(), state: ScreenState::default() }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        let mut performer = Performer { grid: &mut self.grid, state: &mut self.state };
        self.parser.advance(&mut performer, bytes);
    }

    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.state.title.as_deref()
    }

    /// Reads and clears the bell flag — a bell is an edge, not a level.
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.state.bell)
    }
}

struct Performer<'a> {
    grid: &'a mut Grid,
    state: &'a mut ScreenState,
}

impl Performer<'_> {
    fn style(&self) -> (Color, Color, Attrs) {
        (self.state.fg, self.state.bg, self.state.attrs)
    }

    /// SGR. Consumes the parameter list because 38/48 take trailing
    /// arguments, so this cannot be a per-parameter loop.
    fn sgr(&mut self, params: &[u16]) {
        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => {
                    self.state.fg = Color::Default;
                    self.state.bg = Color::Default;
                    self.state.attrs = Attrs::NONE;
                }
                1 => self.state.attrs.insert(Attrs::BOLD),
                3 => self.state.attrs.insert(Attrs::ITALIC),
                4 => self.state.attrs.insert(Attrs::UNDERLINE),
                7 => self.state.attrs.insert(Attrs::REVERSE),
                22 => self.state.attrs.remove(Attrs::BOLD),
                23 => self.state.attrs.remove(Attrs::ITALIC),
                24 => self.state.attrs.remove(Attrs::UNDERLINE),
                27 => self.state.attrs.remove(Attrs::REVERSE),
                30..=37 => self.state.fg = Color::Indexed((params[i] - 30) as u8),
                39 => self.state.fg = Color::Default,
                40..=47 => self.state.bg = Color::Indexed((params[i] - 40) as u8),
                49 => self.state.bg = Color::Default,
                90..=97 => self.state.fg = Color::Indexed((params[i] - 90 + 8) as u8),
                100..=107 => self.state.bg = Color::Indexed((params[i] - 100 + 8) as u8),
                38 | 48 => {
                    let is_fg = params[i] == 38;
                    // 38;5;N (indexed) or 38;2;R;G;B (truecolour). A malformed
                    // run is skipped rather than mis-parsed into the next
                    // parameter, which would recolour unrelated text.
                    match params.get(i + 1) {
                        Some(5) => {
                            if let Some(&n) = params.get(i + 2) {
                                let c = Color::Indexed(n as u8);
                                if is_fg { self.state.fg = c } else { self.state.bg = c }
                            }
                            i += 2;
                        }
                        Some(2) => {
                            if let (Some(&r), Some(&g), Some(&b)) =
                                (params.get(i + 2), params.get(i + 3), params.get(i + 4))
                            {
                                let c = Color::Rgb(r as u8, g as u8, b as u8);
                                if is_fg { self.state.fg = c } else { self.state.bg = c }
                            }
                            i += 4;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

impl vte::Perform for Performer<'_> {
    fn print(&mut self, c: char) {
        let style = self.style();
        self.grid.put(c, style);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.grid.newline(),
            b'\r' => self.grid.carriage_return(),
            0x07 => self.state.bell = true,
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &vte::Params, _inter: &[u8], _ignore: bool, action: char) {
        // Flatten sub-parameters: only SGR's 38/48 use them, and it reads the
        // colon form (38:2:r:g:b) identically to the semicolon form.
        let flat: Vec<u16> = params.iter().flat_map(|p| p.iter().copied()).collect();
        if action == 'm' {
            let effective: &[u16] = if flat.is_empty() { &[0] } else { &flat };
            self.sgr(effective);
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // OSC 0 = icon + title, OSC 2 = title.
        let Some(kind) = params.first() else { return };
        if matches!(*kind, b"0" | b"2") {
            if let Some(raw) = params.get(1) {
                self.state.title = Some(String::from_utf8_lossy(raw).into_owned());
            }
        }
    }
}
```

`src/gateway/pty/screen/mod.rs` 加：

```rust
pub mod perform;
pub use perform::Screen;
```

- [ ] **Step 4: 跑测试，确认通过**

```bash
cargo test -p alephcore --lib gateway::pty::screen::perform
```
Expected: 5 passed。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/pty/screen/
git commit -m "pty: parse printable text, C0 controls and SGR into the grid

The vte Parser is retained across feed() calls on purpose: escape sequences
straddle read boundaries, and a parser rebuilt per read silently truncates a
title that arrived in two chunks."
```

---

## Task 4: 光标移动 · 擦除 · 滚动区

**Files:**
- Modify: `src/gateway/pty/screen/grid.rs`, `src/gateway/pty/screen/perform.rs`

**Interfaces:**
- Consumes: `Screen`, `Grid`（Task 2–3）
- Produces:
  - `Grid::goto(&mut self, row: u16, col: u16)`
  - `Grid::erase_in_display(&mut self, mode: u16)`
  - `Grid::erase_in_line(&mut self, mode: u16)`
  - `Grid::move_cursor(&mut self, d_row: i32, d_col: i32)`

- [ ] **Step 1: 写失败的测试**

Add to `perform.rs` 的 `mod tests`:

```rust
    #[test]
    fn cup_moves_the_cursor_one_based() {
        let mut s = Screen::new(5, 20);
        s.feed(b"\x1b[3;7HX");
        // CSI row;col H is 1-based; row 3 col 7 is grid (2, 6).
        assert_eq!(s.grid.row_cells(2)[6].ch, 'X');
    }

    #[test]
    fn cup_without_params_homes_the_cursor() {
        let mut s = Screen::new(3, 10);
        s.feed(b"abc\r\ndef\x1b[HZ");
        assert_eq!(s.grid.row_text(0), "Zbc");
    }

    #[test]
    fn erase_in_line_to_end_clears_the_tail_only() {
        let mut s = Screen::new(2, 10);
        s.feed(b"abcdef\x1b[1;4H\x1b[0K");
        assert_eq!(s.grid.row_text(0), "abc");
    }

    #[test]
    fn erase_in_display_two_clears_everything() {
        let mut s = Screen::new(3, 10);
        s.feed(b"aaa\r\nbbb\x1b[2J");
        assert_eq!(s.grid.row_text(0), "");
        assert_eq!(s.grid.row_text(1), "");
    }

    /// Cursor-up at the top row must clamp, not underflow. This is the
    /// arithmetic that panics in debug and wraps in release if written with
    /// unsigned subtraction.
    #[test]
    fn cursor_up_at_the_top_row_clamps() {
        let mut s = Screen::new(3, 10);
        s.feed(b"\x1b[10A\x1b[10DX");
        assert_eq!(s.grid.cursor().0, 0);
        assert_eq!(s.grid.row_cells(0)[0].ch, 'X');
    }
```

- [ ] **Step 2: 跑它，确认失败**

```bash
cargo test -p alephcore --lib gateway::pty::screen::perform
```
Expected: 新增 5 条 FAIL。

- [ ] **Step 3: 实现**

Add to `grid.rs`（`impl Grid` 内）：

```rust
    /// Absolute cursor move, clamped to the grid. Callers pass 0-based
    /// coordinates; the 1-based CSI convention is converted by the caller.
    pub fn goto(&mut self, row: u16, col: u16) {
        self.cursor_row = row.min(self.rows - 1);
        self.cursor_col = col.min(self.cols - 1);
    }

    /// Relative cursor move, clamped at every edge. Signed deltas because
    /// unsigned subtraction here panics in debug and wraps in release.
    pub fn move_cursor(&mut self, d_row: i32, d_col: i32) {
        let r = i64::from(self.cursor_row) + i64::from(d_row);
        let c = i64::from(self.cursor_col) + i64::from(d_col);
        self.cursor_row = r.clamp(0, i64::from(self.rows - 1)) as u16;
        self.cursor_col = c.clamp(0, i64::from(self.cols - 1)) as u16;
    }

    /// CSI J. 0 = cursor to end, 1 = start to cursor, 2/3 = all.
    pub fn erase_in_display(&mut self, mode: u16) {
        let cur = self.idx(self.cursor_row, self.cursor_col);
        let range = match mode {
            0 => cur..self.cells.len(),
            1 => 0..=cur.min(self.cells.len().saturating_sub(1)),
            _ => 0..self.cells.len(),
        }
        .collect::<Vec<_>>();
        for i in range {
            self.cells[i] = Cell::default();
        }
    }

    /// CSI K. 0 = cursor to end of line, 1 = start of line to cursor, 2 = line.
    pub fn erase_in_line(&mut self, mode: u16) {
        let start = self.idx(self.cursor_row, 0);
        let end = start + self.cols as usize;
        let cur = self.idx(self.cursor_row, self.cursor_col);
        let (from, to) = match mode {
            0 => (cur, end),
            1 => (start, (cur + 1).min(end)),
            _ => (start, end),
        };
        for cell in &mut self.cells[from..to] {
            *cell = Cell::default();
        }
    }
```

`erase_in_display` 的 `1` 臂用了 `RangeInclusive`，与另外两臂类型不同 —— 上面用 `.collect::<Vec<_>>()` 抹平。若 clippy 抱怨分配，改写成三个显式循环。

Add to `perform.rs` 的 `csi_dispatch`（在 `if action == 'm'` 之后）：

```rust
        let p = |n: usize, default: u16| -> u16 {
            flat.get(n).copied().filter(|v| *v != 0).unwrap_or(default)
        };
        match action {
            'H' | 'f' => self.grid.goto(p(0, 1) - 1, p(1, 1) - 1),
            'A' => self.grid.move_cursor(-i32::from(p(0, 1)), 0),
            'B' => self.grid.move_cursor(i32::from(p(0, 1)), 0),
            'C' => self.grid.move_cursor(0, i32::from(p(0, 1))),
            'D' => self.grid.move_cursor(0, -i32::from(p(0, 1))),
            'J' => self.grid.erase_in_display(flat.first().copied().unwrap_or(0)),
            'K' => self.grid.erase_in_line(flat.first().copied().unwrap_or(0)),
            _ => {}
        }
```

- [ ] **Step 4: 跑测试，确认通过**

```bash
cargo test -p alephcore --lib gateway::pty::screen::
```
Expected: 全部 passed（Task 1–4 共 13 条）。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/pty/screen/
git commit -m "pty: cursor positioning and erase sequences

move_cursor takes signed deltas: the same operation written with unsigned
subtraction panics in debug at the top row and wraps in release, which is
the same byte behaving two ways in two profiles."
```

---

## Task 5: 备用屏 + resize

不做备用屏，`vim` / `htop` / `opencode` 全废 —— 它们一进来就切 `\e[?1049h`，退出时切回并期望原屏完好。

**Files:**
- Modify: `src/gateway/pty/screen/perform.rs`, `src/gateway/pty/screen/grid.rs`

**Interfaces:**
- Consumes: `Screen`, `Grid`
- Produces:
  - `Screen::alt_screen(&self) -> bool`
  - `Grid::resize(&mut self, rows: u16, cols: u16)`

- [ ] **Step 1: 写失败的测试**

Add to `perform.rs` 的 `mod tests`:

```rust
    #[test]
    fn alt_screen_is_separate_and_the_primary_survives_the_round_trip() {
        let mut s = Screen::new(3, 20);
        s.feed(b"primary");
        s.feed(b"\x1b[?1049h");
        assert!(s.alt_screen());
        assert_eq!(s.grid.row_text(0), "", "the alt screen starts blank");
        s.feed(b"alt");
        assert_eq!(s.grid.row_text(0), "alt");
        s.feed(b"\x1b[?1049l");
        assert!(!s.alt_screen());
        assert_eq!(s.grid.row_text(0), "primary", "the primary screen must survive");
    }

    #[test]
    fn resize_preserves_content_that_still_fits() {
        let mut s = Screen::new(3, 20);
        s.feed(b"hello");
        s.resize(5, 40);
        assert_eq!(s.grid.dims(), (5, 40));
        assert_eq!(s.grid.row_text(0), "hello");
    }
```

- [ ] **Step 2: 跑它，确认失败**

```bash
cargo test -p alephcore --lib gateway::pty::screen::perform
```
Expected: 2 条 FAIL。

- [ ] **Step 3: 实现**

Add to `grid.rs`（`impl Grid`）：

```rust
    /// Resize, keeping the top-left content that still fits. Reflow is
    /// deliberately not attempted: a wrong reflow scrambles a screen the user
    /// is looking at, whereas clipping is legible and self-corrects on the
    /// application's next repaint (which every full-screen app does on SIGWINCH).
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(1), cols.max(1));
        if (rows, cols) == (self.rows, self.cols) {
            return;
        }
        let mut next = vec![Cell::default(); rows as usize * cols as usize];
        for r in 0..rows.min(self.rows) {
            for c in 0..cols.min(self.cols) {
                next[r as usize * cols as usize + c as usize] = self.cells[self.idx(r, c)];
            }
        }
        self.cells = next;
        self.rows = rows;
        self.cols = cols;
        self.cursor_row = self.cursor_row.min(rows - 1);
        self.cursor_col = self.cursor_col.min(cols - 1);
    }
```

Modify `perform.rs` —— `Screen` 持有两块屏：

```rust
pub struct Screen {
    pub grid: Grid,
    /// The saved primary screen while the alternate screen is active.
    saved: Option<Grid>,
    parser: vte::Parser,
    state: ScreenState,
}
```

`Screen::new` 加 `saved: None`。加两个方法：

```rust
    #[must_use]
    pub const fn alt_screen(&self) -> bool {
        self.saved.is_some()
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.grid.resize(rows, cols);
        if let Some(saved) = &mut self.saved {
            saved.resize(rows, cols);
        }
    }
```

`ScreenState` 加一个待处理的模式切换（`Performer` 借着 `grid` 的可变引用，换屏不能在它里面做）：

```rust
#[derive(Default)]
struct ScreenState {
    fg: Color,
    bg: Color,
    attrs: Attrs,
    title: Option<String>,
    bell: bool,
    /// Set by `csi_dispatch` for `?1049h/l`; applied by `feed` after the
    /// parser returns, because swapping the grid needs ownership the
    /// Performer's borrow does not have.
    pending_alt: Option<bool>,
}
```

`csi_dispatch` 的 `match action` 加一臂（要看 `_inter` 是否为 `b"?"`，把参数名从 `_inter` 改成 `inter`）：

```rust
            'h' | 'l' if inter == b"?" && flat.first() == Some(&1049) => {
                self.state.pending_alt = Some(action == 'h');
            }
```

`Screen::feed` 结尾处理它：

```rust
    pub fn feed(&mut self, bytes: &[u8]) {
        {
            let mut performer = Performer { grid: &mut self.grid, state: &mut self.state };
            self.parser.advance(&mut performer, bytes);
        }
        match self.state.pending_alt.take() {
            Some(true) if self.saved.is_none() => {
                let (rows, cols) = self.grid.dims();
                let primary = std::mem::replace(&mut self.grid, Grid::new(rows, cols));
                self.saved = Some(primary);
            }
            Some(false) => {
                if let Some(primary) = self.saved.take() {
                    self.grid = primary;
                }
            }
            _ => {}
        }
    }
```

- [ ] **Step 4: 跑测试，确认通过**

```bash
cargo test -p alephcore --lib gateway::pty::screen::
```
Expected: 15 passed。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/pty/screen/
git commit -m "pty: alternate screen buffer and resize

Reflow on resize is deliberately not attempted: a wrong reflow scrambles a
screen the user is reading, while clipping is legible and self-corrects on
the application's next repaint, which every full-screen app does on SIGWINCH."
```

---

## Task 6: 脏行跟踪 → `ScreenPatch`（含 run 折叠）

**Files:**
- Create: `src/gateway/pty/screen/diff.rs`
- Modify: `src/gateway/pty/screen/grid.rs`, `src/gateway/pty/screen/perform.rs`, `src/gateway/pty/screen/mod.rs`

**Interfaces:**
- Consumes: `Grid`, `Screen`
- Produces:
  - `pub struct StyleRun { pub text: String, pub fg: Color, pub bg: Color, pub attrs: Attrs }`
  - `pub struct RowPatch { pub row: u16, pub runs: Vec<StyleRun> }`
  - `pub struct ScreenPatch { pub rows: Vec<RowPatch>, pub cursor: Option<(u16,u16)>, pub alt_screen: Option<bool>, pub title: Option<String>, pub bell: bool }`
  - `ScreenPatch::is_empty(&self) -> bool`
  - `Screen::take_patch(&mut self) -> Option<ScreenPatch>`
  - `Screen::full_patch(&self) -> ScreenPatch`

- [ ] **Step 1: 写失败的测试**

Create `src/gateway/pty/screen/diff.rs` with:

```rust
#[cfg(test)]
mod tests {
    use crate::gateway::pty::screen::grid::{Attrs, Color};
    use crate::gateway::pty::screen::Screen;

    #[test]
    fn only_dirty_rows_are_emitted() {
        let mut s = Screen::new(4, 20);
        s.feed(b"a\r\nb\r\n");
        let p = s.take_patch().expect("first write is dirty");
        let rows: Vec<u16> = p.rows.iter().map(|r| r.row).collect();
        assert_eq!(rows, vec![0, 1], "untouched rows 2 and 3 must not ship");
    }

    /// The whole point of the 16 ms cadence: a quiet terminal costs nothing.
    #[test]
    fn a_second_take_with_no_writes_is_none() {
        let mut s = Screen::new(4, 20);
        s.feed(b"a");
        let _ = s.take_patch();
        assert!(s.take_patch().is_none(), "a quiet screen must produce no frame");
    }

    #[test]
    fn same_style_cells_collapse_into_one_run() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b[31mRED\x1b[0mplain");
        let p = s.take_patch().expect("dirty");
        let runs = &p.rows[0].runs;
        assert_eq!(runs.len(), 2, "two styles, two runs");
        assert_eq!(runs[0].text, "RED");
        assert_eq!(runs[0].fg, Color::Indexed(1));
        assert_eq!(runs[1].text.trim_end(), "plain");
        assert_eq!(runs[1].fg, Color::Default);
        assert_eq!(runs[1].attrs, Attrs::NONE);
    }

    #[test]
    fn a_title_change_rides_along_and_is_reported_once() {
        let mut s = Screen::new(2, 20);
        s.feed(b"\x1b]0;t1\x07x");
        assert_eq!(s.take_patch().and_then(|p| p.title), Some("t1".to_string()));
        s.feed(b"y");
        assert_eq!(s.take_patch().and_then(|p| p.title), None, "an unchanged title must not reship");
    }

    /// A full snapshot is what `pty.attach` hands a fresh client, so it must
    /// carry every row — including the ones no write has touched.
    #[test]
    fn a_full_patch_carries_every_row() {
        let mut s = Screen::new(4, 20);
        s.feed(b"only-row-0");
        let full = s.full_patch();
        assert_eq!(full.rows.len(), 4);
        assert_eq!(full.cursor, Some(s.grid.cursor()));
    }
}
```

- [ ] **Step 2: 跑它，确认失败**

```bash
cargo test -p alephcore --lib gateway::pty::screen::diff
```
Expected: FAIL —— `no method named take_patch`。

- [ ] **Step 3: 实现**

在 `grid.rs` 的 `Grid` 加脏集，并在每个写入点标脏：

```rust
    // 字段（加到 struct Grid）
    dirty: std::collections::BTreeSet<u16>,
```

`Grid::new` 里 `dirty: (0..rows).collect()`（新屏整屏是脏的 —— 一块从没发过的屏对客户端就是全新的）。

在 `put` / `newline`（`scroll_up` 后整屏脏）/ `erase_in_display` / `erase_in_line` / `goto` 之外的**所有改变单元格的地方**调 `self.dirty.insert(row)`；`scroll_up` 与 `resize` 用 `self.dirty.extend(0..self.rows)`。加两个方法：

```rust
    pub(crate) fn take_dirty(&mut self) -> std::collections::BTreeSet<u16> {
        std::mem::take(&mut self.dirty)
    }

    pub(crate) fn mark_all_dirty(&mut self) {
        self.dirty.extend(0..self.rows);
    }
```

`diff.rs` 顶部（`mod tests` 之上）：

```rust
//! Dirty-row tracking and the wire patch.
//!
//! Rows are re-sent whole rather than cell-by-cell. Cell-level diffs save
//! bandwidth but buy a whole class of "one cell never updated" bugs, and a row
//! is only ~200 cells. Whole-row re-send has the property that every frame is
//! self-healing.

use super::grid::{Attrs, Cell, Color, Grid};

/// A run of consecutive cells sharing one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleRun {
    pub text: String,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowPatch {
    pub row: u16,
    pub runs: Vec<StyleRun>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenPatch {
    pub rows: Vec<RowPatch>,
    pub cursor: Option<(u16, u16)>,
    pub alt_screen: Option<bool>,
    pub title: Option<String>,
    pub bell: bool,
}

impl ScreenPatch {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
            && self.cursor.is_none()
            && self.alt_screen.is_none()
            && self.title.is_none()
            && !self.bell
    }
}

/// Fold one row's cells into style runs. Spacer cells (the right half of a
/// wide glyph) carry no character and are dropped: the client re-derives the
/// width from the glyph itself.
pub(crate) fn row_runs(cells: &[Cell]) -> Vec<StyleRun> {
    let mut runs: Vec<StyleRun> = Vec::new();
    for cell in cells {
        if cell.is_spacer() {
            continue;
        }
        match runs.last_mut() {
            Some(last) if last.fg == cell.fg && last.bg == cell.bg && last.attrs == cell.attrs => {
                last.text.push(cell.ch);
            }
            _ => runs.push(StyleRun {
                text: cell.ch.to_string(),
                fg: cell.fg,
                bg: cell.bg,
                attrs: cell.attrs,
            }),
        }
    }
    runs
}

pub(crate) fn patch_rows(grid: &Grid, rows: impl IntoIterator<Item = u16>) -> Vec<RowPatch> {
    rows.into_iter()
        .map(|row| RowPatch { row, runs: row_runs(grid.row_cells(row)) })
        .collect()
}
```

`perform.rs` 的 `Screen` 加两个方法 + 一个"上次发出去的标题"字段（`last_sent_title: Option<String>`，`Screen::new` 里 `None`）：

```rust
    /// The diff since the last call, or `None` when nothing changed. `None` is
    /// what makes a quiet terminal free: the flush task publishes nothing.
    pub fn take_patch(&mut self) -> Option<super::diff::ScreenPatch> {
        let dirty = self.grid.take_dirty();
        let title_changed = self.state.title != self.last_sent_title;
        let alt = self.alt_screen();
        let alt_changed = Some(alt) != self.last_sent_alt;
        let bell = self.take_bell();

        let patch = super::diff::ScreenPatch {
            rows: super::diff::patch_rows(&self.grid, dirty),
            cursor: Some(self.grid.cursor()),
            alt_screen: alt_changed.then_some(alt),
            title: title_changed.then(|| self.state.title.clone()).flatten(),
            bell,
        };
        // Cursor is always present above, so emptiness is decided on the
        // fields that actually carry news.
        if patch.rows.is_empty() && !title_changed && !alt_changed && !bell {
            return None;
        }
        self.last_sent_title.clone_from(&self.state.title);
        self.last_sent_alt = Some(alt);
        Some(patch)
    }

    /// Every row, for `pty.attach`. Does not consume the dirty set — an
    /// attach must not swallow a diff a live client is still waiting for.
    #[must_use]
    pub fn full_patch(&self) -> super::diff::ScreenPatch {
        let (rows, _) = self.grid.dims();
        super::diff::ScreenPatch {
            rows: super::diff::patch_rows(&self.grid, 0..rows),
            cursor: Some(self.grid.cursor()),
            alt_screen: Some(self.alt_screen()),
            title: self.state.title.clone(),
            bell: false,
        }
    }
```

同时加 `last_sent_alt: Option<bool>` 字段（`new` 里 `None`），并让 `Screen::resize` 调 `self.grid.mark_all_dirty()`。

`mod.rs` 加：

```rust
pub mod diff;
pub use diff::{RowPatch, ScreenPatch, StyleRun};
```

- [ ] **Step 4: 跑测试，确认通过**

```bash
cargo test -p alephcore --lib gateway::pty::screen::
```
Expected: 20 passed。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/pty/screen/
git commit -m "pty: dirty-row tracking and the wire patch

Rows ship whole rather than cell-by-cell: cell diffs save bandwidth but buy
a class of 'one cell never updated' bugs, and whole-row re-send is
self-healing. take_patch returns None on a quiet screen, which is what makes
the flush cadence cost nothing when nothing is happening."
```

---

## Task 7: wire 契约 `aleph_protocol::pty`

**Files:**
- Create: `shared/protocol/src/pty.rs`
- Modify: `shared/protocol/src/lib.rs`

**Interfaces:**
- Consumes: 无（协议 crate 不依赖 alephcore）
- Produces（core 与 Panel 双方都 `use` 这些）：
  - `PtyColor`, `PtyAttrs`, `PtyStyleRun`, `PtyRowPatch`, `PtyScreenPatch`
  - `PtyScreenFrame { session_id: String, seq: u64, patch: PtyScreenPatch }`
  - `PtyAttachResponse { seq: u64, rows: u16, cols: u16, patch: PtyScreenPatch, scrollback_len: u32 }`
  - `PtySpawnResponse { session_id: String, shell: String, seq: u64, rows: u16, cols: u16 }`
  - `PTY_SCREEN_TOPIC: &str = "pty.screen"`

- [ ] **Step 1: 写失败的测试**

Create `shared/protocol/src/pty.rs` with only the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// The wire key set is the contract. A superset assertion would pass while
    /// the server over-sends, because serde ignores unknown keys — so this
    /// asserts equality, and derives the expectation from the type itself.
    #[test]
    fn screen_frame_wire_keys_are_exactly_these() {
        let frame = PtyScreenFrame {
            session_id: "s".into(),
            seq: 1,
            patch: PtyScreenPatch::default(),
        };
        let v = serde_json::to_value(&frame).expect("serialisable");
        let keys: std::collections::BTreeSet<&str> =
            v.as_object().expect("object").keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["patch", "seq", "session_id"].into_iter().collect::<std::collections::BTreeSet<_>>()
        );
    }

    /// Absent optional fields must not occupy wire bytes: a quiet frame is
    /// published every 16 ms per active session.
    #[test]
    fn absent_optionals_are_omitted_from_the_wire() {
        let patch = PtyScreenPatch::default();
        let v = serde_json::to_value(&patch).expect("serialisable");
        let obj = v.as_object().expect("object");
        assert!(!obj.contains_key("cursor"));
        assert!(!obj.contains_key("title"));
        assert!(!obj.contains_key("alt_screen"));
        assert!(!obj.contains_key("bell"), "false bell must not ship");
    }

    #[test]
    fn colour_round_trips_through_all_three_forms() {
        for c in [PtyColor::Default, PtyColor::Indexed(9), PtyColor::Rgb(1, 2, 3)] {
            let s = serde_json::to_string(&c).expect("ser");
            let back: PtyColor = serde_json::from_str(&s).expect("de");
            assert_eq!(c, back, "colour must survive the wire: {s}");
        }
    }
}
```

- [ ] **Step 2: 跑它，确认失败**

```bash
cargo test -p aleph_protocol pty::
```
Expected: FAIL（类型不存在）。若 crate 名不是 `aleph_protocol`，用 `sed -n '1,10p' shared/protocol/Cargo.toml` 确认后替换。

- [ ] **Step 3: 实现**

`shared/protocol/src/pty.rs`（放在 `mod tests` 之上）：

```rust
//! Wire contract for the embedded terminal.
//!
//! Both halves of this contract live in one crate on purpose: the server
//! builds its responses *from* these types rather than from `json!` literals,
//! so over-sending a field is a compile-time impossibility rather than
//! something a parse-only reconciliation test would structurally miss.

use serde::{Deserialize, Serialize};

/// The topic live screen diffs are published on. Named here so the server
/// publisher and the Panel subscriber cannot drift.
pub const PTY_SCREEN_TOPIC: &str = "pty.screen";

/// The topic a session's exit is published on.
pub const PTY_EXIT_TOPIC: &str = "pty.exit";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum PtyColor {
    #[default]
    Default,
    Indexed { n: u8 },
    Rgb { r: u8, g: u8, b: u8 },
}

impl PtyColor {
    #[must_use]
    pub const fn indexed(n: u8) -> Self {
        Self::Indexed { n }
    }

    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::Rgb { r, g, b }
    }
}

/// SGR attribute bits. One byte on the wire, matching the server's `Attrs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct PtyAttrs(pub u8);

impl PtyAttrs {
    pub const BOLD: u8 = 1 << 0;
    pub const ITALIC: u8 = 1 << 1;
    pub const UNDERLINE: u8 = 1 << 2;
    pub const REVERSE: u8 = 1 << 3;

    #[must_use]
    pub const fn has(self, bit: u8) -> bool {
        self.0 & bit == bit
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyStyleRun {
    pub text: String,
    #[serde(default, skip_serializing_if = "is_default_colour")]
    pub fg: PtyColor,
    #[serde(default, skip_serializing_if = "is_default_colour")]
    pub bg: PtyColor,
    #[serde(default, skip_serializing_if = "is_no_attrs")]
    pub attrs: PtyAttrs,
}

fn is_default_colour(c: &PtyColor) -> bool {
    matches!(c, PtyColor::Default)
}

fn is_no_attrs(a: &PtyAttrs) -> bool {
    a.0 == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyRowPatch {
    pub row: u16,
    pub runs: Vec<PtyStyleRun>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyScreenPatch {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<PtyRowPatch>,
    /// `(row, col)`, zero-based.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<(u16, u16)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt_screen: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub bell: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One live frame. `seq` is per-session and monotonic; a client that receives
/// `seq != last + 1` has missed a frame (the gateway event bus is a bounded
/// broadcast that drops for lagging subscribers) and must re-attach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyScreenFrame {
    pub session_id: String,
    pub seq: u64,
    pub patch: PtyScreenPatch,
}

/// `pty.attach` — one snapshot. Split across two calls this would open a
/// window where a client holds a screen and a different cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyAttachResponse {
    pub seq: u64,
    pub rows: u16,
    pub cols: u16,
    pub patch: PtyScreenPatch,
    pub scrollback_len: u32,
}

/// `pty.spawn`. Carries `seq` so there is no window between the spawn
/// response and the first frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtySpawnResponse {
    pub session_id: String,
    pub shell: String,
    pub seq: u64,
    pub rows: u16,
    pub cols: u16,
}
```

`shared/protocol/src/lib.rs` 按字母序加（在 `pub mod providers;` 之前）：

```rust
pub mod pty;
```

- [ ] **Step 4: 跑测试，确认通过**

```bash
cargo test -p aleph_protocol pty::
```
Expected: 3 passed。

- [ ] **Step 5: 提交**

```bash
git add shared/protocol/
git commit -m "protocol: add the embedded-terminal wire contract

The server will build responses from these types rather than from json!
literals: a parse-only reconciliation test is structurally blind to
over-sending, because serde ignores unknown keys."
```

---

## Task 8: 把 screen 接进 `PtySession`，删 `pty.output`

**Files:**
- Modify: `src/gateway/pty/session.rs`, `src/gateway/pty/manager.rs`, `src/gateway/event_scope.rs`
- Create: `src/gateway/pty/screen/convert.rs`

**Interfaces:**
- Consumes: `Screen`（Task 3–6）、`aleph_protocol::pty`（Task 7）
- Produces:
  - `PtySession::feed_and_take_frame(&self) -> Option<PtyScreenFrame>`
  - `PtySession::attach_snapshot(&self) -> PtyAttachResponse`
  - `PtySession::resize` 同步 screen

- [ ] **Step 1: 写失败的测试**

Add to `src/gateway/pty/session.rs` 的 `#[cfg(test)] mod tests`（若没有则新建）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end through a real PTY: bytes a child writes must reach the
    /// server's screen, and the snapshot must show them.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_child_write_reaches_the_server_held_screen() {
        let opts = SpawnOptions {
            command: Some(if cfg!(windows) { "cmd.exe" } else { "sh" }.to_string()),
            args: if cfg!(windows) {
                vec!["/C".into(), "echo ALEPH_SCREEN_OK".into()]
            } else {
                vec!["-c".into(), "printf 'ALEPH_SCREEN_OK'".into()]
            },
            rows: 10,
            cols: 40,
            ..Default::default()
        };
        let session = PtySession::spawn("t-screen".into(), &opts, None).expect("spawn");

        // The reader thread feeds the screen; poll the snapshot rather than
        // sleeping a fixed amount, so a slow machine does not flake.
        let mut found = false;
        for _ in 0..100 {
            let snap = session.attach_snapshot();
            if snap.patch.rows.iter().any(|r| {
                r.runs.iter().any(|run| run.text.contains("ALEPH_SCREEN_OK"))
            }) {
                found = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(found, "the child's output must appear on the server-held screen");
        session.kill();
    }

    /// seq must advance only when a frame is actually produced, otherwise a
    /// client's gap detection fires on frames that were never sent.
    #[tokio::test(flavor = "multi_thread")]
    async fn seq_advances_only_when_a_frame_is_produced() {
        let opts = SpawnOptions { rows: 5, cols: 20, ..Default::default() };
        let session = PtySession::spawn("t-seq".into(), &opts, None).expect("spawn");
        // Drain whatever the shell printed at startup.
        for _ in 0..20 {
            if session.feed_and_take_frame().is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let before = session.attach_snapshot().seq;
        assert!(session.feed_and_take_frame().is_none(), "a quiet screen yields no frame");
        assert_eq!(session.attach_snapshot().seq, before, "a no-op must not burn a seq");
        session.kill();
    }
}
```

- [ ] **Step 2: 跑它，确认失败**

```bash
cargo test -p alephcore --lib gateway::pty::session::tests
```
Expected: FAIL —— `no method named attach_snapshot`。

- [ ] **Step 3: 实现**

Create `src/gateway/pty/screen/convert.rs`：

```rust
//! Server screen types → wire types.
//!
//! The conversion lives here rather than as `From` impls on the protocol
//! types because the protocol crate must not depend on alephcore.

use aleph_protocol::pty::{
    PtyAttrs, PtyColor, PtyRowPatch, PtyScreenPatch, PtyStyleRun,
};

use super::grid::{Attrs, Color};
use super::diff::{RowPatch, ScreenPatch, StyleRun};

#[must_use]
pub fn colour(c: Color) -> PtyColor {
    match c {
        Color::Default => PtyColor::Default,
        Color::Indexed(n) => PtyColor::indexed(n),
        Color::Rgb(r, g, b) => PtyColor::rgb(r, g, b),
    }
}

#[must_use]
pub fn attrs(a: Attrs) -> PtyAttrs {
    PtyAttrs(a.0)
}

#[must_use]
pub fn run(r: &StyleRun) -> PtyStyleRun {
    PtyStyleRun {
        text: r.text.clone(),
        fg: colour(r.fg),
        bg: colour(r.bg),
        attrs: attrs(r.attrs),
    }
}

#[must_use]
pub fn patch(p: &ScreenPatch) -> PtyScreenPatch {
    PtyScreenPatch {
        rows: p
            .rows
            .iter()
            .map(|r| PtyRowPatch { row: r.row, runs: r.runs.iter().map(run).collect() })
            .collect(),
        cursor: p.cursor,
        alt_screen: p.alt_screen,
        title: p.title.clone(),
        bell: p.bell,
    }
}
```

`mod.rs` 加 `pub mod convert;`。

Modify `src/gateway/pty/session.rs`：

1. `PtySession` 加两个字段：

```rust
    /// The server-held screen. Fed by the reader thread, drained by the flush
    /// task. A `Mutex` rather than a channel because both halves want the
    /// latest state, not every intermediate one.
    screen: crate::sync_primitives::Mutex<super::screen::Screen>,
    /// Monotonic per-session frame counter. Advances only when a frame is
    /// actually published, so a client's gap detection means what it says.
    seq: crate::sync_primitives::Mutex<u64>,
```

`Self { .. }` 构造处加 `screen: Mutex::new(super::screen::Screen::new(rows, cols)), seq: Mutex::new(0),`。

2. `spawn_reader` 不再 base64 编码上总线，改为喂 screen：

```rust
                    Ok(n) => {
                        session
                            .screen
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .feed(&buf[..n]);
                    }
```

删掉该分支里原有的 `BASE64.encode` / `TopicEvent::new("pty.output", ..)` / `bus.publish(..)` 三段，以及 `spawn_reader` 里因此不再使用的 `bus` 参数（`pty.exit` 仍要用 bus，所以保留参数，只删 output 那段）。文件顶部若 `BASE64` 因此不再使用则删 `use`。

3. 加三个方法：

```rust
    /// The diff since the last call, already in wire form, with a fresh `seq`.
    /// `None` when nothing changed — that is what makes a quiet terminal free.
    pub fn feed_and_take_frame(&self) -> Option<aleph_protocol::pty::PtyScreenFrame> {
        let patch = {
            let mut screen = self.screen.lock().unwrap_or_else(|e| e.into_inner());
            screen.take_patch()?
        };
        let seq = {
            let mut s = self.seq.lock().unwrap_or_else(|e| e.into_inner());
            *s += 1;
            *s
        };
        Some(aleph_protocol::pty::PtyScreenFrame {
            session_id: self.id.clone(),
            seq,
            patch: super::screen::convert::patch(&patch),
        })
    }

    /// One snapshot for `pty.attach`: the whole screen plus the seq it was
    /// taken at, so the client knows which live frames to discard.
    pub fn attach_snapshot(&self) -> aleph_protocol::pty::PtyAttachResponse {
        let screen = self.screen.lock().unwrap_or_else(|e| e.into_inner());
        let (rows, cols) = screen.grid.dims();
        let seq = *self.seq.lock().unwrap_or_else(|e| e.into_inner());
        aleph_protocol::pty::PtyAttachResponse {
            seq,
            rows,
            cols,
            patch: super::screen::convert::patch(&screen.full_patch()),
            scrollback_len: screen.grid.scrollback_len(),
        }
    }
```

`Grid` 加 `pub fn scrollback_len(&self) -> u32 { self.scrollback.len() as u32 }`。

4. `PtySession::resize` 在转发给内核之后同步 screen：

```rust
        self.screen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .resize(rows.max(1), cols.max(1));
```

5. `src/gateway/event_scope.rs`：把 `"pty.output"` 改成 `aleph_protocol::pty::PTY_SCREEN_TOPIC` 的字面值 `"pty.screen"`。**改动点有两处以上**（`default_rules` 与其孪生 pin 测试、`handlers/users.rs:1269`、`server/handler.rs:2402`）——用 `rg -n '"pty\.output"'` 找齐全部，一个不留。

- [ ] **Step 4: 跑测试，确认通过**

```bash
rg -n '"pty\.output"'          # 必须零命中
cargo test -p alephcore --lib gateway::pty::
```
Expected: `rg` 无输出；测试全 passed。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/pty/ src/gateway/event_scope.rs src/gateway/handlers/users.rs src/gateway/server/handler.rs
git commit -m "pty: hold the screen server-side and retire the raw byte topic

pty.output published one base64 event per 8 KiB read onto a bounded
broadcast that drops for lagging subscribers, so a client that fell behind
was permanently garbled with no error anywhere. The screen now lives on the
server and what goes on the wire is a bounded per-frame diff."
```

---

## Task 9: flush task + `pty.screen` 发布 + `pty.attach` handler

**Files:**
- Modify: `src/gateway/pty/manager.rs`, `src/gateway/handlers/pty.rs`, `src/gateway/handlers/mod.rs`, `src/gateway/method_census.rs`

**Interfaces:**
- Consumes: `PtySession::{feed_and_take_frame, attach_snapshot}`（Task 8）
- Produces:
  - `pty::manager().start_flush_loop()`（进程内单例，幂等）
  - RPC `pty.attach`
  - `pty.spawn` 响应换成 `PtySpawnResponse`

- [ ] **Step 1: 写失败的测试**

Add to `src/gateway/handlers/pty.rs` 的 `mod tests`：

```rust
    #[tokio::test]
    async fn attach_returns_a_snapshot_with_its_seq() {
        let spawn = handle_spawn(req("pty.spawn", json!({ "rows": 8, "cols": 30 }))).await;
        let sid = spawn.result.as_ref().expect("spawned")["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        let resp = handle_attach(req("pty.attach", json!({ "session_id": sid }))).await;
        let value = resp.result.expect("attach must succeed");
        let parsed: aleph_protocol::pty::PtyAttachResponse =
            serde_json::from_value(value.clone()).expect("attach response must match the contract");
        assert_eq!(parsed.rows, 8);
        assert_eq!(parsed.cols, 30);
        assert_eq!(parsed.patch.rows.len(), 8, "a snapshot carries every row");

        // The contract is the key set, not a subset: a parse-only assertion
        // is blind to over-sending because serde ignores unknown keys.
        let keys: std::collections::BTreeSet<&str> =
            value.as_object().expect("object").keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["cols", "patch", "rows", "scrollback_len", "seq"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );

        let _ = handle_close(req("pty.close", json!({ "session_id": sid }))).await;
    }

    #[tokio::test]
    async fn attach_on_an_unknown_session_is_an_error_not_an_empty_screen() {
        let resp = handle_attach(req("pty.attach", json!({ "session_id": "ghost" }))).await;
        assert!(resp.result.is_none(), "an unknown session must not read as a blank screen");
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    async fn spawn_response_matches_the_contract_key_for_key() {
        let resp = handle_spawn(req("pty.spawn", json!({ "rows": 4, "cols": 12 }))).await;
        let value = resp.result.expect("spawned");
        let keys: std::collections::BTreeSet<&str> =
            value.as_object().expect("object").keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["cols", "rows", "seq", "session_id", "shell"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );
        let sid = value["session_id"].as_str().expect("id").to_string();
        let _ = handle_close(req("pty.close", json!({ "session_id": sid }))).await;
    }
```

若该文件的 `mod tests` 里没有 `req` 助手，照既有 `handle_list` 测试的写法补一个：

```rust
    fn req(method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: method.into(),
            params: Some(params),
        }
    }
```
（字段名以 `src/gateway/protocol.rs` 里 `JsonRpcRequest` 的实际定义为准 —— 先 `grep -n 'pub struct JsonRpcRequest' -A 10 src/gateway/protocol.rs`。）

- [ ] **Step 2: 跑它，确认失败**

```bash
cargo test -p alephcore --lib gateway::handlers::pty
```
Expected: FAIL —— `cannot find function handle_attach`。

- [ ] **Step 3: 实现**

`src/gateway/pty/manager.rs` 加 flush 循环：

```rust
/// Publish cadence. 16 ms ≈ 60 Hz: fast enough that no human sees the delay,
/// slow enough that a process writing megabytes per second still costs one
/// bounded frame per tick. This coalescing *is* the backpressure design.
const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);

impl PtyManager {
    /// Start the process-global flush loop. Idempotent — safe to call from
    /// every gateway boot path.
    pub fn start_flush_loop(&'static self) {
        static STARTED: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(FLUSH_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let Some(bus) = self.current_bus() else { continue };
                let sessions: Vec<Arc<PtySession>> = {
                    let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                    inner.sessions.values().cloned().collect()
                };
                for session in sessions {
                    let Some(frame) = session.feed_and_take_frame() else { continue };
                    let Ok(data) = serde_json::to_value(&frame) else { continue };
                    let ev = crate::gateway::event_bus::TopicEvent::new(
                        aleph_protocol::pty::PTY_SCREEN_TOPIC,
                        data,
                    );
                    let _ = bus.publish(serde_json::to_string(&ev).unwrap_or_default());
                }
            }
        });
    }
}
```

`attach_event_bus` 之后调用它 —— 改 `src/gateway/pty/mod.rs` 的自由函数：

```rust
pub fn attach_event_bus(bus: Arc<GatewayEventBus>) {
    manager().attach_event_bus(bus);
    manager().start_flush_loop();
}
```

`src/gateway/handlers/pty.rs`：

```rust
#[derive(Debug, Deserialize)]
pub struct AttachParams {
    pub session_id: String,
}

/// `pty.attach` — one snapshot of a session's screen plus the seq it was
/// taken at. One call, not two: split across two round trips this opens a
/// window where the client holds a screen and a different cursor.
pub async fn handle_attach(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    let params: AttachParams = match parse(&request) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    match pty::manager().attach_snapshot(&params.session_id) {
        Ok(snapshot) => match serde_json::to_value(&snapshot) {
            Ok(v) => JsonRpcResponse::success(id, v),
            Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, format!("encode failed: {e}")),
        },
        Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, e),
    }
}
```

`PtyManager` 加：

```rust
    /// Snapshot a session's screen. An unknown session is an error, never an
    /// empty screen — a blank grid would read as "the terminal is idle".
    pub fn attach_snapshot(
        &self,
        session_id: &str,
    ) -> Result<aleph_protocol::pty::PtyAttachResponse, String> {
        self.with_session(session_id, |s| Ok(s.attach_snapshot()))
    }
```

`handle_spawn` 改成用契约类型构造响应：

```rust
    match pty::manager().spawn(&opts) {
        Ok(res) => {
            let snapshot = pty::manager()
                .attach_snapshot(&res.session_id)
                .unwrap_or_else(|_| aleph_protocol::pty::PtyAttachResponse {
                    seq: 0,
                    rows: if params.rows == 0 { 24 } else { params.rows },
                    cols: if params.cols == 0 { 80 } else { params.cols },
                    patch: aleph_protocol::pty::PtyScreenPatch::default(),
                    scrollback_len: 0,
                });
            let body = aleph_protocol::pty::PtySpawnResponse {
                session_id: res.session_id,
                shell: res.shell,
                seq: snapshot.seq,
                rows: snapshot.rows,
                cols: snapshot.cols,
            };
            match serde_json::to_value(&body) {
                Ok(v) => JsonRpcResponse::success(id, v),
                Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, format!("encode failed: {e}")),
            }
        }
        Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, e),
    }
```

注册 handler —— `src/gateway/handlers/mod.rs` 在 `registry.register("pty.list", ..)` 之后加：

```rust
        registry.register("pty.attach", pty::handle_attach);
```

`src/gateway/method_census.rs` 的表里加（保持字母序，与既有 `("pty.close", Class::Admin)` 同族）：

```rust
        ("pty.attach", Class::Admin),
```

- [ ] **Step 4: 跑测试，确认通过**

```bash
cargo test -p alephcore --lib gateway::handlers::pty
cargo test -p alephcore --lib gateway::method_census
cargo test -p alephcore --lib gateway::event_scope
```
Expected: 全 passed。`method_census` 与 `event_scope` 里有"每个注册方法都必须在册"的守卫，漏登记会在这里红。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/
git commit -m "pty: publish screen diffs on a 16ms cadence and add pty.attach

The cadence is the backpressure design: a process writing megabytes per
second still costs one bounded frame per tick. attach is one call rather
than two because splitting it opens a window where a client holds a screen
and a cursor from different moments."
```

---

## Task 10: attach 表 + min-size 共享 resize

**Files:**
- Modify: `src/gateway/pty/manager.rs`, `src/gateway/handlers/pty.rs`

**Interfaces:**
- Consumes: `PtyManager`
- Produces:
  - `PtyManager::note_viewport(&self, session_id: &str, conn_id: &str, rows: u16, cols: u16)`
  - `PtyManager::release_conn(&self, conn_id: &str)`
  - `SessionInfo` 增 `attached_count: usize`

- [ ] **Step 1: 写失败的测试**

Add to `src/gateway/pty/manager.rs` 的 `mod tests`：

```rust
    /// Two clients with different viewports share one PTY, which has exactly
    /// one size. The smallest wins (tmux's convention for shared sessions):
    /// deterministic, and it never thrashes between two live clients.
    #[test]
    fn the_smallest_attached_viewport_wins() {
        let mgr = PtyManager::new();
        let res = mgr
            .spawn(&SpawnOptions { rows: 40, cols: 120, ..Default::default() })
            .expect("spawn");
        let sid = res.session_id;

        mgr.note_viewport(&sid, "conn-a", 40, 120);
        mgr.note_viewport(&sid, "conn-b", 24, 80);
        assert_eq!(mgr.effective_size(&sid), Some((24, 80)));

        // The constraint must be released when its client goes away —
        // otherwise a crashed tab pins every other client to its size.
        mgr.release_conn("conn-b");
        assert_eq!(mgr.effective_size(&sid), Some((40, 120)));

        mgr.close(&sid).expect("close");
    }

    #[test]
    fn attached_count_is_visible_for_diagnosis() {
        let mgr = PtyManager::new();
        let sid = mgr.spawn(&SpawnOptions::default()).expect("spawn").session_id;
        mgr.note_viewport(&sid, "conn-a", 24, 80);
        mgr.note_viewport(&sid, "conn-b", 24, 80);
        assert_eq!(mgr.list()[0].attached_count, 2);
        mgr.close(&sid).expect("close");
    }
```

- [ ] **Step 2: 跑它，确认失败**

```bash
cargo test -p alephcore --lib gateway::pty::manager
```
Expected: FAIL。

- [ ] **Step 3: 实现**

`manager.rs`：

```rust
// 加到 struct Inner
    /// `session_id -> (conn_id -> viewport)`. Present because a server-held
    /// screen makes multi-client sharing free, and the moment a second client
    /// attaches, something has to decide the one size the PTY gets.
    viewports: HashMap<String, HashMap<String, (u16, u16)>>,
```

`SessionInfo` 加 `pub attached_count: usize`，`list()` 里填 `inner.viewports.get(id).map_or(0, HashMap::len)`。

```rust
impl PtyManager {
    /// Record a client's viewport and re-apply the smallest one.
    pub fn note_viewport(&self, session_id: &str, conn_id: &str, rows: u16, cols: u16) {
        {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner
                .viewports
                .entry(session_id.to_string())
                .or_default()
                .insert(conn_id.to_string(), (rows.max(1), cols.max(1)));
        }
        self.apply_effective_size(session_id);
    }

    /// Drop every viewport constraint held by a departing connection.
    pub fn release_conn(&self, conn_id: &str) {
        let touched: Vec<String> = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let mut touched = Vec::new();
            for (sid, map) in &mut inner.viewports {
                if map.remove(conn_id).is_some() {
                    touched.push(sid.clone());
                }
            }
            inner.viewports.retain(|_, m| !m.is_empty());
            touched
        };
        for sid in touched {
            self.apply_effective_size(&sid);
        }
    }

    /// The size every attached client can display: the per-axis minimum.
    #[must_use]
    pub fn effective_size(&self, session_id: &str) -> Option<(u16, u16)> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let map = inner.viewports.get(session_id)?;
        map.values()
            .copied()
            .reduce(|(ar, ac), (br, bc)| (ar.min(br), ac.min(bc)))
    }

    fn apply_effective_size(&self, session_id: &str) {
        let Some((rows, cols)) = self.effective_size(session_id) else { return };
        let _ = self.resize(session_id, rows, cols);
    }
}
```

`close` / `remove` 里同时 `inner.viewports.remove(session_id);`。

`handlers/pty.rs` 的 `handle_resize` 改为记录视口而非直接 resize —— 需要 `conn_id`。**这里有一个前置未知**：`JsonRpcRequest` 是否携带连接标识。先查：

```bash
grep -n 'conn_id\|connection_id' src/gateway/protocol.rs src/gateway/server/handler.rs | head
```

- 若 `JsonRpcRequest` 已有连接标识字段，直接用。
- 若没有，**本 Task 的 handler 改造降级为**：`ResizeParams` 增一个必填的 `client_id: String`（由 Panel 生成的 uuid，随每次 `pty.attach`/`pty.resize` 带上），`release_conn` 由 `pty.close` 与会话断开时的既有清理路径调用。**在提交信息里写明这是降级路径以及为什么**，并把"绑真正的 WS 连接生命周期"记进 Part 2 的待办。

- [ ] **Step 4: 跑测试，确认通过**

```bash
cargo test -p alephcore --lib gateway::pty::
```
Expected: 全 passed。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/pty/ src/gateway/handlers/pty.rs
git commit -m "pty: per-session viewport table with smallest-wins sizing

A server-held screen makes multi-client sharing free, and the moment a
second client attaches something must decide the one size the PTY gets.
Smallest-wins is tmux's convention: deterministic, and it never thrashes
between two live clients. A departing client releases its constraint,
otherwise a crashed tab pins everyone else to its size."
```

---

## Task 11: cwd jail

**Files:**
- Create: `src/gateway/pty/jail.rs`
- Modify: `src/gateway/pty/mod.rs`, `src/gateway/handlers/pty.rs`

**Interfaces:**
- Consumes: `AgentEnvStore`（既有），`sandbox::config::SandboxConfig::workspace_root`
- Produces:
  - `pub fn resolve_spawn_cwd(requested: Option<&str>, roots: &[PathBuf]) -> Result<PathBuf, String>`

- [ ] **Step 1: 写失败的测试**

Create `src/gateway/pty/jail.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn roots(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        vec![dir.to_path_buf()]
    }

    #[test]
    fn a_path_inside_a_registered_root_is_allowed() {
        let tmp = tempfile::tempdir().expect("tmp");
        let sub = tmp.path().join("proj");
        std::fs::create_dir_all(&sub).expect("mkdir");
        let got = resolve_spawn_cwd(Some(sub.to_str().expect("utf8")), &roots(tmp.path()))
            .expect("inside a root must be allowed");
        assert!(got.ends_with("proj"));
    }

    #[test]
    fn a_path_outside_every_root_is_refused() {
        let tmp = tempfile::tempdir().expect("tmp");
        let other = tempfile::tempdir().expect("tmp2");
        let err = resolve_spawn_cwd(Some(other.path().to_str().expect("utf8")), &roots(tmp.path()))
            .expect_err("outside every root must be refused");
        assert!(err.contains("outside"), "the refusal must say why: {err}");
    }

    /// The classic escape: a path that lexically looks inside but resolves out.
    #[test]
    fn dot_dot_traversal_is_refused_after_canonicalisation() {
        let tmp = tempfile::tempdir().expect("tmp");
        let sub = tmp.path().join("proj");
        std::fs::create_dir_all(&sub).expect("mkdir");
        let sneaky = sub.join("..").join("..");
        assert!(
            resolve_spawn_cwd(sneaky.to_str(), &roots(tmp.path())).is_err(),
            "traversal must be judged on the canonical form, not the literal one"
        );
    }

    /// Omitting cwd must not fall back to the daemon's process cwd: that
    /// answers a different question ("where was the server started") and
    /// would be a lie dressed as a default.
    #[test]
    fn an_omitted_cwd_falls_back_to_the_first_root_not_the_process_cwd() {
        let tmp = tempfile::tempdir().expect("tmp");
        let got = resolve_spawn_cwd(None, &roots(tmp.path())).expect("must resolve");
        let expected = std::fs::canonicalize(tmp.path()).expect("canonical");
        assert_eq!(got, expected);
        assert_ne!(
            got,
            std::env::current_dir().expect("cwd"),
            "the daemon's cwd is never the answer"
        );
    }

    /// With nothing registered the refusal must name the remedy, not pick a
    /// directory on the user's behalf.
    #[test]
    fn no_registered_roots_refuses_loudly_and_names_the_remedy() {
        let err = resolve_spawn_cwd(None, &[]).expect_err("must refuse");
        assert!(err.contains("workspace"), "the refusal must name what to do: {err}");
    }
}
```

- [ ] **Step 2: 跑它，确认失败**

```bash
cargo test -p alephcore --lib gateway::pty::jail
```
Expected: FAIL。

- [ ] **Step 3: 实现**

`src/gateway/pty/jail.rs`（`mod tests` 之上）：

```rust
//! Working-directory jail for `pty.spawn`.
//!
//! The client's `cwd` is a *request*, not an authorisation: the gate resolves
//! it against the operator-registered workspaces (`workspace.list`'s table)
//! and refuses anything outside. `EXEC_WORKSPACE` — the equivalent floor for
//! the `bash` tool — is a `tokio::task_local` scoped to an agent run and is
//! structurally unreachable from an RPC handler, which is why the source of
//! truth here is the workspace store instead.
//!
//! This gate covers the *starting* directory only. A `cd` typed inside the
//! terminal is not constrained, because a command-grained gate is not
//! expressible on an interactive byte stream. What it buys is "every terminal
//! starts somewhere enumerable and auditable", not isolation.

use std::path::{Path, PathBuf};

/// Canonicalise both sides through one function. On Windows `canonicalize`
/// yields the `\\?\C:\` verbatim form; converting only one side flips
/// `starts_with` from allow to deny.
fn canonical(p: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(p).map_err(|e| format!("cannot resolve {}: {e}", p.display()))
}

/// Resolve the directory a new PTY may start in.
///
/// * `requested` — the client's ask, or `None` to take the default.
/// * `roots` — the operator-registered workspace roots.
pub fn resolve_spawn_cwd(requested: Option<&str>, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let canonical_roots: Vec<PathBuf> = roots.iter().filter_map(|r| canonical(r).ok()).collect();
    if canonical_roots.is_empty() {
        return Err(
            "no workspace is registered, so there is no directory a terminal may start in — \
             register one first (Panel → Settings → Workspaces, or `aleph workspace create`)"
                .to_string(),
        );
    }

    let Some(requested) = requested.filter(|s| !s.trim().is_empty()) else {
        // Not the daemon's cwd: that answers "where was the server started",
        // which is a different question from "what is this terminal
        // authorised to work in".
        return Ok(canonical_roots[0].clone());
    };

    let asked = canonical(Path::new(requested))?;
    if canonical_roots.iter().any(|root| asked.starts_with(root)) {
        Ok(asked)
    } else {
        Err(format!(
            "cwd {} is outside every registered workspace",
            asked.display()
        ))
    }
}
```

`mod.rs` 加 `pub mod jail;`。

`handlers/pty.rs` 的 `handle_spawn` 在构造 `SpawnOptions` 之前插入：

```rust
    let roots = pty::workspace_roots();
    let cwd = match jail::resolve_spawn_cwd(params.cwd.as_deref(), &roots) {
        Ok(p) => p,
        Err(e) => return JsonRpcResponse::error(id, INVALID_PARAMS, e),
    };
```
并把 `cwd: params.cwd` 换成 `cwd: Some(cwd.to_string_lossy().into_owned())`。

`src/gateway/pty/mod.rs` 加 roots 取值函数 —— **这一步要先读 `AgentEnvStore` 的真实 API**：

```bash
grep -rn 'pub fn list\|pub struct AgentEnv\b\|pub trait AgentEnvStore' --include='*.rs' src/ | head
```

按查到的签名实现：

```rust
/// The operator-registered workspace roots, plus the sandbox workspace root
/// as a floor. Read fresh on every spawn — a boot-time snapshot would let a
/// workspace registered after start-up stay unusable until restart.
#[must_use]
pub fn workspace_roots() -> Vec<std::path::PathBuf> {
    // Fill in from the AgentEnvStore signature found above; include
    // `crate::sandbox::config::SandboxConfig::default().workspace_root` as the
    // last entry so a fresh install is not dead on arrival.
    todo!("replace with the AgentEnvStore call confirmed in this step")
}
```

**这个 `todo!` 不得留到提交** —— Step 3 结束前必须用真实调用替换。之所以在计划里写成这个形状，是因为 `AgentEnvStore` 的取值路径需要在有代码在手时确认，而计划不允许我编造一个签名。

- [ ] **Step 4: 跑测试，确认通过**

```bash
rg -n 'todo!' src/gateway/pty/          # 必须零命中
cargo test -p alephcore --lib gateway::pty::jail
cargo test -p alephcore --lib gateway::handlers::pty
```
Expected: `rg` 无输出；测试全 passed。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/pty/ src/gateway/handlers/pty.rs
git commit -m "pty: jail spawn cwd to the registered workspaces

The client's cwd is a request, not an authorisation. EXEC_WORKSPACE is the
equivalent floor for the bash tool but it is a task_local scoped to an agent
run and structurally unreachable from an RPC handler, so the source of truth
here is the workspace store.

Omitting cwd does not fall back to the daemon's process cwd: that answers
'where was the server started', a different question, and a default that
answers a different question is a lie rather than a default."
```

---

## Task 12: 会话开关 `[gateway.terminal]`

**Files:**
- Modify: `src/config/types/` 下的 gateway 配置类型、`src/config/reload_impact.rs`、`src/config/live_apply.rs`、`src/gateway/handlers/pty.rs`、`src/gateway/pty/manager.rs`

**Interfaces:**
- Consumes: 既有 `Config`
- Produces:
  - `TerminalConfig { enabled: bool, scrollback_lines: u32, max_sessions: usize }`
  - `PtyManager::close_all(&self) -> usize`

- [ ] **Step 1: 写失败的测试**

先定位 gateway 配置类型：

```bash
grep -rn 'pub struct GatewayConfig' -A 25 src/config/types/ | head -40
```

Add tests 到该文件的 `mod tests`：

```rust
    /// Default-on. A default-off switch on a freshly wired feature makes
    /// "nobody used it" and "nobody could" look identical — which is exactly
    /// how pty.* stayed clientless for four rounds.
    #[test]
    fn the_terminal_is_enabled_by_default() {
        assert!(TerminalConfig::default().enabled);
        assert_eq!(TerminalConfig::default().scrollback_lines, 1000);
        assert_eq!(TerminalConfig::default().max_sessions, 64);
    }

    #[test]
    fn the_terminal_section_parses_from_toml() {
        let cfg: TerminalConfig =
            toml::from_str("enabled = false\nscrollback_lines = 200\n").expect("parse");
        assert!(!cfg.enabled);
        assert_eq!(cfg.scrollback_lines, 200);
        assert_eq!(cfg.max_sessions, 64, "unset fields keep their defaults");
    }
```

Add to `src/config/reload_impact.rs` 的 `mod tests`：

```rust
    /// A security switch that only takes effect after a restart is not a
    /// switch. It is declared live, and the declaration is backed by a real
    /// handle (the gate reads the live config at spawn time).
    #[test]
    fn the_terminal_switch_is_declared_live() {
        assert!(
            LIVE_SUBSECTIONS.contains(&"gateway.terminal"),
            "turning the terminal off must not wait for a restart"
        );
    }
```

Add to `src/gateway/pty/manager.rs` 的 `mod tests`：

```rust
    /// A config field with no consumer is indistinguishable from one nobody
    /// sets: it looks settable and never does anything. `scrollback_lines`
    /// must reach the grid that actually bounds the ring.
    #[test]
    fn the_configured_scrollback_reaches_the_session_grid() {
        let mgr = PtyManager::new();
        let sid = mgr
            .spawn_with_scrollback(&SpawnOptions { rows: 3, cols: 10, ..Default::default() }, 7)
            .expect("spawn")
            .session_id;
        assert_eq!(
            mgr.scrollback_limit_of(&sid),
            Some(7),
            "the configured limit must bound the session's ring, not the built-in default"
        );
        mgr.close(&sid).expect("close");
    }

    /// Turning the switch off must kill live sessions, not merely block new
    /// ones: a gate evaluated only at admission leaves the shell that is
    /// already open still open.
    #[test]
    fn close_all_terminates_every_live_session() {
        let mgr = PtyManager::new();
        let a = mgr.spawn(&SpawnOptions::default()).expect("a").session_id;
        let b = mgr.spawn(&SpawnOptions::default()).expect("b").session_id;
        assert_eq!(mgr.list().len(), 2);
        assert_eq!(mgr.close_all(), 2);
        assert!(mgr.list().is_empty());
        assert!(mgr.write(&a, b"x").is_err());
        assert!(mgr.write(&b, b"x").is_err());
    }
```

- [ ] **Step 2: 跑它，确认失败**

```bash
cargo test -p alephcore --lib config::
cargo test -p alephcore --lib gateway::pty::manager
```
Expected: FAIL。

- [ ] **Step 3: 实现**

在 gateway 配置类型文件加：

```rust
/// Embedded terminal settings.
///
/// `enabled` is the session-grained gate. It is default-on because the two
/// floors below it are not optional — operator-only on both the RPC and the
/// subscribe face, and a cwd jail — and because a default-off switch on a
/// freshly wired feature makes "nobody used it" and "nobody could use it"
/// indistinguishable. It is turned off from Panel → Settings → Terminal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    /// Gate ②. Turning this off also kills live sessions.
    pub enabled: bool,
    /// Server-held scrollback per session (see `gateway::pty::screen`).
    pub scrollback_lines: u32,
    /// Concurrent session ceiling; beyond it the oldest is killed FIFO.
    pub max_sessions: usize,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self { enabled: true, scrollback_lines: 1000, max_sessions: 64 }
    }
}
```

`GatewayConfig` 加 `#[serde(default)] pub terminal: TerminalConfig,`。

`reload_impact.rs`：`LIVE_SUBSECTIONS` 加 `"gateway.terminal"`，并在其上方的 doc 里加一段说明（照 `policies.spend` 那段的写法）：

```rust
/// - `gateway.terminal` — the gate is read fresh from the live config on every
///   `pty.spawn`, and turning `enabled` off runs `PtyManager::close_all`, so
///   the change is complete at apply time. `[gateway]`'s other fields (host,
///   port, TLS) need a restart, hence the parent stays out of `LIVE_SECTIONS`.
```

`live_apply.rs::apply_live_sections` 加一臂：把新值应用到进程 —— 关闭时 `close_all`：

```rust
        if *target == "gateway.terminal" {
            if !cfg.gateway.terminal.enabled {
                let killed = crate::gateway::pty::manager().close_all();
                if killed > 0 {
                    tracing::warn!(killed, "terminal disabled; live PTY sessions terminated");
                }
            }
            applied.push("gateway.terminal");
        }
```
（具体写法以该函数既有分支的结构为准。）

`manager.rs`：

```rust
    /// Terminate every live session, returning how many were killed. Used when
    /// the terminal switch is turned off: a gate evaluated only at admission
    /// leaves the shell that is already open still open.
    pub fn close_all(&self) -> usize {
        let sessions: Vec<Arc<PtySession>> = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.order.clear();
            inner.viewports.clear();
            inner.sessions.drain().map(|(_, s)| s).collect()
        };
        let n = sessions.len();
        for s in sessions {
            s.kill();
        }
        n
    }
```

把配置值接到真正约束环的那个字段 —— 否则 `scrollback_lines` 是一个没有消费者的旋钮：

`grid.rs` 加：

```rust
    /// Override the scrollback ceiling. Called at spawn from
    /// `[gateway.terminal] scrollback_lines`; without this the field would be
    /// settable and inert.
    pub fn set_scrollback_limit(&mut self, lines: usize) {
        self.scrollback_limit = lines.max(1);
        while self.scrollback.len() > self.scrollback_limit {
            self.scrollback.pop_front();
        }
    }

    #[must_use]
    pub const fn scrollback_limit(&self) -> usize {
        self.scrollback_limit
    }
```

`perform.rs` 的 `Screen` 转发：

```rust
    pub fn set_scrollback_limit(&mut self, lines: usize) {
        self.grid.set_scrollback_limit(lines);
        if let Some(saved) = &mut self.saved {
            saved.set_scrollback_limit(lines);
        }
    }

    #[must_use]
    pub fn scrollback_limit(&self) -> usize {
        self.grid.scrollback_limit()
    }
```

`session.rs` 的 `PtySession` 加转发（`scrollback_limit()` 供测试读回）；`manager.rs` 加：

```rust
    /// Spawn with an explicit scrollback ceiling. `spawn` delegates here with
    /// the configured value so there is one path, not two.
    pub fn spawn_with_scrollback(
        &self,
        opts: &SpawnOptions,
        scrollback_lines: usize,
    ) -> Result<SpawnResult, String> {
        // Task 13 re-points this at `spawn_as` when the actor is threaded
        // through; until then `spawn` is the only constructor.
        let result = self.spawn(opts)?;
        self.with_session(&result.session_id, |s| {
            s.set_scrollback_limit(scrollback_lines);
            Ok(())
        })?;
        Ok(result)
    }

    #[must_use]
    pub fn scrollback_limit_of(&self, session_id: &str) -> Option<usize> {
        self.with_session(session_id, |s| Ok(s.scrollback_limit())).ok()
    }
```

`handlers/pty.rs::handle_spawn` 改成调 `spawn_with_scrollback(&opts, cfg.gateway.terminal.scrollback_lines as usize)`，并用 `cfg.gateway.terminal.max_sessions` 替换 `manager.rs` 里写死的 `MAX_SESSIONS`（把该常量改成 `PtyManager` 的一个字段，`spawn` 时现读配置传入 —— 与开关同一条"现读不快照"纪律）。

`handlers/pty.rs::handle_spawn` 最前面加闸（**现读，不快照**）：

```rust
    if !crate::config::current().gateway.terminal.enabled {
        return JsonRpcResponse::error(
            id,
            INVALID_PARAMS,
            "the embedded terminal is disabled ([gateway.terminal] enabled = false)".to_string(),
        );
    }
```
（取当前配置的函数名以仓库既有写法为准 —— 先 `grep -rn 'fn current()' src/config/` 确认；若没有进程级句柄，用 `route`/`policies.spend` 同款 `ArcSwap` 模式补一个，并在 doc 里写明它就是 `LIVE_SUBSECTIONS` 声明的那个背书句柄。）

- [ ] **Step 4: 跑测试，确认通过**

```bash
cargo test -p alephcore --lib config::
cargo test -p alephcore --lib gateway::pty::
```
Expected: 全 passed。

- [ ] **Step 5: 提交**

```bash
git add src/config/ src/gateway/
git commit -m "gateway: add the [gateway.terminal] session gate, live and default-on

Declared in LIVE_SUBSECTIONS with a real handle behind it: a security switch
that waits for a restart is not a switch. Turning it off also kills live
sessions, because a gate evaluated only at admission leaves the shell that
is already open still open."
```

---

## Task 13: `self_config` 举卡 + 问责 + SECURITY.md

**Files:**
- Modify: `src/tools/scoped/gate_chain.rs`、`src/gateway/pty/manager.rs`、`src/gateway/handlers/pty.rs`、`docs/reference/SECURITY.md`

**Interfaces:**
- Consumes: `gate_chain::DestructiveArguments`（既有）、`ambient_actor()`（既有）
- Produces: `SessionInfo` 增 `created_by: Option<String>`

- [ ] **Step 1: 写失败的测试**

Add to `src/tools/scoped/gate_chain.rs` 的 `mod tests`：

```rust
    /// A gate whose off-switch can be flipped without a card is not a gate:
    /// two individually legal steps ("write config", "spawn terminal") would
    /// add up to the thing the gate refuses.
    #[test]
    fn writing_the_terminal_switch_trips_the_destructive_argument_filter() {
        let args = serde_json::json!({
            "action": "set",
            "path": "gateway.terminal.enabled",
            "value": true
        });
        assert!(
            arguments_are_destructive("self_config", &args),
            "flipping the terminal gate must raise a card"
        );
    }

    #[test]
    fn an_unrelated_config_write_does_not_trip_it() {
        let args = serde_json::json!({
            "action": "set",
            "path": "behavior.greeting",
            "value": "hi"
        });
        assert!(!arguments_are_destructive("self_config", &args));
    }
```

（函数名 `arguments_are_destructive` 以该文件真实导出的谓词为准 —— 先 `grep -n 'fn tier_asks_for_arguments\|fn arguments_are_destructive\|DestructiveArguments' src/tools/scoped/gate_chain.rs`。）

Add to `src/gateway/pty/manager.rs` 的 `mod tests`：

```rust
    /// Accountability names the person, not just the identity: on a
    /// multi-user install "which operator" is the question an audit asks.
    #[test]
    fn a_spawn_records_who_asked_for_it() {
        let mgr = PtyManager::new();
        let sid = mgr
            .spawn_as(&SpawnOptions::default(), Some("u-alice".to_string()))
            .expect("spawn")
            .session_id;
        assert_eq!(mgr.list()[0].created_by.as_deref(), Some("u-alice"));
        mgr.close(&sid).expect("close");
    }
```

- [ ] **Step 2: 跑它，确认失败**

```bash
cargo test -p alephcore --lib tools::scoped::gate_chain
cargo test -p alephcore --lib gateway::pty::manager
```
Expected: FAIL。

- [ ] **Step 3: 实现**

`gate_chain.rs` 的破坏性参数判据里加 `gateway.terminal` 前缀（与既有 `policies.tool_permissions` 那条同处，**照它的写法**，不新造一个平行表）。

`manager.rs`：`SessionInfo` 加 `pub created_by: Option<String>`；`PtySession` 加同名字段；`spawn` 改名为内部 `spawn_as(&self, opts, created_by: Option<String>)`，并保留 `spawn(&self, opts)` 委托给 `spawn_as(opts, None)`（既有测试与调用点不改）。

同时把 Task 12 的 `spawn_with_scrollback` 改成收 `created_by` 并转调 `spawn_as`，把 Task 12 里留下的那句 "Task 13 re-points this" 注释一并删掉 —— **一条说"将来会改"的注释在改完之后就是假话**：

```rust
    pub fn spawn_with_scrollback(
        &self,
        opts: &SpawnOptions,
        scrollback_lines: usize,
        created_by: Option<String>,
    ) -> Result<SpawnResult, String> {
        let result = self.spawn_as(opts, created_by)?;
        self.with_session(&result.session_id, |s| {
            s.set_scrollback_limit(scrollback_lines);
            Ok(())
        })?;
        Ok(result)
    }
```

Task 12 里那条 `the_configured_scrollback_reaches_the_session_grid` 测试同步补第三个参数 `None`。

`handlers/pty.rs::handle_spawn` 传入施动者：

```rust
    let actor = crate::gateway::visibility::ambient_actor();
```
（函数路径以 `grep -rn 'pub fn ambient_actor' src/` 为准。）
并改调 `pty::manager().spawn_as(&opts, actor.clone())`，随后落一条审计记录（用仓库既有的审计入口 —— `grep -rn 'fn record_audit\|audit::' src/gateway/ | head` 确认）。

`docs/reference/SECURITY.md` 加一节，**必须包含这三句**：

```markdown
### 内嵌终端（`pty.*`）

- **两面 operator-only**：RPC 面在 `method_admin::ADMIN_PREFIXES`，订阅面在 `event_scope::default_rules`。
- **cwd jail 只管起点**。终端内部的 `cd` 不受约束 —— 命令粒度的闸在交互式字节流上不可表达（`vim` 里的回车不是命令）。
  它买到的是**"每个终端的起点可枚举、可审计"，不是"终端不能离开工作区"**。别把它当成隔离来引用。
- **PTY 不经 `[sandbox.command_policy]` 也不经 exec tier**（`method_admin.rs` 的注释自陈 "strictly more dangerous"）。
  会话粒度的开关 `[gateway.terminal] enabled` 是这一层唯一说得出口的谓词；关掉它会杀掉在飞的会话。
- **终端历史住在服务器上**（每会话 `scrollback_lines` 行，默认 1000），因此对诊断与审计面可见。
- **同一装机的所有 operator 共享 `["*"]` 作用域**，能互相看见并 attach 彼此的会话。这是单层信任模型的有意结果，不是疏漏。
```

同时把这三句的**同义表述**同批加到 `[gateway.terminal]` 的 doc comment 与 `self_config` 的 `DESCRIPTION` —— 一句关于什么被闸住的话有三份拷贝，最贵的那份是发给模型的。

- [ ] **Step 4: 跑测试，确认通过**

```bash
cargo test -p alephcore --lib tools::scoped::gate_chain
cargo test -p alephcore --lib gateway::pty::
cargo test -p alephcore --lib --no-run
```
Expected: 全 passed。

- [ ] **Step 5: 提交**

```bash
git add src/ docs/reference/SECURITY.md
git commit -m "pty: gate the terminal switch's writer, record who spawned, document the limit

SECURITY.md states plainly what the cwd jail buys and what it does not: it
constrains the starting directory, not where a cd can go. Written down so
the next reader does not cite it as isolation."
```

---

## Task 14: Panel —— `PanelMode::Terminal` + 路由 + 空视图

**Files:**
- Modify: `interfaces/webchat/src/components/mode_sidebar.rs`、`interfaces/webchat/src/components/nav_menu.rs`、`interfaces/webchat/src/app.rs`、`interfaces/webchat/src/platform/wide/views/mod.rs`、`interfaces/webchat/locales/*`
- Create: `interfaces/webchat/src/platform/wide/views/terminal/mod.rs`

**Interfaces:**
- Consumes: 无
- Produces: `PanelMode::Terminal`；路由 `/terminal`；`TerminalView` 组件

- [ ] **Step 1: 写失败的测试**

Add to `interfaces/webchat/src/components/mode_sidebar.rs` 的 `mod tests`：

```rust
    /// `from_path` is a string chain, so the compiler cannot find it when a
    /// variant is added. This test is the thing that does.
    #[test]
    fn the_terminal_route_resolves_to_the_terminal_mode() {
        assert_eq!(PanelMode::from_path("/terminal"), PanelMode::Terminal);
        assert_eq!(PanelMode::from_path("/terminal/anything"), PanelMode::Terminal);
    }

    /// Every mode must round-trip through its own path, or a nav item leads
    /// somewhere that highlights a different tab.
    #[test]
    fn every_mode_round_trips_through_its_path() {
        for mode in PanelMode::all() {
            if matches!(mode, PanelMode::More) {
                continue; // phone-only landing, no desktop route
            }
            assert_eq!(
                PanelMode::from_path(mode.path()),
                mode,
                "{mode:?} must round-trip through {}",
                mode.path()
            );
        }
    }
```

（`PanelMode::all()` / `path()` 若不存在，本 Task 顺带加上 —— `all()` 返回一个 `&'static [PanelMode]`，`path()` 就是 `nav_menu.rs:38` 那个 match 搬进 `PanelMode`。这正好把"一族同构的映射收敛成一张表"落实。）

- [ ] **Step 2: 跑它，确认失败**

```bash
cargo test -p aleph-panel --lib components::mode_sidebar
```
Expected: FAIL —— `no variant named Terminal`。

- [ ] **Step 3: 实现**

1. `mode_sidebar.rs` 的 `enum PanelMode` 加：

```rust
    /// Embedded terminal (`views/terminal/`). Operator-only on the server
    /// side; the nav item is shown to everyone and the refusal is explained
    /// by the view, because hiding it would make "not allowed" look like
    /// "does not exist".
    Terminal,
```

2. `from_path` 加一臂（**编译器不会替你找到这里** —— 它是字符串链）：

```rust
        } else if path.starts_with("/terminal") {
            Self::Terminal
```

3. `nav_menu.rs` 的三处 match 各加一臂：`NAV_ORDER` 常量数组加 `PanelMode::Terminal`、路径 `"/terminal"`、标签 `t_string!(i18n, nav.terminal)`、图标沿用既有图标集里的终端图标。

4. `cargo build` 会在**所有穷尽 match** 处报错 —— 逐个补臂。这是加变体的正确方式。

5. `locales/` 每个语言文件加 `nav.terminal` 键（**每个**都要加；漏一个语言就是那个语言下的空白标签）。用 `ls interfaces/webchat/locales/` 列全。

6. Create `interfaces/webchat/src/platform/wide/views/terminal/mod.rs`：

```rust
//! Embedded terminal view.
//!
//! The VT emulator is on the server (see `src/gateway/pty/screen/`), so this
//! view is a renderer: it subscribes to `pty.screen`, paints a grid, and
//! sends keystrokes. Unmounting is lossless — the screen survives on the
//! server and `pty.attach` restores it — which is why the subscription is
//! ephemeral and there is no park/reveal machinery here.

use leptos::prelude::*;

#[component]
pub fn TerminalView() -> impl IntoView {
    view! {
        <div class="flex flex-1 min-w-0 min-h-0 flex-col" data-terminal-view="">
            <div class="flex-1 min-h-0 grid place-items-center text-text-secondary">
                "Terminal"
            </div>
        </div>
    }
}
```

7. `views/mod.rs` 加 `pub mod terminal;`；`app.rs` 加路由（照 `canvas` 的 `<Route path=... view=... />` 写法）。

- [ ] **Step 4: 跑测试，确认通过**

```bash
cargo test -p aleph-panel --lib
just wasm
```
Expected: 测试全 passed；`just wasm` 成功（它是唯一编译出厂形态的命令，`cargo check` 看不见这个 crate 的测试模块，也不编译 wasm 目标）。

- [ ] **Step 5: 提交**

```bash
git add interfaces/webchat/
git commit -m "panel: add the Terminal nav mode, route and empty view

PanelMode::from_path is a string chain, so adding a variant does not make
the compiler find it. The round-trip test is what does."
```

---

## Task 15: Panel —— 客户端会话（attach / seq / 缺口重同步 / 在途缓冲）

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/terminal/session.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/terminal/mod.rs`
- Modify: `interfaces/webchat/Cargo.toml`（依赖 `aleph_protocol`）

**Interfaces:**
- Consumes: `aleph_protocol::pty::*`（Task 7）
- Produces:
  - `pub struct ClientScreen { rows: u16, cols: u16, .. }`
  - `ClientScreen::apply(&mut self, frame: PtyScreenFrame) -> ApplyOutcome`
  - `pub enum ApplyOutcome { Applied, Gap { expected: u64, got: u64 }, Buffered }`
  - `ClientScreen::begin_attach(&mut self)` / `ClientScreen::finish_attach(&mut self, resp: PtyAttachResponse)`
  - `ClientScreen::row_runs(&self, row: u16) -> &[PtyStyleRun]`

- [ ] **Step 1: 写失败的测试**

Create `interfaces/webchat/src/platform/wide/views/terminal/session.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::pty::{PtyAttachResponse, PtyRowPatch, PtyScreenFrame, PtyScreenPatch, PtyStyleRun};

    fn run(text: &str) -> PtyStyleRun {
        PtyStyleRun {
            text: text.into(),
            fg: Default::default(),
            bg: Default::default(),
            attrs: Default::default(),
        }
    }

    fn frame(seq: u64, row: u16, text: &str) -> PtyScreenFrame {
        PtyScreenFrame {
            session_id: "s".into(),
            seq,
            patch: PtyScreenPatch {
                rows: vec![PtyRowPatch { row, runs: vec![run(text)] }],
                ..Default::default()
            },
        }
    }

    #[test]
    fn frames_in_order_apply() {
        let mut s = ClientScreen::new(4, 20, 0);
        assert!(matches!(s.apply(frame(1, 0, "a")), ApplyOutcome::Applied));
        assert!(matches!(s.apply(frame(2, 1, "b")), ApplyOutcome::Applied));
        assert_eq!(s.row_text(0), "a");
        assert_eq!(s.row_text(1), "b");
    }

    /// The gateway event bus is a bounded broadcast that drops for lagging
    /// subscribers, so a gap is expected traffic, not an exceptional case.
    #[test]
    fn a_gap_is_reported_rather_than_silently_misapplied() {
        let mut s = ClientScreen::new(4, 20, 0);
        let _ = s.apply(frame(1, 0, "a"));
        match s.apply(frame(3, 1, "c")) {
            ApplyOutcome::Gap { expected, got } => {
                assert_eq!((expected, got), (2, 3));
            }
            other => panic!("a missed frame must be reported, got {other:?}"),
        }
        assert_eq!(s.row_text(1), "", "a gapped frame must not be applied");
    }

    /// The snapshot is taken at seq N while frames N+1.. are already in
    /// flight. Without buffer-and-replay those frames are lost and the screen
    /// is silently wrong with no error anywhere.
    #[test]
    fn frames_arriving_during_attach_are_replayed_after_the_snapshot() {
        let mut s = ClientScreen::new(4, 20, 0);
        s.begin_attach();
        assert!(matches!(s.apply(frame(6, 2, "late")), ApplyOutcome::Buffered));
        s.finish_attach(PtyAttachResponse {
            seq: 5,
            rows: 4,
            cols: 20,
            patch: PtyScreenPatch {
                rows: vec![PtyRowPatch { row: 0, runs: vec![run("snap")] }],
                ..Default::default()
            },
            scrollback_len: 0,
        });
        assert_eq!(s.row_text(0), "snap");
        assert_eq!(s.row_text(2), "late", "in-flight frames must be replayed");
        assert_eq!(s.seq(), 6);
    }

    /// A frame at or below the snapshot's seq is already represented in the
    /// snapshot; replaying it would double-apply.
    #[test]
    fn frames_at_or_below_the_snapshot_seq_are_discarded() {
        let mut s = ClientScreen::new(4, 20, 0);
        s.begin_attach();
        let _ = s.apply(frame(5, 3, "stale"));
        s.finish_attach(PtyAttachResponse {
            seq: 5,
            rows: 4,
            cols: 20,
            patch: PtyScreenPatch::default(),
            scrollback_len: 0,
        });
        assert_eq!(s.row_text(3), "", "a frame already in the snapshot must be dropped");
    }

    #[test]
    fn a_resize_in_the_snapshot_is_adopted() {
        let mut s = ClientScreen::new(4, 20, 0);
        s.begin_attach();
        s.finish_attach(PtyAttachResponse {
            seq: 1,
            rows: 10,
            cols: 60,
            patch: PtyScreenPatch::default(),
            scrollback_len: 0,
        });
        assert_eq!(s.dims(), (10, 60));
    }
}
```

- [ ] **Step 2: 跑它，确认失败**

```bash
cargo test -p aleph-panel --lib views::terminal::session
```
Expected: FAIL。若 `aleph_protocol` 不在 `interfaces/webchat/Cargo.toml` 的 `[dependencies]`，先加（照 workspace 内其他 crate 的引用写法）。

- [ ] **Step 3: 实现**

`session.rs`（`mod tests` 之上）：

```rust
//! Client-side screen state: applies server diffs, detects gaps, and buffers
//! frames that arrive while an attach is in flight.
//!
//! The gateway event bus is a bounded broadcast that drops frames for lagging
//! subscribers, so a gap is ordinary traffic rather than an exceptional case.
//! That is why `seq` exists and why a gap must resynchronise from a snapshot
//! instead of being papered over.

use aleph_protocol::pty::{PtyAttachResponse, PtyScreenFrame, PtyStyleRun};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    /// A frame was missed. The caller must call `pty.attach` and feed the
    /// result to [`ClientScreen::finish_attach`].
    Gap { expected: u64, got: u64 },
    /// Held until the in-flight attach lands.
    Buffered,
}

pub struct ClientScreen {
    rows: u16,
    cols: u16,
    seq: u64,
    grid: Vec<Vec<PtyStyleRun>>,
    cursor: (u16, u16),
    title: Option<String>,
    alt_screen: bool,
    /// `Some` while an attach is in flight; frames land here instead of on
    /// the grid, because the snapshot they must be ordered against has not
    /// arrived yet.
    pending: Option<Vec<PtyScreenFrame>>,
}

impl ClientScreen {
    #[must_use]
    pub fn new(rows: u16, cols: u16, seq: u64) -> Self {
        Self {
            rows,
            cols,
            seq,
            grid: vec![Vec::new(); rows as usize],
            cursor: (0, 0),
            title: None,
            alt_screen: false,
            pending: None,
        }
    }

    #[must_use]
    pub const fn dims(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    #[must_use]
    pub const fn seq(&self) -> u64 {
        self.seq
    }

    #[must_use]
    pub const fn cursor(&self) -> (u16, u16) {
        self.cursor
    }

    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    #[must_use]
    pub fn row_runs(&self, row: u16) -> &[PtyStyleRun] {
        self.grid.get(row as usize).map_or(&[], Vec::as_slice)
    }

    /// Row as plain text. Test- and selection-facing.
    #[must_use]
    pub fn row_text(&self, row: u16) -> String {
        let s: String = self.row_runs(row).iter().map(|r| r.text.as_str()).collect();
        s.trim_end().to_string()
    }

    pub fn begin_attach(&mut self) {
        self.pending.get_or_insert_with(Vec::new);
    }

    /// Adopt the snapshot, then replay every buffered frame newer than it.
    pub fn finish_attach(&mut self, resp: PtyAttachResponse) {
        self.resize(resp.rows, resp.cols);
        self.seq = resp.seq;
        self.write_patch(&resp.patch);
        let buffered = self.pending.take().unwrap_or_default();
        for frame in buffered {
            if frame.seq > self.seq {
                self.seq = frame.seq;
                let patch = frame.patch;
                self.write_patch(&patch);
            }
        }
    }

    pub fn apply(&mut self, frame: PtyScreenFrame) -> ApplyOutcome {
        if let Some(buf) = &mut self.pending {
            buf.push(frame);
            return ApplyOutcome::Buffered;
        }
        let expected = self.seq + 1;
        if frame.seq != expected {
            return ApplyOutcome::Gap { expected, got: frame.seq };
        }
        self.seq = frame.seq;
        self.write_patch(&frame.patch);
        ApplyOutcome::Applied
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        if (rows, cols) == (self.rows, self.cols) {
            return;
        }
        self.rows = rows.max(1);
        self.cols = cols.max(1);
        self.grid.resize(self.rows as usize, Vec::new());
    }

    fn write_patch(&mut self, patch: &aleph_protocol::pty::PtyScreenPatch) {
        for row in &patch.rows {
            if let Some(slot) = self.grid.get_mut(row.row as usize) {
                slot.clone_from(&row.runs);
            }
        }
        if let Some(c) = patch.cursor {
            self.cursor = c;
        }
        if let Some(alt) = patch.alt_screen {
            self.alt_screen = alt;
        }
        if let Some(t) = &patch.title {
            self.title = Some(t.clone());
        }
    }
}
```

`terminal/mod.rs` 加 `pub mod session;`。

- [ ] **Step 4: 跑测试，确认通过**

```bash
cargo test -p aleph-panel --lib views::terminal::session
```
Expected: 5 passed。

- [ ] **Step 5: 提交**

```bash
git add interfaces/webchat/
git commit -m "panel: client screen with gap detection and attach buffering

A snapshot is taken at seq N while frames N+1.. are already in flight.
Without buffer-and-replay those frames are lost and the screen is silently
wrong with no error anywhere — which is the failure mode this whole seq
discipline exists to prevent."
```

---

## Task 16: Panel —— canvas2d 网格渲染

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/terminal/render.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/terminal/mod.rs`、`interfaces/webchat/Cargo.toml`（web-sys features 若缺）

**Interfaces:**
- Consumes: `ClientScreen`（Task 15）
- Produces:
  - `pub struct CellMetrics { pub width: f64, pub height: f64 }`
  - `pub fn measure(ctx: &CanvasRenderingContext2d, font: &str) -> CellMetrics`
  - `pub fn paint(ctx: &CanvasRenderingContext2d, screen: &ClientScreen, m: CellMetrics, theme: &Theme)`
  - `pub fn viewport_cells(px_w: f64, px_h: f64, m: CellMetrics) -> (u16, u16)`

- [ ] **Step 1: 写失败的测试**

纯算术部分可以脱离 DOM 测；渲染本身留给 Part 2 的真机装置。Create `render.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Viewport → grid dimensions. Off-by-one here means the server sizes the
    /// PTY to a screen the client cannot show, and the bottom row is cut off
    /// on every single session.
    #[test]
    fn viewport_cells_floors_and_never_returns_zero() {
        let m = CellMetrics { width: 8.0, height: 17.0 };
        assert_eq!(viewport_cells(800.0, 340.0, m), (20, 100));
        assert_eq!(viewport_cells(7.0, 3.0, m), (1, 1), "a tiny pane still needs one cell");
        assert_eq!(viewport_cells(0.0, 0.0, m), (1, 1), "a pane mid-layout must not divide by zero");
    }

    /// A zero or NaN metric means the font has not loaded yet. Rendering with
    /// it produces a division by zero, so the caller must be able to tell.
    #[test]
    fn metrics_report_whether_they_are_usable() {
        assert!(CellMetrics { width: 8.0, height: 17.0 }.is_usable());
        assert!(!CellMetrics { width: 0.0, height: 17.0 }.is_usable());
        assert!(!CellMetrics { width: f64::NAN, height: 17.0 }.is_usable());
    }

    #[test]
    fn indexed_colours_map_into_the_sixteen_colour_palette() {
        use aleph_protocol::pty::PtyColor;
        let t = Theme::dark();
        assert_eq!(t.resolve_fg(PtyColor::indexed(1)), t.palette[1]);
        assert_eq!(t.resolve_fg(PtyColor::Default), t.fg);
        assert_eq!(t.resolve_fg(PtyColor::rgb(1, 2, 3)), "#010203");
    }
}
```

- [ ] **Step 2: 跑它，确认失败**

```bash
cargo test -p aleph-panel --lib views::terminal::render
```
Expected: FAIL。

- [ ] **Step 3: 实现**

`render.rs`（`mod tests` 之上）：

```rust
//! Canvas2d grid renderer.
//!
//! Only dirty-free full repaints are attempted here: the client screen is
//! already the diff's result, and a 200x50 grid of style runs paints in well
//! under a frame. Run-level `fill_text` (not per-cell) is what keeps it cheap.

use aleph_protocol::pty::{PtyAttrs, PtyColor};
use web_sys::CanvasRenderingContext2d;

use super::session::ClientScreen;

/// One cell's pixel size, measured once from the loaded monospace font.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellMetrics {
    pub width: f64,
    pub height: f64,
}

impl CellMetrics {
    /// A zero or non-finite metric means the font has not loaded. Painting
    /// with it divides by zero; callers check this first.
    #[must_use]
    pub fn is_usable(self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width > 0.0 && self.height > 0.0
    }
}

/// How many cells fit. Floors, and never returns zero: a pane measured
/// mid-layout is 0x0, and a zero-column PTY is not a thing.
#[must_use]
pub fn viewport_cells(px_w: f64, px_h: f64, m: CellMetrics) -> (u16, u16) {
    if !m.is_usable() {
        return (1, 1);
    }
    let cols = (px_w / m.width).floor().max(1.0).min(f64::from(u16::MAX));
    let rows = (px_h / m.height).floor().max(1.0).min(f64::from(u16::MAX));
    (rows as u16, cols as u16)
}

/// Colour resolution. The server never sends a concrete palette because it
/// does not know the client's theme; `Default` and `Indexed` resolve here.
pub struct Theme {
    pub fg: &'static str,
    pub bg: &'static str,
    pub palette: [&'static str; 16],
}

impl Theme {
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            fg: "#e8e6e1",
            bg: "#0d0d12",
            palette: [
                "#0d0d12", "#e05561", "#8cc265", "#d18f52", "#4aa5f0", "#c162de", "#42b3c2",
                "#a1a8b3", "#4d5566", "#ff6b7f", "#a5e075", "#f0a45d", "#63b0ff", "#d977f5",
                "#5fd0e0", "#e8e6e1",
            ],
        }
    }

    #[must_use]
    pub fn resolve_fg(&self, c: PtyColor) -> String {
        match c {
            PtyColor::Default => self.fg.to_string(),
            PtyColor::Indexed(n) => self.palette[(n as usize) % 16].to_string(),
            PtyColor::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        }
    }

    #[must_use]
    pub fn resolve_bg(&self, c: PtyColor) -> Option<String> {
        match c {
            PtyColor::Default => None,
            other => Some(self.resolve_fg(other)),
        }
    }
}

/// Measure the monospace cell. `measure_text` on a wide sample divided by its
/// length is steadier than measuring one glyph, which sub-pixel rounding can
/// skew enough to drift a column over 200 cells.
#[must_use]
pub fn measure(ctx: &CanvasRenderingContext2d, font: &str) -> CellMetrics {
    ctx.set_font(font);
    const SAMPLE: &str = "MMMMMMMMMMMMMMMMMMMM";
    let width = ctx
        .measure_text(SAMPLE)
        .map(|m| m.width() / SAMPLE.len() as f64)
        .unwrap_or(0.0);
    // Line height is not measurable portably; the canonical ratio for a
    // terminal is ~1.2x the em box, and the font size is parsed from `font`.
    let px = font
        .split_whitespace()
        .find_map(|t| t.strip_suffix("px").and_then(|n| n.parse::<f64>().ok()))
        .unwrap_or(14.0);
    CellMetrics { width, height: (px * 1.2).round() }
}

/// Repaint the whole grid.
pub fn paint(ctx: &CanvasRenderingContext2d, screen: &ClientScreen, m: CellMetrics, theme: &Theme) {
    if !m.is_usable() {
        return;
    }
    let (rows, cols) = screen.dims();
    let (w, h) = (f64::from(cols) * m.width, f64::from(rows) * m.height);

    ctx.set_fill_style_str(theme.bg);
    ctx.fill_rect(0.0, 0.0, w, h);

    for row in 0..rows {
        let y = f64::from(row) * m.height;
        let mut x = 0.0_f64;
        for run in screen.row_runs(row) {
            let run_w = run.text.chars().count() as f64 * m.width;
            if let Some(bg) = theme.resolve_bg(run.bg) {
                ctx.set_fill_style_str(&bg);
                ctx.fill_rect(x, y, run_w, m.height);
            }
            ctx.set_fill_style_str(&theme.resolve_fg(run.fg));
            let weight = if run.attrs.has(PtyAttrs::BOLD) { "bold " } else { "" };
            let style = if run.attrs.has(PtyAttrs::ITALIC) { "italic " } else { "" };
            ctx.set_font(&format!("{style}{weight}14px 'JetBrains Mono', monospace"));
            let _ = ctx.fill_text(&run.text, x, y + m.height * 0.8);
            x += run_w;
        }
    }

    // Cursor as a block overlay.
    let (cr, cc) = screen.cursor();
    ctx.set_fill_style_str(theme.fg);
    ctx.set_global_alpha(0.6);
    ctx.fill_rect(f64::from(cc) * m.width, f64::from(cr) * m.height, m.width, m.height);
    ctx.set_global_alpha(1.0);
}
```

`mod.rs` 加 `pub mod render;`。

`interfaces/webchat/Cargo.toml` 的 `web-sys` features 确认含 `"CanvasRenderingContext2d"`, `"HtmlCanvasElement"`, `"TextMetrics"`（spec 预期零新增；若 `set_fill_style_str` 报缺，按 rustc 提示补 feature）。

**注意**：canvas 尺寸要按 `devicePixelRatio` 放大（`canvas.width = css_w * dpr`，`ctx.scale(dpr, dpr)`），否则高分屏上是糊的。这一步在 Task 17 挂载 canvas 时一并做。

- [ ] **Step 4: 跑测试，确认通过**

```bash
cargo test -p aleph-panel --lib views::terminal::render
just wasm
```
Expected: 3 passed；wasm 构建成功。

- [ ] **Step 5: 提交**

```bash
git add interfaces/webchat/
git commit -m "panel: canvas2d terminal grid renderer

Cell width is measured across a 20-glyph sample rather than one glyph:
sub-pixel rounding on a single measurement drifts by a full column over 200
cells. viewport_cells never returns zero, because a pane measured mid-layout
is 0x0 and a zero-column PTY is not a thing."
```

---

## Task 17: Panel —— 键盘映射 + 挂载 + 端到端

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/terminal/keymap.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/terminal/mod.rs`

**Interfaces:**
- Consumes: `ClientScreen`、`render`（Task 15–16）
- Produces:
  - `pub fn encode_key(key: &str, ctrl: bool, alt: bool, shift: bool) -> Option<Vec<u8>>`
  - `TerminalView` 完整实现（spawn / attach / 订阅 / 渲染 / 输入）

- [ ] **Step 1: 写失败的测试**

Create `keymap.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_keys_are_utf8_bytes() {
        assert_eq!(encode_key("a", false, false, false), Some(b"a".to_vec()));
        assert_eq!(encode_key("A", false, false, true), Some(b"A".to_vec()));
        assert_eq!(encode_key("中", false, false, false), Some("中".as_bytes().to_vec()));
    }

    #[test]
    fn named_keys_map_to_their_control_bytes() {
        assert_eq!(encode_key("Enter", false, false, false), Some(b"\r".to_vec()));
        assert_eq!(encode_key("Tab", false, false, false), Some(b"\t".to_vec()));
        assert_eq!(encode_key("Backspace", false, false, false), Some(vec![0x7f]));
        assert_eq!(encode_key("Escape", false, false, false), Some(vec![0x1b]));
    }

    #[test]
    fn arrows_use_the_normal_cursor_key_form() {
        assert_eq!(encode_key("ArrowUp", false, false, false), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode_key("ArrowDown", false, false, false), Some(b"\x1b[B".to_vec()));
        assert_eq!(encode_key("ArrowRight", false, false, false), Some(b"\x1b[C".to_vec()));
        assert_eq!(encode_key("ArrowLeft", false, false, false), Some(b"\x1b[D".to_vec()));
    }

    /// Ctrl-C is the single most important key in a terminal. Getting the
    /// arithmetic wrong here means the user cannot stop a runaway process.
    #[test]
    fn ctrl_letters_become_c0_controls() {
        assert_eq!(encode_key("c", true, false, false), Some(vec![0x03]));
        assert_eq!(encode_key("C", true, false, true), Some(vec![0x03]));
        assert_eq!(encode_key("d", true, false, false), Some(vec![0x04]));
        assert_eq!(encode_key("a", true, false, false), Some(vec![0x01]));
    }

    /// Alt is ESC-prefix (meta-sends-escape), the default every shell assumes.
    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(encode_key("b", false, true, false), Some(vec![0x1b, b'b']));
    }

    /// Modifier-only keydowns must send nothing, or holding Shift types
    /// garbage into the shell.
    #[test]
    fn modifier_only_keys_send_nothing() {
        for k in ["Shift", "Control", "Alt", "Meta", "CapsLock"] {
            assert_eq!(encode_key(k, false, false, false), None, "{k} must send nothing");
        }
    }

    /// Browser shortcuts we deliberately do not swallow.
    #[test]
    fn unknown_named_keys_send_nothing_rather_than_their_name() {
        assert_eq!(encode_key("F13", false, false, false), None);
        assert_eq!(encode_key("BrowserBack", false, false, false), None);
    }
}
```

- [ ] **Step 2: 跑它，确认失败**

```bash
cargo test -p aleph-panel --lib views::terminal::keymap
```
Expected: FAIL。

- [ ] **Step 3: 实现**

`keymap.rs`（`mod tests` 之上）：

```rust
//! Browser `KeyboardEvent.key` → the bytes a PTY expects.
//!
//! Deliberately the "normal" (not application) cursor-key form and
//! meta-sends-escape for Alt: those are what an unconfigured shell assumes,
//! and the alternatives are negotiated by the application via modes this
//! client does not yet track.

/// `None` means "send nothing" — a modifier-only keydown, or a key we do not
/// claim. Returning the key's *name* instead would type "F13" into the shell.
#[must_use]
pub fn encode_key(key: &str, ctrl: bool, alt: bool, _shift: bool) -> Option<Vec<u8>> {
    let base: Vec<u8> = match key {
        "Enter" => vec![b'\r'],
        "Tab" => vec![b'\t'],
        "Backspace" => vec![0x7f],
        "Escape" => vec![0x1b],
        "ArrowUp" => b"\x1b[A".to_vec(),
        "ArrowDown" => b"\x1b[B".to_vec(),
        "ArrowRight" => b"\x1b[C".to_vec(),
        "ArrowLeft" => b"\x1b[D".to_vec(),
        "Home" => b"\x1b[H".to_vec(),
        "End" => b"\x1b[F".to_vec(),
        "PageUp" => b"\x1b[5~".to_vec(),
        "PageDown" => b"\x1b[6~".to_vec(),
        "Delete" => b"\x1b[3~".to_vec(),
        "Insert" => b"\x1b[2~".to_vec(),
        "F1" => b"\x1bOP".to_vec(),
        "F2" => b"\x1bOQ".to_vec(),
        "F3" => b"\x1bOR".to_vec(),
        "F4" => b"\x1bOS".to_vec(),
        "F5" => b"\x1b[15~".to_vec(),
        "F6" => b"\x1b[17~".to_vec(),
        "F7" => b"\x1b[18~".to_vec(),
        "F8" => b"\x1b[19~".to_vec(),
        "F9" => b"\x1b[20~".to_vec(),
        "F10" => b"\x1b[21~".to_vec(),
        "F11" => b"\x1b[23~".to_vec(),
        "F12" => b"\x1b[24~".to_vec(),
        // A single printable character (any script). Anything longer is a
        // named key we do not claim.
        k if k.chars().count() == 1 => {
            let c = k.chars().next()?;
            if ctrl {
                // Ctrl-<letter> is the letter's position in the alphabet.
                let upper = c.to_ascii_uppercase();
                if upper.is_ascii_uppercase() {
                    vec![(upper as u8) - b'A' + 1]
                } else {
                    match c {
                        '[' => vec![0x1b],
                        '\\' => vec![0x1c],
                        ']' => vec![0x1d],
                        ' ' => vec![0x00],
                        _ => c.to_string().into_bytes(),
                    }
                }
            } else {
                c.to_string().into_bytes()
            }
        }
        _ => return None,
    };
    if alt {
        let mut out = vec![0x1b];
        out.extend_from_slice(&base);
        return Some(out);
    }
    Some(base)
}
```

`TerminalView` 完整实现（`terminal/mod.rs`）。仓库真实 helper 签名（已核实）：`expect_context::<DashboardState>()` · `state.rpc_call(method, params).await -> Result<Value, String>` · `state.subscribe_topic_ephemeral(pattern).await -> Result<(), String>` · `state.subscribe_events(|GatewayEvent{topic, data}| ..) -> usize` · `state.unsubscribe_events(id)`。

**核心正确性点是 `resync` 那一段** —— gap 检测到之后必须 `begin_attach` → RPC → `finish_attach`，中间到达的帧全部走 `Buffered`。写成真代码而不是注释：

```rust
pub mod keymap;
pub mod render;
pub mod session;

use aleph_protocol::pty::{PtyAttachResponse, PtyScreenFrame, PtySpawnResponse, PTY_SCREEN_TOPIC};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::context::DashboardState;
use session::{ApplyOutcome, ClientScreen};

#[component]
pub fn TerminalView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();
    // `StoredValue` rather than a signal: the screen is mutated from an event
    // callback on every frame, and a signal write per frame would re-run every
    // subscriber 60 times a second for a canvas that repaints itself anyway.
    let screen = StoredValue::new(None::<ClientScreen>);
    let session_id = StoredValue::new(None::<String>);
    let repaint_tick = RwSignal::new(0_u32);
    let error = RwSignal::new(None::<String>);

    // Re-attach: the one recovery path. A gap means the bounded broadcast
    // dropped a frame for this subscriber; the screen is on the server, so the
    // fix is to ask for it again rather than to guess.
    let resync = move |sid: String| {
        let state = state;
        spawn_local(async move {
            screen.update_value(|s| {
                if let Some(s) = s {
                    s.begin_attach();
                }
            });
            match state
                .rpc_call("pty.attach", serde_json::json!({ "session_id": sid }))
                .await
            {
                Ok(v) => match serde_json::from_value::<PtyAttachResponse>(v) {
                    Ok(resp) => {
                        screen.update_value(|s| {
                            if let Some(s) = s {
                                s.finish_attach(resp);
                            }
                        });
                        repaint_tick.update(|n| *n = n.wrapping_add(1));
                    }
                    Err(e) => error.set(Some(format!("attach decode failed: {e}"))),
                },
                // An Err is never read as an empty screen: the server said
                // something, and what it said is not "the terminal is idle".
                Err(e) => error.set(Some(e)),
            }
        });
    };

    // Mount: subscribe BEFORE spawning. Whatever the shell prints on startup
    // then cannot land in the gap between spawn and subscribe.
    Effect::new(move |_| {
        let state = state;
        spawn_local(async move {
            if let Err(e) = state.subscribe_topic_ephemeral(PTY_SCREEN_TOPIC).await {
                error.set(Some(e));
                return;
            }
            let (rows, cols) = (24_u16, 80_u16); // replaced by the measured
                                                 // viewport in the resize step below
            match state
                .rpc_call("pty.spawn", serde_json::json!({ "rows": rows, "cols": cols }))
                .await
            {
                Ok(v) => match serde_json::from_value::<PtySpawnResponse>(v) {
                    Ok(resp) => {
                        screen.set_value(Some(ClientScreen::new(resp.rows, resp.cols, resp.seq)));
                        session_id.set_value(Some(resp.session_id.clone()));
                        resync(resp.session_id);
                    }
                    Err(e) => error.set(Some(format!("spawn decode failed: {e}"))),
                },
                // Covers both refusals that have a way out: the gate
                // ([gateway.terminal] enabled = false) and the cwd jail. The
                // server's message names the remedy; show it verbatim.
                Err(e) => error.set(Some(e)),
            }
        });
    });

    // Frame handler.
    Effect::new(move |_| {
        let handler_id = state.subscribe_events(move |ev| {
            if ev.topic != PTY_SCREEN_TOPIC {
                return;
            }
            let Ok(frame) = serde_json::from_value::<PtyScreenFrame>(ev.data) else {
                return;
            };
            let Some(mine) = session_id.get_value() else { return };
            if frame.session_id != mine {
                return;
            }
            let outcome = screen.try_update_value(|s| {
                s.as_mut().map_or(ApplyOutcome::Buffered, |s| s.apply(frame))
            });
            match outcome {
                Some(ApplyOutcome::Applied) => repaint_tick.update(|n| *n = n.wrapping_add(1)),
                Some(ApplyOutcome::Gap { .. }) => resync(mine),
                _ => {}
            }
        });
        on_cleanup(move || state.unsubscribe_events(handler_id));
    });

    view! {
        <div class="flex flex-1 min-w-0 min-h-0 flex-col" data-terminal-view="">
            {move || error.get().map(|e| view! {
                <div class="px-3 py-2 text-sm text-danger" role="alert">{e}</div>
            })}
            <canvas node_ref=canvas_ref tabindex="0" class="flex-1 min-h-0 outline-none" />
        </div>
    }
}
```

剩下三段按同样风格补：

1. **重绘**：一个 `Effect` 读 `repaint_tick`，取 canvas、`render::measure`、`render::paint`。**取 canvas 与测量收进一个私有函数**，不要在 `request_animation_frame` 回调里 `get_untracked()`。
2. **resize**：`ResizeObserver`（或窗口 resize 事件）→ `render::viewport_cells` → `rpc_call("pty.resize", {session_id, rows, cols, client_id})`。挂载时也跑一次，替换上面写死的 `(24, 80)`。
3. **keydown**：`encode_key` 返回 `Some(bytes)` 才 `prevent_default()` 并发 `rpc_call("pty.input", {session_id, data: BASE64(bytes), base64: true})`；返回 `None` 一律不拦（否则浏览器快捷键全被吞掉）。

**注意 `error` 的展示用 `{move || error.get().map(..)}` 而不是 `<Show>`** —— `<Show when=…>` 的守卫与 body 是两个独立的反应式作用域，body 在信号刚被清空时可以先跑一次新值，把 `expect("visible implies Some")` 变成整页崩溃。单次读 + `Option` 视图没有这个裂缝。

**实现时必须遵守的三条**（都是仓库已经踩过的坑）：
1. **`request_animation_frame` 回调里不许 `NodeRef::get_untracked()`** —— 回调晚一帧执行，那一帧足够组件卸载，`get_untracked` 会 unwrap 成整页崩溃。把测量与取 canvas 收进**一个**私有函数，只有一种拼法。
2. **`<Show when=…>` 的守卫与 body 是两个反应式作用域** —— 别在 body 里 `expect("visible implies Some")`；用单次读 + `Option` 视图。
3. **`Err` 不许读作"空屏"** —— `pty.spawn` / `pty.attach` 失败要显示拒绝原因（尤其 `[gateway.terminal] enabled = false` 与 cwd jail 的拒绝，两者都是**有出路**的，措辞要说出出路）。

DPR：挂载与 resize 时设 `canvas.width = (css_w * dpr) as u32`、`canvas.height = (css_h * dpr) as u32`，`ctx.scale(dpr, dpr)`；再按 `render::viewport_cells` 算出 rows/cols 发 `pty.resize`。

- [ ] **Step 4: 跑测试并真机验证**

```bash
cargo test -p aleph-panel --lib views::terminal
just wasm
cargo run --bin aleph-server &
```

浏览器打开 `http://127.0.0.1:18790/terminal`，逐条确认：

1. 终端出现，有 shell 提示符。
2. 敲 `echo hello` + Enter → 屏幕出现 `hello`。
3. 敲 `ls --color=always` → 彩色输出正确着色。
4. 敲 `printf '中文表格\t|\tOK\n'` → CJK 不错位。
5. 跑 `vim`（或 `htop`）→ 备用屏生效，`:q` 退出后原屏内容还在。
6. **刷新页面** → 屏幕内容原样恢复（这是服务端持屏的核心收益）。
7. 跑 `yes | head -100000` → 页面不卡死，WS 不断（这是 16ms 合流的核心收益）。
8. Ctrl-C 能打断 `sleep 100`。
9. 开第二个标签页到 `/terminal` → 两个标签页看到同一块屏，任一处输入两边都看得见。
10. `[gateway.terminal] enabled = false` 后 `config` 热应用 → 在飞会话被杀，新 spawn 被拒且拒绝语说得出怎么打开。

**每条不过就停下修**，不要攒到最后。

- [ ] **Step 5: 提交**

```bash
kill %1
git add interfaces/webchat/
git commit -m "panel: keyboard encoding and the mounted terminal view

encode_key returns None for keys it does not claim rather than the key's
name, or an unhandled F13 types 'F13' into the shell. Ctrl-<letter> is the
letter's alphabet position: getting that arithmetic wrong means the user
cannot stop a runaway process."
```

---

## Task 18: Phase 0–4 收尾验证

**Files:**
- Modify: `docs/superpowers/specs/2026-08-29-panel-embedded-terminal-design.md`（记录实际偏差）

**Interfaces:**
- Consumes: 全部
- Produces: 一份可交给 Part 2 的、验证过的基线

- [ ] **Step 1: 跑完整验证集**

```bash
just _stage-shell-placeholders
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --bins
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p alephcore --lib gateway::pty
cargo test -p aleph_protocol pty::
cargo test -p aleph-panel --lib
cargo check -p aleph-desktop-macos
cargo clippy --workspace --all-targets
just wasm
```

**全部必须绿。** 任何一条红就停下修，不要记为"已知问题"。

- [ ] **Step 2: 确认熵减清单已执行**

```bash
rg -n '"pty\.output"'     # 零命中
rg -n 'todo!|unimplemented!' src/gateway/pty/ shared/protocol/src/pty.rs interfaces/webchat/src/platform/wide/views/terminal/   # 零命中
rg -n 'BASE64' src/gateway/pty/session.rs   # 只应出现在 pty.input 相关处，或零命中
```

- [ ] **Step 3: 确认没有留下第二条半接的路**

```bash
rg -n 'pty\.' --type rust -g '!*/tests*' src/gateway/method_census.rs
rg -n 'pty' src/gateway/handlers/mod.rs
```

`method_census` 里注册的 `pty.*` 方法数必须等于 `handlers/mod.rs` 里 `registry.register("pty...` 的行数（本计划后应为 6：spawn / input / resize / close / list / attach）。

- [ ] **Step 4: 回写 spec**

在 spec 里补一节「Part 1 实施偏差」，逐条记录与设计不同的地方，至少包括：

- Task 1 spike 发现的 `vte` 真实签名（若与假设不同）。
- Task 10 的 `conn_id` 是否走真实 WS 连接标识，还是降级成客户端生成的 `client_id`。
- Task 11 `workspace_roots()` 的真实数据源。
- Task 12 读取 live config 的真实句柄。
- Task 17 真机十条里任何一条的实际表现差异。

**这一步不是形式** —— Part 2 的任务要引用这些真实签名。

- [ ] **Step 5: 提交**

```bash
git add docs/superpowers/specs/2026-08-29-panel-embedded-terminal-design.md
git commit -m "docs: record Part 1 implementation deltas against the terminal spec"
```

---

## Part 1 完成判据

全部满足才算完成，缺一条都不算：

1. 验证集十条命令全绿。
2. Task 17 的真机十条全过。
3. `rg '"pty\.output"'` 零命中。
4. `pty.*` 的注册数与 census 数相等。
5. spec §12（Phase 0 结论）与「Part 1 实施偏差」两节都已写实。
6. `rg 'todo!|unimplemented!'` 在本计划触及的四个目录下零命中。

**Part 1 交付物明确不含**（不是缺陷，是划线，见上方「Part 1 显式不做的」）：

- **中文 / 日文 / 韩文输入**（IME 归 Part 2）—— 交付时要向用户明说这一条，否则它读起来像 bug。
- **ESC 族转义序列**（`ESC 7`/`ESC 8` DECSC/DECRC、`ESC M` RI 归 Part 2；落回 vte 默认 no-op）—— `less` / `vim` 等全屏程序下可能出现光标位置错位，交付时要向用户明说这一条，否则它读起来像 bug。
- 向上滚动看历史（`pty.scrollback` 归 Part 2；服务端**已经在存**）。
- **会话退出的任何提示**（归 Part 2）—— 服务端发 `pty.exit`，Part 1 的 Panel 不订阅它。用户 `exit` 之后终端只是停止更新，不报错也不变灰。交付时要向用户明说这一条：它比另外两条更像 bug，因为一块不再响应的矩形和一块坏掉的矩形在屏幕上是同一个东西。
- Tab 条 / 分屏 / 选区 / 搜索（B 档结构，归 Part 2）。

达成后立刻写 Part 2（Phase 5–8：Tab 条 / 分屏树 / `pty.scrollback` + 滚动 / IME / 选区 / 搜索 / `qa/terminal/run.sh` / FEATURE_LOCATOR 与判据清单补充），引用「Part 1 实施偏差」里记录的真实签名。
