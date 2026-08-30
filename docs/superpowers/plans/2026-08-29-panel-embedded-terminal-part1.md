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
cargo test -p aleph-protocol pty::   # hyphen: `-p` does NOT accept the underscore
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
cargo test -p aleph-protocol pty::   # hyphen: `-p` does NOT accept the underscore
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

**这个未知已由 controller 在派单前查实（2026-08-29），下面两个分支都不适用，走第三条**：

`JsonRpcRequest` 只有 `jsonrpc` / `method` / `params` / `id` 四个字段，**不带连接标识**——所以原写在这里的降级分支（Panel 自造 `client_id: String`）会被触发。**但它是错的，不要走**：

1. `client_id` 是**调用方自己挑的**标识。视口表按它键控，就等于让分级轴由被分级的一方决定（判据清单 §5.17）。
2. 更实在的是**生命周期**：客户端断线时没有任何东西会释放它的视口条目，而 sizing 是 smallest-wins ⇒ **一个死掉的客户端会把共享 PTY 永久钉在它最后要过的那个尺寸上**，且没有任何界面能看见这条僵尸记录。「`pty.close` 时释放」补不上这个洞——断线不是 close。

**第三条路，也是本仓已有的做法**：`conn_id` 一直存在于 WS 分发循环里（`server/handler.rs:606`，`format!("{peer_addr}")`），只是没有交给 handler。`caller_identity.rs` 已经有一组 task-local（`CALLER_ROLE` / `CALLER_USER` / `CALLER_IS_LOOPBACK`），在 `handler.rs:1976-1981` 恰好围着 `process_request` 建立作用域——`conn_id` 就在那个作用域的调用点上。

- 在 `caller_identity.rs` 加第四个 task-local `CALLER_CONN_ID: Option<String>`，与既有三个**同处**建立作用域，从那个 `conn_id` 播种；配一个 `current_caller_conn_id()` 取值函数。
- `handle_resize` 读它。拿不到（非网关调用者）时**拒绝**而不是编一个 id。
- 视口释放挂进**既有的连接拆除块**（`handler.rs:1895-1950`），紧挨着 `subscription_manager.remove_connection(&conn_id)`——那个块已经在为 conns / reverse-RPC / node registry / presence / subscriptions 五个子系统做同样的事，视口是第六个，不是一个新机制。

这样既不新增第二条身份通道，也让"断线即释放"成为结构性的而不是纪律性的。

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
- Consumes: `alephcore::workspace_root_for(&AgentDefaults)`（`src/config/agent_resolver/mod.rs:487`——**读配置的那一个**，不是 `default_workspace_root()`）。⚠️ 原本写的 `AgentEnvStore` 已查实给不出路径（`AgentEnv` 没有目录字段），`SandboxConfig::workspace_root` 是未配置时的回落、不是真源
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

`src/gateway/pty/mod.rs` 加 roots 取值函数。

**controller 已在派单前查实（2026-08-29），原本写在这里的 `todo!` 与它的前提都已作废**：

1. **`AgentEnvStore` 给不出路径**。`AgentEnv` 的字段是 `id` / `profile` / `created_at` /
   `last_active_at` / `cache_state` / `description` / `name` / `icon` / `is_archived` /
   `decay_rate` / `permanent_fact_types` —— **没有任何一个是目录**。workspace 路径是**推导**出来的
   （root + agent id），不是存出来的。所以本 Task 的 `Consumes: AgentEnvStore` 是错的，删掉它。

2. **不要用 `SandboxConfig::default().workspace_root`**，也不要用 `default_workspace_root()`。
   那两个答的是「**没有配置时**住哪」。真源是 `alephcore::workspace_root_for(&AgentDefaults)`
   （`src/config/agent_resolver/mod.rs:487`），它读 `[agents.defaults] workspace_root`、支持 `~`
   展开，并在未配置时才回落到 `default_workspace_root()`。
   **分辨的问法是「这个函数吃不吃配置」——无参 vs 有参，签名本身就在说它们答的不是同一问。**
   那个函数自己的 doc 逐字写着：它存在是因为这条规则曾在同一个文件里有三份表述，而
   **「一个复述它的 provisioning 站点必定漏掉配置的那一半」**；并且「任何创建或归档 agent 目录
   的东西都必须与它一致，因为它才是重启后重建每个 agent 的那个函数」。
   照 `SandboxConfig::default()` 写，等于在**每一台配过 `[agents.defaults] workspace_root` 的机器上**
   让 jail 的允许根**不包含操作者真正的工作区** —— 那台机器上每一次 spawn 都被拒，而干净装机上一切正常。

按此实现（`config` 从 handler 已有的取值路径拿，别新造一条）：

```rust
/// The workspace roots a PTY may be spawned under, read fresh on every spawn —
/// a boot-time snapshot would let a workspace registered after start-up stay
/// unusable until restart.
///
/// The root is `workspace_root_for(&defaults)`, NOT `default_workspace_root()`:
/// the latter answers "where does this live when nothing is configured", which
/// is a different question and is wrong on every install that sets
/// `[agents.defaults] workspace_root`.
#[must_use]
pub fn workspace_roots(defaults: &crate::config::types::AgentDefaults) -> Vec<std::path::PathBuf> {
    vec![crate::workspace_root_for(defaults)]
}
```

**如果实现时发现 `AgentDefaults` 的导入路径或 handler 侧的 config 取值路径与上面不符，以代码为准并在报告里说明** —— 上面这两点是查实的，导入路径没有逐字核过。

- [ ] **Step 4: 跑测试，确认通过**

```bash
rg -n 'todo!' src/gateway/pty/          # 必须零命中
cargo test -p alephcore --lib gateway::pty::jail
cargo test -p alephcore --lib gateway::handlers::pty
```
Expected: `rg` 无输出；测试全 passed。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/pty/jail.rs src/gateway/pty/mod.rs src/gateway/handlers/pty.rs
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

## Task 12: 会话开关 `[policies.terminal]`

**Files:**
- Modify: `src/config/types/policies/mod.rs`（+ 新建 `src/config/types/policies/terminal.rs`）、
  `src/config/reload_impact.rs`、`src/config/live_apply.rs`、`src/gateway/handlers/pty.rs`、
  `src/gateway/pty/manager.rs`、`src/gateway/handlers/mod.rs`（摘掉 `pty.spawn` 那一行）、
  `src/bin/aleph-server/commands/start/builder/handlers/system.rs`（新增 `register_pty_handlers`）、
  `src/bin/aleph-server/commands/start/mod.rs`（**调用点**，约 1919 行）、
  `src/gateway/pty/screen/grid.rs`（`Grid::set_scrollback_limit` / `scrollback_limit`）、
  `src/gateway/pty/screen/mod.rs`（`Screen::set_scrollback_limit`，转发给 grid 与已保存的备用屏）、
  `src/gateway/pty/session.rs`（`PtySession::set_scrollback_limit`）

⚠️ **后四条是 controller 2026-08-30 补上的，因为它们此前只出现在 Step 3 的代码块里、不在这张表上。**
真机后果不是"少写了一行文档"：**Step 5 的 `git add` 是照这张表写的**，于是那次提交会缺三个
被 `manager.rs` 调用的方法定义 ⇒ **提交出来编译不过**，而缺的是 `start/mod.rs` 时更隐蔽——
注册函数定义了却没人调，`method_census` 在**提交之后**才红。
判据（本计划已第五次撞上同一形状）：**Files 块和代码块是同一份事实的两份表述，而实施者按代码块
干活、按 Files 块暂存。** 所以**暂存清单要从 `git status` 推导，逐个文件问「这一轮是不是我的」，
不要从 Files 块抄。**

**Interfaces:**
- Consumes: 既有 `Config`
- Produces:
  - `TerminalConfig { enabled: bool, scrollback_lines: u32, max_sessions: usize }`
  - `PtyManager::close_all(&self) -> usize`

- [ ] **Step 1: 写失败的测试**

⚠️ **controller 在派单前查实（2026-08-29）：本任务原来写的是 `[gateway.terminal]`，那个位置
结构上不可用。整份计划已改名为 `[policies.terminal]`——这不是换个名字，是换一个真源。**

**为什么 `[gateway]` 不行：`Config` 里根本没有 `gateway` 字段，而且是有意的。**
`src/config/dead_keys.rs:75` 逐字写着这件事：

```
path: "gateway",
why: "read by GatewayConfig::load_default (src/gateway/config.rs) out of this same file;
      `Config` has no `gateway` field by design",
```

`[gateway]` 由 `src/gateway/config.rs::GatewayConfig` **第二个解析根**读同一个文件
（`Config` 是另一个根）。三个下游机件因此全部够不到它：

1. `apply_live_sections(cfg: &Config, ..)` 的入参是 `Config` —— 它**读不到** `cfg.gateway.*`；
2. `LIVE_SUBSECTIONS` 里的路径命名的是 `Config` 内的子树，`"gateway.terminal"` 命名的是**另一个根**里的东西；
3. **最要命的一条**：`self_config` / `config.patch` 经 `patcher.rs` 把 `Config` 序列化成 JSON、
   打补丁、再反序列化回 `Config`。一个 `gateway.*` 补丁于是被**静默丢弃并报成功**
   （判据「一个报成功的 no-op」）。也就是说 Task 12 的整个「热开关」故事、以及 Task 13 要为它加的
   那道闸，**守的是一条写不进去的路径**。

三个机件各自都有代码、各自都有测试，而它们连成的那条线一格都不通——这一类只有把三段放在一起读
才看得见。

**为什么 `[policies.terminal]` 是对的位置，而不只是可用的位置**：
`PoliciesConfig` 已经装着 `exec_tier` / `mode` / `spend` / `tool_permissions` / `guardian`
——**它就是「什么被允许」的那个 section**，而终端开关正是这样一个谓词，不是传输设置
（host / port / TLS / origins 才是 `[gateway]` 该管的）。`policies.spend` 还是
`LIVE_SUBSECTIONS` 唯一的既有成员，形状逐条对应：父 section 不 live、子 section live。
**而 Task 13 要加条目的那张表 `GATE_DECIDING_CONFIG_PATHS` 现有两项都在 `policies.` 下**
——新条目落在它们旁边，不再需要为「它凭什么和那两个并列」另找说辞。

先读现有的 policies 配置类型（新字段加在这里，新类型建议单独一个文件）：

```bash
grep -n 'pub struct PoliciesConfig' -A 40 src/config/types/policies/mod.rs
ls src/config/types/policies/
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
            LIVE_SUBSECTIONS.contains(&"policies.terminal"),
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

`reload_impact.rs`：`LIVE_SUBSECTIONS` 加 `"policies.terminal"`，并在其上方的 doc 里加一段说明（照 `policies.spend` 那段的写法）：

⚠️ **controller 在派单前查实（2026-08-29）：上面这段 doc 只覆盖三个字段中的一个，照抄就是把
`LIVE_SECTIONS` 自己的 doc 警告过的那个失效，往下挪了一层。** 那段 doc 逐字写着：把父 section 声明
成 live 会「为那些没有 live-apply 接线的字段一并广告『无需重启』」——而 `policies.terminal` 有**三个**
字段，`enabled` 之外的两个各有各的真相：

| 字段 | 真实的 liveness | 必须做什么 |
|---|---|---|
| `enabled` | **真 live**：`close_all()` 在 apply 时就完成 | 照 sketch 实现 |
| `max_sessions` | ⚠️ **目前是死的**：`MAX_SESSIONS` 是 `manager.rs:24` 的 `const`。加了配置字段却不改它，这个键**patch 了什么都不会发生** | 让 spawn 处**每次读**活配置，别留 `const`；否则把它排除出 liveness 声明 |
| `scrollback_lines` | **只对新会话生效**：它喂 `Grid::scrollback_limit`，而那是 `Grid::new` 时定的。已在跑的会话保留旧值 | **不要**去改已有会话的环（那会在一次 config patch 上销毁用户的回滚历史）。照实写进 doc |

所以那段 doc 必须**逐字段说话**，而不是给整个 subsection 一句总括：

```rust
/// - `policies.terminal` — declared live because each of its three fields is
///   either applied at apply time or applies to work started afterwards, and
///   NONE of them silently requires a restart:
///   * `enabled` — read fresh from the live config on every `pty.spawn`, and
///     turning it off runs `PtyManager::close_all`, so the change is complete
///     when the patch returns.
///   * `max_sessions` — read fresh at spawn time (deliberately NOT a `const`;
///     it was one until this task, which would have made the key inert while
///     this list advertised it as live).
///   * `scrollback_lines` — applies to sessions started after the patch.
///     Sessions already running keep the ring they were built with, because
///     rewriting a live ring would destroy scrollback the user can still see.
///     No restart is required to get the new value — only a new terminal.
///   `[gateway]`'s other fields (host, port, TLS) DO need a restart, hence the
///   parent stays out of `LIVE_SECTIONS`.
```

**判据（这一条比这个 Task 大）**：一句「无需重启」的声明，必须由被它覆盖的**每一个**字段兑现。
先问**这句话是谁执行的**，再问**是不是每条路径、每个字段都会执行它**——第二问才是这类缺陷的家。

⚠️ **controller 在派单前查实（2026-08-29）：这里有一条既有守卫，它挡得住一半，另一半要你自己补。**

`live_apply.rs` 的 `every_live_section_has_an_apply_arm` 是**双向**的：往 `LIVE_SUBSECTIONS`
加了名字而没往它的 `known_arms` 里加 ⇒ 红；反过来也红。所以**你会被它逼着改三处**
（`LIVE_SUBSECTIONS` / `known_arms` / `match` 臂）。

但它比对的是**两张名单**，不是「臂真的存在」——`known_arms` 是测试里**手写**的第三份拷贝。
把名字加进 `known_arms` 和 `LIVE_SUBSECTIONS`、**忘了写 `match` 臂**，这条守卫**照绿**：
控制流落进 `_ => false`，`landed = false`，走 fail-soft 的降级日志。那个降级是对的
（响应会诚实地说没落地），但它意味着**这条守卫证明不了你真的接上了线**。

**所以必须再补一条断言效果的测试**——不是断言 `close_all` 能杀会话（那是它自己的单测，
sketch 里已经有了），而是断言**走 `apply_live_sections` 这条路会到达它**：

```rust
    /// The census above only proves the name is on both lists. `known_arms`
    /// is a hand-written third copy, so a missing `match` arm still passes it
    /// -- the call falls through to `_ => false` and honestly downgrades.
    /// This asserts the wire itself: a live patch that disables the terminal
    /// must reach `close_all`, and the target must be reported as applied.
    #[test]
    fn disabling_the_terminal_live_kills_sessions_through_apply_live_sections() {
        let mgr = crate::gateway::pty::manager();
        let sid = mgr.spawn(&SpawnOptions::default()).expect("spawn").session_id;

        let mut cfg = Config::default();
        cfg.policies.terminal.enabled = false;
        let applied = apply_live_sections(&cfg, &["policies"]);

        assert!(
            applied.contains(&"policies.terminal"),
            "a declared-live target that does not land is not live"
        );
        assert!(
            mgr.list().iter().all(|s| s.session_id != sid || s.closed),
            "the in-flight session must be gone, not merely reported gone"
        );
    }
```

⚠️ 注意 `top_sections` 传的是 `["policies"]` 而**不是** `["policies.terminal"]`——单条 patch 的
调用方（`patcher.rs`）只知道它写的那条路径的**顶层** section，`dotted_prefix_matches` 就是为这个
写的。用精确名字传进去会让这条测试走一条生产上不存在的路。

（`Config::default()` / `SpawnOptions` 的实际可见性与构造方式按该文件既有测试的写法来；
若 `manager()` 的进程级单例让这条测试与兄弟测试互相干扰，**说出来再决定**，别改成断言调用次数。）

`live_apply.rs::apply_live_sections` 加一臂：把新值应用到进程 —— 关闭时 `close_all`：

```rust
        if *target == "policies.terminal" {
            if !cfg.policies.terminal.enabled {
                let killed = crate::gateway::pty::manager().close_all();
                if killed > 0 {
                    tracing::warn!(killed, "terminal disabled; live PTY sessions terminated");
                }
            }
            applied.push("policies.terminal");
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
    /// `[policies.terminal] scrollback_lines`; without this the field would be
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

⚠️ **本段是 Task 12 的历史形态，已被它自己的收尾改掉，不要照它实现。**
落地的是把 `scrollback_lines` 折进 `SpawnOptions`（`spawn` 在构造 `Screen` 之后、
`spawn_reader` 之前应用它），`spawn_with_scrollback` **已删除**，那句
"Task 13 re-points this" 注释也已删除。理由与实测见提交 `891dc8b7e` 与
`SpawnOptions::scrollback_lines` 的 doc。以树为准。

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

`handlers/pty.rs::handle_spawn` 改成调 `spawn_with_scrollback(&opts, terminal.scrollback_lines as usize)`（`terminal` 即下面那次 `config.read()` 取到的快照 —— 一次 spawn 内读一次，跨 spawn 不缓存），并用 `terminal.max_sessions` 替换 `manager.rs` 里写死的 `MAX_SESSIONS`（把该常量改成 `PtyManager` 的一个字段，`spawn` 时现读配置传入 —— 与开关同一条"现读不快照"纪律）。

⚠️ **`crate::config::current()` 不存在。** 上一版这里凭空写了它；仓库里读活配置有两种真实做法，
本任务要用的是第一种：

**用哪一种：handler 收 `config: Arc<RwLock<Config>>` 作为参数。** 这是本仓 handler 读活配置的
既有形态（`handlers/route_config.rs::handle_get(request, config)` 等五个 handler 文件都是它）。
`handle_spawn` 改成 `handle_spawn(request: JsonRpcRequest, config: Arc<RwLock<Config>>)`，
读 `config.read().await.policies.terminal`。

⚠️ **拿到句柄之后，把这个函数里那个「第二个配置答案」一并消灭——这是本任务的熵减项，不是可选项。**
Task 11 在同一个函数里留下了：

```rust
    let defaults = crate::config::Config::load().unwrap_or_default().agents.defaults;
```

它每次 spawn **重读一次磁盘**（并顺带安装一次进程级 defaults 槽）。Task 11 那样写是对的——
当时那个函数手上没有任何句柄。但你把句柄注进来之后，同一个函数里就有了**两个「当前配置是什么」
的答案，相隔三行**：一个是进程的活配置，一个是磁盘。两者在一次只改内存不落盘的 live patch 之后
会分歧，而分歧的那一次决定的是**终端被 jail 到哪个目录**。

改成从同一个快照取：

```rust
    let cfg = config.read().await;
    if !cfg.policies.terminal.enabled { /* 上面那条拒绝 */ }
    let roots = pty::workspace_roots(&cfg.agents.defaults);
    let terminal = cfg.policies.terminal.clone();
    drop(cfg);            // 别把读锁攥过 spawn
```

**同批删掉 `use` 里因此变成孤儿的那一项**（若 `Config::load` 是这个文件唯一的用处）。

顺带回答 Task 11 报告里那条：`unwrap_or_default()` 在配置读不出时替换成 `AgentDefaults::default()`，
于是 jail 指向一个**不是 operator 配置的**目录。它不是洞（fail 到另一个 jail，不是 fail 到无 jail），
但它正是 `jail.rs` 自己的模块 doc 在论证的那个形状——「一个缺省值如果回答的是另一个问题，
它就不是缺省值，是谎话」。收敛到注入句柄之后**这一问自动消失**：拿不到 config 的 handler 根本
不存在，所以没有「读不出配置」这条臂需要编一个答案。

**代价是注册要搬家**：`registry.register` 只接受 `Fn(JsonRpcRequest)`，捕获依赖的那一层是
`src/bin/aleph-server/commands/start/builder/handlers/` 里的 `register_handler!` 宏
（它闭包捕获 `Arc` 再调 `$handler(req, ctx1)`，底下仍是同一个 `register`）。所以
`pty.spawn` 那一行要从 `src/gateway/handlers/mod.rs:362` 挪到 bin 那侧、写成
`register_handler!(server, "pty.spawn", pty::handle_spawn, config);`。

**controller 已把这条路全程走过一遍（2026-08-29），四个前提都核实过，别再推演：**

1. **行号是准的**：`handlers/mod.rs:362` 就是 `pty.spawn`，362–367 六条连号。
2. **宏形态对得上**：`register_handler!` 的 1-ctx 臂做 `Arc::clone(&$ctx1)` 再
   `$handler(req, ctx1)` ⇒ handler 收 `Arc<RwLock<Config>>`，与上面的签名逐字匹配。
3. **搬到哪个文件**：`src/bin/aleph-server/commands/start/builder/handlers/system.rs`。
   那个目录有 11 个文件，随便挑一个都编得过——**而 `pty.spawn` 落进 `settings.rs` 是一个
   没有任何测试会发现的归档错误**。`system.rs` 是 daemon/系统面，且已有同形先例
   `register_oauth_handlers(server, oauth_state, config, vault, daemon)`。
   ⚠️ 注意那里的 `config` 是**逐函数参数**、不是文件级绑定，所以形状是**新加一个
   `register_pty_handlers(server: &mut GatewayServer, config: &Arc<RwLock<Config>>)`**，
   照 `register_oauth_handlers` 抄签名。
4. **调用点**：`src/bin/aleph-server/commands/start/mod.rs:1919` 附近，
   `register_oauth_handlers(&mut server, …, &app_config_for_oauth, …)` 那一行旁边。

⚠️ **别被 `app_config_for_oauth` 这个名字劝退——它不是给 OAuth 单做的快照。**
`mod.rs:1832` 是 `let app_config_for_oauth = app_config.clone();`，`Arc::clone`，
**同一个活句柄**（旁边 1831 行的 `app_config_for_reload` 正是热重载的写入端，所以经它读到的
永远是当前值）。按该处局部约定新建一个 `app_config_for_pty = app_config.clone()` 即可。
这一段写在这里，是因为一个谨慎的实施者会恰好在这里停下来怀疑自己拿到的是不是活配置——
而**怀疑是对的，答案是「是活的」**。

⚠️ **搬走而忘了落地 = `pty.spawn` 从注册表里静默消失**（判据「注册不是派发」）。这条**有守卫**：
`method_census.rs` 的扫描器是**源码级**的，`register(` 与 `register_handler!(` 两种形状它都认
（见 `literal_after_paren` 专门剥掉 `register_handler!` 多出来的那个 receiver 实参），所以搬家
之后 `pty.spawn` 仍在普查里，漏掉才会红。**但注意 `cargo test -p alephcore --lib` 看不见 bin 里
的注册**——搬完必须跑 `cargo test -p alephcore --bins`。

**不要用哪一种：进程级句柄（`policies.spend` 那套 `CapabilitySlot`）。** 它对 spend 是对的，
对这个开关是**反的**：`TerminalConfig::default().enabled == true`，所以一个**从未被 boot 安装**
的句柄读出来是 `enabled = true`——与「配置里就是开着」**逐字节相同**。失效场景是
operator 写了 `enabled = false`、boot 漏装句柄、终端照常开着，零报错。`spend` 自己的 doc 逐字
警告过这个形状（"That default is precisely the round-7 indistinguishable read: it is the right
value to return and it says nothing about whether boot got here"），只是那里的默认值是「无限额」
所以方向无害。`Arc<RwLock<Config>>` 没有这一问：拿不到 config 的 handler 根本不存在。

`handlers/pty.rs::handle_spawn` 最前面加闸（**现读，不快照** —— 每次 spawn 都 `config.read()`）：

```rust
    let terminal = config.read().await.policies.terminal.clone();
    if !terminal.enabled {
        return JsonRpcResponse::error(
            id,
            INVALID_PARAMS,
            "the embedded terminal is disabled: set `[policies.terminal] enabled = true` \
             in config.toml to turn it on"
                .to_string(),
        );
    }
```

⚠️ **拒绝语必须说出出路，而这一行原来只说了现状**（`"...is disabled ([policies.terminal]
enabled = false)"`）。controller 2026-08-29 查实：**下游有两处断言它说得出出路**，两处都会红在
这个计划自己规定的那个字符串上——

- Task 17 Step 3 第 3 条：「`Err` 不许读作『空屏』…尤其 `[policies.terminal] enabled = false`
  与 cwd jail 的拒绝，**两者都是有出路的，措辞要说出出路**」；
- Task 17 Step 4 真机第 10 条：「新 spawn 被拒且**拒绝语说得出怎么打开**」。

这是 Task 11 Minor 2 的**镜像**：那次拒绝语点名了一个**不存在**的补救（"register a workspace"，
而根本没有注册这一步）；这次拒绝语**一个补救都没点名**，而下游两处都当它点了。两种错法方向
相反，成因相同——**写拒绝语的那个人和检查拒绝语的那个人，看的不是同一段文字**。

判据：**写下一条拒绝时，把「被拒的这个人接下来该干什么」写进去，并且去核实那件事真的做得到。**
一条点名不适用补救的拒绝比什么都不说更糟；一条什么都不说的拒绝，则会让每一处断言"有出路"的
下游检查静默失真。

⚠️ 措辞刻意点名**配置文件**而不是 `self_config` 工具。这条错误回给 Panel（operator 面），
而 operator 正是能改配置的那个人；把工具名写进去等于在一条拒绝里教一条绕闸路径，而那条写入
按 Task 13 是要举卡的。
⚠️ **这一段刻意没有「如果没有句柄就补一个」的退路，而这是一次裁定不是省略。**
上一段刚论证完为什么进程级句柄（`ArcSwap` / `CapabilitySlot`）在这里是 fail-**OPEN**：
未安装的读数是 `TerminalConfig::default()`，而它的 `enabled` 是 `true`，与「operator 就是开着的」
**逐字节相同**。在同一份文件里先驳倒一个写法、两行后又指着它，是这一轮真实出现过的缺陷
（controller 2026-08-29 修）。**不要去找 `config::current()`，它不存在**（也已裁定过一次）。

⚠️ **本任务另外承担一件此前没写进来的事：它是「handler 测试在全量套件下会红」这个缺陷的唯一治法。**
controller 2026-08-29 实测（fix round 2 落地之后）：

```
cargo test -p alephcore --lib   →  17435 passed; 10 failed
```

十条里**两条是 pty handler 的**，而 fix round 2 的报告写的是「8 failed，全部既有/无关，两条新
pty 测试都不在名单里」——**那份报告在这一点上是错的**。真实失败信息把成因一次说清：

```
an omitted cwd must chdir the child into ... /T/.tmpB8sw3x/root/workspaces ...;
screen held: "/T/.tmpz5ShMj/workspaces"
```

**两个不同的临时目录。** 兄弟测试在用 `AlephHomeEnvGuard` 之类把 `ALEPH_HOME` 指向临时树，而
`std::env` 是**进程全局**的、libtest 并行跑：这条测试算 `expected` 时读到一棵树，`handle_spawn`
里 `Config::load()` 解析根时读到**另一棵**。同批还有 `mcp::…::test_default_path`（单跑绿、全量红）
与 `resize_with_conn_id_records_viewport_and_applies_it`（panic 在 `spawned`——它的临时根在
spawn 之前就被 drop 了）。

**这是判据清单那条陷阱的另一面**：「一部分测试隔离比全都不隔离更糟」。此前只从"别去隔离"这一侧
记过；这一侧是**没隔离的那个被隔离了的兄弟拖下水**，而它自己一行 env 都没写。

**成因可归属**：`aefe65457`（Task 11）把 `Config::load()` 放进了 `handle_spawn`。在那之前
handler 不读配置，因此**所有** handler 测试都没有 env 依赖。Task 10 的
`resize_with_conn_id_records_viewport_and_applies_it` 是被顺带拖脆的。

**而本任务正好拆掉这个成因**——它把 `Config::load()` 换成注入的 `Arc<RwLock<Config>>`。所以：

1. **两条 pty 测试要改成注入配置**，不再经 `Config::load()`。
2. ⚠️ **只注入配置还不够，必须注入一个 `workspace_root` 被显式设置的配置。**
   controller 查实 `agent_resolver/mod.rs:487`：`workspace_root_for` 只在
   `defaults.workspace_root` 是 `Some` 时用它，否则回落 `default_workspace_root()`——**那条路
   读 `ALEPH_HOME`**。注入一个 `workspace_root: None` 的配置只是把 env 读取往下挪了一层，
   竞争原样保留。测试要指向自己拥有的临时目录。
3. **验证方式是 8 次全量跑**，不是一次：`cargo test -p alephcore --lib` 跑八遍，报告两条 pty
   测试在八次里各红了几次。0/8 才算修好。**一次绿证明不了竞态消失**——本轮 controller 正是
   用一次绿差点推翻了一份正确的复审报告。
4. 剩下**五条 `thinker::prompt_budget` / `prompt_sanitizer` 单跑也红**，controller 已核实
   `main...HEAD` 从未碰过 `src/thinker/` ⇒ 它们在 main 上就是红的，**不归本计划**，也别顺手修。

**照现成先例做，端到端已经有一个：`src/gateway/handlers/generation_config.rs`。**
它就是这个形状的每一段——`pub async fn handle_get(request: JsonRpcRequest, config: Arc<RwLock<Config>>)`、
`let cfg = config.read().await;`、`use tokio::sync::RwLock;`（**是 tokio 的，不是 std 的**，
所以 `.await` 是对的而不是笔误）、以及在 bin 里用 `register_handler!` 把 config 注进去。
读它一遍比推演任何一种句柄都快。

- [ ] **Step 4: 跑测试，确认通过**

```bash
cargo test -p alephcore --lib config::
cargo test -p alephcore --lib gateway::pty::
```
Expected: 全 passed。

- [ ] **Step 5: 提交**

```bash
git add src/config/types/policies/mod.rs src/config/types/policies/terminal.rs \
        src/config/reload_impact.rs src/config/live_apply.rs \
        src/gateway/handlers/pty.rs src/gateway/handlers/mod.rs \
        src/gateway/pty/manager.rs src/gateway/pty/session.rs \
        src/gateway/pty/screen/grid.rs src/gateway/pty/screen/mod.rs \
        src/bin/aleph-server/commands/start/builder/handlers/system.rs \
        src/bin/aleph-server/commands/start/mod.rs
git commit -m "gateway: add the [policies.terminal] session gate, live and default-on

Declared in LIVE_SUBSECTIONS with a real handle behind it: a security switch
that waits for a restart is not a switch. Turning it off also kills live
sessions, because a gate evaluated only at admission leaves the shell that
is already open still open."
```

⚠️ **具名路径，别用目录。** 这棵树上同时有多个 agent 在干活，目录暂存会把别人的在飞工作
一起提交进你的任务（`git add src/` 尤其——它扫得到整棵源码树）。提交前跑一次
`git diff --cached --stat` 并确认它印出来的就是上面这几条，多一条都要停下来查。

---

## Task 13: `self_config` 举卡 + 问责 + SECURITY.md

**Files:**
- Modify: `src/tools/scoped/gate_chain.rs`、`src/config/types/policies/exec_tier.rs`（`GATE_DECIDING_CONFIG_PATHS`，Step 3 要改它而这一行原来漏了它）、`src/gateway/pty/manager.rs`、`src/gateway/handlers/pty.rs`、`docs/reference/SECURITY.md`

**Interfaces:**
- Consumes: `ExecTier::floor_asks_for_arguments` → `ScopedToolService::gate_removal_floor` →
  `confirmation_rule` 第 2 位 `GateRule::GateRemoval`（既有链，**不是** `DestructiveArguments`——
  见下方裁定）、`crate::gateway::visibility::ambient_actor()`（既有，`visibility.rs:143`）、
  Task 12 已落地的 `PtyManager::{scrollback_limit_of, close_all}` 与
  `SpawnOptions::scrollback_lines`
  ⚠️ **controller 对树核实（2026-08-30，Task 12 提交 `891dc8b7e` 之后）**：本行原来还列着
  `PtyManager::{spawn_with_scrollback, set_scrollback_limit}`。**两个都不在 `PtyManager` 上**——
  前者已被删除（`scrollback_lines` 折进 `SpawnOptions`），后者在 `PtySession`
  （`src/gateway/pty/session.rs`），下面还有 `Screen::` / `Grid::` 两层同名方法。
  经 manager 走的路现在是 `spawn(&opts)` + `opts.scrollback_lines`，读回来用
  `scrollback_limit_of(&session_id) -> Option<usize>`。**别按记忆里的类型名去 grep**——
  本仓已经有两次半小时花在一个「名字对、住在别的模块里」的符号上
- Produces: `SessionInfo` 增 `created_by: Option<String>`

- [ ] **Step 1: 写失败的测试**

Add to `src/tools/scoped/gate_chain.rs` 的 `mod tests`：

⚠️ **这一节原来写的两条测试断言的是 `arguments_are_destructive(..)`——即链上第 3 位
`DestructiveArguments`，也就是下方裁定整段推翻的那条规则**，而且它们一条都没有把档位设成
`Full`。换句话说：任务的**要求块**（下面第 1 问）和任务的**测试代码**互相矛盾，而测试代码是
实施者会照抄的那一半。已按裁定重写，并按 `gate_chain.rs` 现有测试的写法落在
`confirmation_rule` 上（controller 已核实 `service(..)` / `perms(..)` 两个 helper 的签名与
`GateRule::id()` 的取值，`gate_chain.rs:506/524/178`）：

⚠️ **controller 修正（2026-08-30，Task 13 落地后）：下面这些 payload 原来写的是
`{"action": "set", "path": …, "value": …}`——那不是 `SelfConfigArgs` 的线上形状。**
真实形状由 `#[serde(tag = "action", rename_all = "snake_case")]` 决定：
`{"action": "update_config", "config_path": …, "config_value": …}`，而
`self_config_touches_the_gate`（`exec_tier.rs:614`）正是按 `action == "update_config"`
与 `config_path` 读的。照原样写下去，这几条测试**在 Step 3 之后仍然是红的**，而红的理由
与它们要证明的事无关。

**生产代码没有这个错**——那道闸是既有代码，字段名一直对；错的只有本计划里的夹具。
但这是「一条只读自己刚写下的字面量的断言」的邻居形态：**一个 payload 里的键名也是 wire 契约**，
而计划文本没有编译器替它查。写跨层夹具时去读那个类型的 serde 属性，别按记忆里的字段名写。

```rust
    /// A gate whose off-switch can be flipped without a card is not a gate:
    /// two individually legal steps ("write the config", "spawn a terminal")
    /// would add up to the thing the gate refuses.
    ///
    /// Asserted at `Full`, and through `confirmation_rule` rather than through
    /// any single predicate, because `Full` is the whole point: it never asks
    /// by contract, so a rule that only fires below it buys nothing against the
    /// operator most likely to flip this switch in one sentence. Rule 2
    /// (`GateRemoval`) returns before the chain ever reaches `permission_for`
    /// — that position, not any `is_floor()` verdict, is what makes it
    /// tier-independent.
    #[test]
    fn writing_the_terminal_switch_cards_even_at_full() {
        let svc = service(
            vec![Declared { name: "self_config", idempotent: false, confirm: false }],
            ExecTier::Full,
            Some(perms(PermissionAction::Allow, &[])),
        );
        let rule = svc
            .confirmation_rule(
                "self_config",
                &json!({"action": "update_config", "config_path": "policies.terminal.enabled", "config_value": true}),
            )
            .expect("flipping the terminal gate must card at every tier, Full included");
        // The constant, not the literal: `GateRule::id`'s own doc says a rename
        // here is a compile error at the decision-set derivation, and a test
        // spelling it out by hand would quietly opt out of that.
        assert_eq!(rule.id(), crate::exec::allowed_decisions::GATE_REMOVAL_RULE);
    }

    /// The narrowness half. A rule that cards every `self_config` write would
    /// answer this task's question and destroy the tool: the claim is
    /// "gate-deciding subtrees", not "config writes".
    #[test]
    fn an_unrelated_config_write_still_does_not_card_at_full() {
        let svc = service(
            vec![Declared { name: "self_config", idempotent: false, confirm: false }],
            ExecTier::Full,
            Some(perms(PermissionAction::Allow, &[])),
        );
        assert!(
            svc.confirmation_rule(
                "self_config",
                &json!({"action": "update_config", "config_path": "behavior.greeting", "config_value": "hi"}),
            )
            .is_none(),
            "only the gate-deciding subtrees card at Full"
        );
    }

    /// `dot_paths_intersect` compares by SEGMENT (`exec_tier.rs:614` — it tests
    /// `starts_with("{b}.")`, with the dot), so a sibling key that merely shares
    /// a prefix must not be swept in. Written because "add a prefix" is how this
    /// change reads, and prefix matching would be the wrong mechanism.
    #[test]
    fn a_sibling_key_sharing_the_prefix_is_not_swept_in() {
        let svc = service(
            vec![Declared { name: "self_config", idempotent: false, confirm: false }],
            ExecTier::Full,
            Some(perms(PermissionAction::Allow, &[])),
        );
        assert!(
            svc.confirmation_rule(
                "self_config",
                &json!({"action": "update_config", "config_path": "policies.terminal_legacy.x", "config_value": 1}),
            )
            .is_none(),
            "`policies.terminal_legacy` is a different subtree"
        );
    }

    /// Requirement 3, asserted rather than assumed: an exactly-named
    /// `[policies.tool_permissions]` entry DOES stand this down, because
    /// `gate_removal_floor` is `!explicitly_named(name) && ..`. That is
    /// deliberate — the entry is a decision a person wrote, and the write that
    /// created it carded through this very rule (`policies.tool_permissions` is
    /// itself on the list). Do not "fix" this.
    #[test]
    fn an_exactly_named_entry_stands_the_terminal_floor_down() {
        let svc = service(
            vec![Declared { name: "self_config", idempotent: false, confirm: false }],
            ExecTier::Full,
            Some(perms(PermissionAction::Allow, &[("self_config", PermissionAction::Allow)])),
        );
        assert!(
            svc.confirmation_rule(
                "self_config",
                &json!({"action": "update_config", "config_path": "policies.terminal.enabled", "config_value": true}),
            )
            .is_none(),
            "an exact entry is a person's decision and stands the floor down by design"
        );
    }
```

⚠️ **RED 阶段要看对地方。** 这四条里只有第一条在实现前会红；后三条（narrowness / sibling /
exact-entry）在实现前就是绿的——它们钉的是**这次改动不许破坏的东西**，不是这次改动要带来的
东西。报告里请分开写：哪一条 RED→GREEN，哪三条全程 GREEN 且**为什么它们全程绿也仍然值得写**。
把「四条都红了」当成 RED 证据，或者反过来因为「三条本来就绿」就删掉它们，两种都是误读。

⚠️ 第 2 问（这张卡提供「始终允许」吗）由 `GateRule::is_floor()` 回答，而 `GateRemoval` 在
`is_floor()` 里**已经**是 `true`（`gate_chain.rs:217`）——**本任务不改它，也不需要为它写新测试**。
在报告里指出这一行即可。

⚠️ **controller 在派单前查实（2026-08-29）：先决定用哪条规则，`DestructiveArguments` 很可能是错的那一条。**

⚠️ **controller 在派单前把机制追到底了（2026-08-29），上一版写在这里的理由是错的，而结论是对的——所以这一段整个换掉，并且换成了一条比原来小得多的改动。**

**错在哪：** 上一版写「`is_floor()` 返回 true 同时买到两件事：卡不许持久授权 + 在 `effective_permission` 里是 rung 0，显式条目掀不翻它」。`is_floor()` 自己的注释**恰恰在否认这句话**——它逐字写着 "⚠️ 'Floor' means two different things in this subsystem and they are not in conflict"，并且**同一个 `match` 里就坐着反例**：`PlanMode` 在 `is_floor()` 里是 `false`，在 `effective_permission` 里却正是 rung 0。两个轴是两个文件里的两套机件：

- `effective_permission` 的 rung 0（`src/config/types/policies/exec_tier.rs:481`）键控在 **`tier.rule_for(facts) == Some(Deny)`** 上——它问的是「**exec 档位**拒绝了吗」，与 `gate_chain` **毫无关系**。
- `is_floor()` 只买 ①（这张卡不许提供 "always allow"）。

按上一版的理由做下去会得到一个**半截地板**：新变体 `is_floor()` 返回 true、拿到不许持久授权，却在链上排到第 3 位或更后 ⇒ **`full` 档下照旧不响**，而报告会写「已按 floor 实现」。这正是判据清单那条「**一个「地板」如果排在 explicit 条目之下，它就不是地板，是默认值**」。

**真正买到「`full` 也问」的是链上的位置，不是那个谓词。** `confirmation_rule`（`gate_chain.rs:372`）是一条**有序**链，它的注释逐条写明了这一点：

1. `ToolDeclared` —— 注释：「Read independently of the tier and of any explicit `allow`」
2. `GateRemoval` —— `gate_removal_floor()`
3. `DestructiveArguments` —— `tier_asks_for_arguments()`，**对被精确点名的工具让位**
4/5. 之后才第一次去问 `permission_for(name)`

**第 1、2 位在碰 `permission_for` 之前就 return 了**——这才是「每个档位都问、`full` 也问」的来源。第 3 位不是。

**顺带纠正一句范围**：`GateRemoval` 也**不是**「显式条目掀不翻」。`gate_removal_floor` = `!explicitly_named(name) && ExecTier::floor_asks_for_arguments(name, input)`，所以一条**精确点名** `self_config` 的 `[policies.tool_permissions]` 条目确实能让它站下——这是有意的（那条条目是人写的，而**创建它的那次写入自己会经这条规则举卡**），也正是判据清单里「操作者显式点名了这个工具」必须精确匹配的那一条。

---

**因此本任务的裁定（controller，已核实机制、已核实两处 doc 原文）：**

**不要新建 `GateRule` 变体，也不要新建平行常量。** 改动是**一个常量多一个条目 + 那个常量的 doc 重写**：

`src/config/types/policies/exec_tier.rs` 的 `GATE_DECIDING_CONFIG_PATHS`（现有两项：`policies.tool_permissions` / `policies.exec_tier`）加 `"policies.terminal"`。链路是 `GATE_DECIDING_CONFIG_PATHS` → `self_config_touches_the_gate` → `ExecTier::floor_asks_for_arguments` → `gate_removal_floor` → `confirmation_rule` 第 2 位。`dot_paths_intersect` 是**按段**比较的，所以 `"policies.terminal"` 覆盖 `policies.terminal.enabled`，而**不会**误命中一个假想的 `policies.terminal_legacy`。

⚠️ **doc 必须一起重写，这不是润色。** 那个常量现在的 doc 写的是「The two config subtrees that decide whether the argument-level cards above are raised **at all**」——一条**精确的成员资格规则**，而 `policies.terminal.enabled` **不满足它**：打开终端不会让任何一张卡不响，它是**开出一条新的执行面**。两者都该无档位地举卡，但理由不同。把新条目塞进去而不改 doc，就是让一个常量的 doc 不再描述它自己的内容——「同一事实的两份表述」里最便宜也最常见的那一种。新 doc 要写出**覆盖两类成员的那条规则**（大意：模型不许无卡写入的配置子树——写下去要么**退掉**一张参数级卡，要么**交出**一个新的执行面）。**名字保留**：改名会牵动几处 doc 链接而换不到任何行为，含义由 doc 承载。

⚠️ **两步路已经查过是闭合的**：模型想先写 `policies.tool_permissions` 把 `self_config` 点名、再无卡打开终端——**第一步自己就命中这条规则**（`policies.tool_permissions` 本来就在表上）。

下面这段是上一版的论证，**结论仍然成立**（终端配置该走 `GateRemoval` 这一位，不该走 `DestructiveArguments`），保留它是因为它说清了为什么：

用 `DestructiveArguments` 的后果因此有两个，都不是我们要的：
- **`full` 档下这张卡不响**——而一个跑在 `full` 上的 operator 正是最可能一句话就把终端打开的人；
- 卡上会出现「始终允许」，**点一次就永久授权此后每一次对终端配置的 `self_config` 写入**。

而 `GateRemoval` 的 doc **逐字描述的就是这个情形**：「This call can reach the configuration that decides whether the approval gates fire at all」。而 `[policies.terminal] enabled = true` **确实**是这样一个配置——`handlers/pty.rs` 的模块 doc 写着「A PTY is a raw shell: the command policy does not see it and the exec tier does not gate it」，所以打开终端等于开出一条**命令策略看不见、exec 档位管不着**的执行路径。

**要求**：按上面的裁定实现（`GATE_DECIDING_CONFIG_PATHS` 加一项 + 重写该常量的 doc），并在报告里逐条回答：

1. **`full` 档下这张卡响吗？** 要有一条**真的把档位设成 `Full`** 的测试，而不是断言 `is_floor()` 返回什么——后者测的是那个谓词，不是链上的位置。
2. **这张卡提供「始终允许」吗？** 必须不提供。
3. **一条精确点名 `self_config` 的 `[policies.tool_permissions]` 条目会让它站下吗？** 会——这是有意的；确认它，别去"修"它。
4. **重写后的 doc 说得出两个成员共同满足的那条规则吗？** 把新 doc 原文贴进报告。

（controller 核实过的是**机制**：`confirmation_rule` 的链序与逐条注释、`is_floor` 的返回值与它自己那段「两个含义」的警告、`effective_permission` rung 0 的真实键控、`gate_removal_floor` 的两个合取项、`GATE_DECIDING_CONFIG_PATHS` 的现有成员与它的 doc 原文、`dot_paths_intersect` 的按段语义。**没有**核实的是那个常量的作者是否愿意让它容纳第二类成员——所以第 4 问的答案如果写不出来，**停下来上报**，别把 doc 写成一句含糊话把两类东西糊在一起。）

Add to `src/gateway/pty/manager.rs` 的 `mod tests`：

```rust
    /// Accountability names the person, not just the identity: on a
    /// multi-user install "which operator" is the question an audit asks.
    #[test]
    fn a_spawn_records_who_asked_for_it() {
        // A LOCAL PtyManager, so `list()` really is this test's own sessions.
        // Never index the process-global one -- see the handler test below.
        let mgr = PtyManager::new();
        let sid = mgr
            .spawn(&SpawnOptions {
                created_by: Some("u-alice".to_string()),
                ..Default::default()
            })
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

⚠️ **这一行原来写的是「`gate_chain.rs` 的破坏性参数判据里加 `policies.terminal` 前缀」——
那正是上面整段裁定推翻的那条规则**（`DestructiveArguments` 是链上第 3 位，`full` 档下不响）。
已按裁定改写：

`src/config/types/policies/exec_tier.rs` 的 `GATE_DECIDING_CONFIG_PATHS`（`exec_tier.rs:567`，
现有两项）加 `"policies.terminal"`，**并重写它上方的 doc**（`exec_tier.rs:560-566`）。
`gate_chain.rs` 在本任务里**只加测试，不改判据**。

⚠️ **doc 重写不许再写一个数目。** 它现在的第一句逐字是「The **two** config subtrees that
decide whether the argument-level cards above are raised at all」——加了第三项之后，那句话
**两个地方都错了**：数目错了，成员资格规则也错了（`policies.terminal.enabled` 不会让任何一张卡
不响）。新 doc 写**成员资格规则**，别写成员数——一条注释里写着数目就是一条会腐烂的名单，
本仓已在这个形状上栽过四次。

⚠️ **controller 对树核实并改写本步（2026-08-30，Task 12 落地之后）。** 本步原来写的是
「`spawn` 改名为内部 `spawn_as(&self, opts, created_by)`，`spawn` 委托给它，并把
`spawn_with_scrollback` 改成收第三个参数转调 `spawn_as`」。**那条路已经不存在了**：
`PtyManager::spawn_with_scrollback` 在 Task 12 收尾时被删除，`scrollback_lines` 折进了
`SpawnOptions`（提交 `891dc8b7e`）。理由记在 `SpawnOptions::scrollback_lines` 的 doc 上——
一个"先 spawn 再回头设"的 wrapper 要把刚建好的会话**再查一次**，而 `spawn_reader` 在子进程
EOF 时会 `manager().remove(&id)`。

**而 `created_by` 是同一个形状的东西，所以答案也一样：它进 `SpawnOptions`，不要新增
`spawn_as`。** 判据一句：**这个值是不是"这次 spawn 的一个参数"**？是 ⇒ 它和 `command` /
`cwd` / `rows` / `scrollback_lines` 住在一起，由构造时一次写入，**不要为它开第三条 spawn
路径**。开一条 wrapper 就是本仓刚刚花一轮删掉的那个东西。

```rust
// src/gateway/pty/session.rs — SpawnOptions
    /// Who asked for this session, for `pty.list`'s accountability column.
    /// `None` = not attributable (a spawn that did not come through a
    /// caller-identified face). Carried here rather than through a
    /// `spawn_as` wrapper for the same reason `scrollback_lines` is — see
    /// that field's doc.
    pub created_by: Option<String>,
```

`PtySession` 加同名字段，从 `opts.created_by.clone()` 初始化；`SessionInfo` 加
`pub created_by: Option<String>` 并从会话读出。**`spawn` 的签名一个字都不用改**，既有调用点
与测试（含 `the_configured_scrollback_reaches_the_session_grid`，它用 `..Default::default()`）
**全部不受影响** —— 这正是把参数放进 options 结构体买到的东西。

⚠️ 别去找 `spawn_with_scrollback` 或那句 "Task 13 re-points this" 注释，**两者都已经不在树上**。
读一遍 `SpawnOptions` 现在的样子再动手：`grep -n 'pub struct SpawnOptions' -A 30 src/gateway/pty/session.rs`。

`handlers/pty.rs::handle_spawn` 传入施动者：

```rust
    let actor = crate::gateway::visibility::ambient_actor();
```
（函数路径以 `grep -rn 'pub fn ambient_actor' src/` 为准。）
并把它写进 `opts`：**`created_by: actor.clone()`**，紧挨着 Task 12 已经放在那里的
`scrollback_lines: Some(terminal.scrollback_lines as usize)`。`spawn` 的调用行不变。
⚠️ **这一步原来还要求「随后落一条审计记录」，controller 已在派单前把它撤掉（2026-08-29）。
不要落审计记录，也不要为它新增 `AuditEventType` 变体。**

撤掉的理由是查出来的，不是省事：本任务给的确认命令
`grep -rn 'fn record_audit\|audit::' src/gateway/` **零命中**——`src/gateway/` 下根本没有
审计入口。真正的设施在 `src/security/audit.rs`（`AuditEntry` + `SecurityAuditLog::log` +
`audit::global()`），而它现有的五个构造器（`scoped_content_read` / `authority_change` /
`command_policy` / `auth_failure` / `rate_limited`）**没有一个装得下「一个 PTY 被 spawn 了」**。
换句话说，这一步读起来是一行接线，实际是一次**没有被指定的设计决定**：要么新增一个
`AuditEventType` 变体，要么把这个含义塞进一个已有的列。

而那个文件自己的 doc 恰恰在禁止后者——`CommandPolicy` 的 doc 逐字写着为什么它不复用
`ExecBlocked`：「folding a second, unrelated meaning into that column would leave it unable
to answer either question cleanly」。那张表上每个变体都带着一段多行论证和一个裁定日期；
把第 N+1 个变体作为**另一个任务的一句顺带**加进去，正是一个列长出第二个含义的标准路径。

**问责这一半本任务已经交付了**：`SessionInfo.created_by` 有一个真实的读者——`pty.list`，
它是已注册、operator-only 的 RPC 面（判据「一个字段在提交前必须能指出读它的那一行」在这里
答得出来）。审计行则**零读者**，加它就是 R10 的「零消费者的通道优先 CUT」。

⚠️ **但撤掉不等于这个问题不存在**，所以理由留在这里而不是删干净：PTY 确实开出一条
命令策略看不见、exec 档位管不着的执行面，那**是**一个可审计事件。它作为一条具名待办留给
后续轮次——带着「该不该给 `AuditEventType` 加一个变体」这个**问题**，而不是带着一个
在本任务里顺手做出的答案。报告里请原样重述这一段裁定，别自行恢复这一步。

⚠️ **这一步欠一条经过 handler 的测试，而两个任务各自的单测都抓不到它要抓的东西。**

原因在 Task 12 折叠之后变得更简单，但一个字都没变弱：`created_by` 与 `scrollback_lines` 现在
是 `SpawnOptions` 上**并列的两个字段**，而**没有任何东西强制 `handle_spawn` 两个都填**。漏掉
`scrollback_lines` ⇒ 配置里的值到不了任何真实会话、终端历史静默退回内置默认；漏掉
`created_by` ⇒ `pty.list` 的问责列对每一行都是空。

**两个漏法在测试报告里都长得一模一样地绿。** Task 12 的
`the_configured_scrollback_reaches_the_session_grid` 直接构造 `SpawnOptions` 调 `mgr.spawn(..)`,
**从不经过 `handle_spawn`** ⇒ 它守的是"字段接到了 grid"，坏掉的是"handler 填了那个字段"。
本任务若只给 `created_by` 写一条同样形状的单测，就是把同一个盲区再造一遍。这正是判据
「守卫要断言**效果到达了**，不是**调用发生了**」——而这里"产地"是 `SpawnOptions`，
"连线"是 handler 那几行赋值。

所以这条测试必须走 handler，并**同时**断言两件事：

```rust
    /// The handler is the only place both fields have to be filled in, and it
    /// is the one place neither task's own test looks: Task 12's constructs
    /// `SpawnOptions` by hand and asserts the scrollback field reaches the
    /// grid; a `created_by` test of the same shape would assert the same
    /// thing about the other field. A `handle_spawn` that filled exactly one
    /// of them passes both.
    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn a_spawn_through_the_handler_carries_both_the_actor_and_the_scrollback() {
        let cfg = /* Arc<RwLock<Config>> with policies.terminal.scrollback_lines = 7 */;
        let resp = pty::handle_spawn(spawn_request(), cfg).await;
        let sid = session_id_of(&resp);

        let info = /* the SessionInfo for `sid` -- look it up BY ID, never list()[0]:
                      this binary's tests share one process-global manager */;
        assert_eq!(info.created_by.as_deref(), Some("u-alice"), "the actor must reach the session");
        assert_eq!(
            pty::manager().scrollback_limit_of(&sid),
            Some(7),
            "and so must the configured scrollback -- a handler that fills one \
             SpawnOptions field and not the other passes every other test in both tasks"
        );
        pty::manager().close(&sid).expect("close");
    }
```

⚠️ **两条本轮刚落地的纪律，这条测试必须遵守，否则它会让别人的测试变 flaky：**
1. **`#[serial_test::parallel(pty_global_manager)]` 是必填的**，`gateway::handlers::pty::tests`
   里每一条都有，而且有一条源码级 census（`every_test_here_is_tagged_against_the_global_manager_killer`）
   会**按名字**红掉。理由见 `config::live_apply` 那条 `close_all` 测试的 doc。
2. **`pty::manager().list()[0]` 是错的。** 这个 manager 是进程全局的，兄弟测试同时在里面
   spawn；`[0]` 拿到的是**别人的会话**。按你自己的 `sid` 查。

施动者怎么在测试里落到 `Some("u-alice")`，取决于 `ambient_actor()` 的取值方式——**先读它**
（`grep -rn 'pub fn ambient_actor' src/`），它大概率是 task-local，那就用仓库既有的
`CarriedAttribution` / scope 设置方式包一层，别自己造第二种设法。**若 `ambient_actor()` 在
测试里无法设置**，就把断言拆成两条：这条只断 scrollback（它才是会被静默回滚的那一半），
actor 那半留给 `a_spawn_records_who_asked_for_it`，并在报告里说明为什么。

`docs/reference/SECURITY.md` 加一节，**必须包含这三句**：

```markdown
### 内嵌终端（`pty.*`）

- **两面 operator-only**：RPC 面在 `method_admin::ADMIN_PREFIXES`，订阅面在 `event_scope::default_rules`。
- **cwd jail 只管起点**。终端内部的 `cd` 不受约束 —— 命令粒度的闸在交互式字节流上不可表达（`vim` 里的回车不是命令）。
  它买到的是**"每个终端的起点可枚举、可审计"，不是"终端不能离开工作区"**。别把它当成隔离来引用。
- **PTY 不经 `[sandbox.command_policy]` 也不经 exec tier**（`method_admin.rs` 的注释自陈 "strictly more dangerous"）。
  会话粒度的开关 `[policies.terminal] enabled` 是这一层唯一说得出口的谓词；关掉它会杀掉在飞的会话。
- **终端历史住在服务器上**（每会话 `scrollback_lines` 行，默认 1000），因此对诊断与审计面可见。
- **同一装机的所有 operator 共享 `["*"]` 作用域**，能互相看见并 attach 彼此的会话。这是单层信任模型的有意结果，不是疏漏。
```

同时把这三句的**同义表述**加到 `[policies.terminal]` 的 doc comment。

⚠️ **`self_config` 的 `DESCRIPTION` 那一份：不要加**（controller 裁定，2026-08-30，Task 13
落地后核实）。本行原来要求"同批加到 doc comment 与 `DESCRIPTION`"，理由是"一句关于什么被闸住的
话有三份拷贝，最贵的那份是发给模型的"。那条判据本身是对的，**但它在这里的前提是假的**：
`self_config` 的 `DESCRIPTION`（`src/builtin_tools/self_config.rs:752`）**从来没有提过这道闸**
——`GATE_DECIDING_CONFIG_PATHS` 两个既有成员（`policies.tool_permissions` /
`policies.exec_tier`）一个都没写进去。

所以第三份拷贝**对三个成员都不存在**，而不是"漏了新的这个"。只给新成员加，得到的是一份
**只列出三分之一的名单**——那正是"列举法只覆盖立法当天的世界"这条判据的**出生形态**，比没有
名单更糟：模型会读成"只有这一条会举卡"。

要给模型这份回声，是一次**独立的、覆盖全部三个成员**的改动，而且它先要在别处省出字节
（实测 108_784 B 对天花板 108_800 B，**只剩 16 字节**），**不是抬天花板**。

⚠️ **改 `self_config` 的 `DESCRIPTION` 会动描述字节棘轮，本任务原来对此一个字都没说。**
controller 已查实（2026-08-29）：`self_config` 的 `DESCRIPTION`
（`src/builtin_tools/self_config.rs:752`）经 `definitions.rs:263` 进 `BUILTIN_TOOL_DEFINITIONS`，
而 `catalog_description_bytes_ratchet`（`src/executor/builtin_registry/definitions.rs:2313`）
把它算进总和，天花板 `CATALOG_DESCRIPTION_CEILING_BYTES = 108_800`。**这些字节每个请求都付。**

所以这一步欠三件事：
1. 改之前跑一次 `cargo test -p alephcore --lib catalog_description_bytes_ratchet` 记下**实测**总数；
2. 改之后再跑一次，把**增量**写进报告（不是「还在天花板以下」，是「+N B」）；
3. ⚠️ **如果破线，停下来上报，不要抬天花板。** 下调不需要答三问，把闸设在实测值之上需要——
   而抬高一条棘轮的正当理由不可能是「我这个任务需要空间」。真到了那一步，答案更可能是
   把这句话写短，或者只留在 `[policies.terminal]` 的 doc comment 里（那份不进每个请求）。

- [ ] **Step 4: 跑测试，确认通过**

```bash
cargo test -p alephcore --lib tools::scoped::gate_chain
cargo test -p alephcore --lib gateway::pty::
cargo test -p alephcore --lib --no-run
```
Expected: 全 passed。

- [ ] **Step 5: 提交**

```bash
# ⚠️ 这一行原来漏了三个本任务必然会改的文件（controller 2026-08-30 补，**第三次**同形）：
#   src/gateway/pty/session.rs             —— created_by 进 SpawnOptions
#   src/builtin_tools/self_config.rs       —— DESCRIPTION 那句话
#   src/config/types/policies/terminal.rs  —— 同一句话的 doc comment 拷贝
# 所以：**暂存清单从 `git status --short` 推导**，逐个文件问「这一轮是不是我的」，
# 提交前跑 `git diff --cached --stat` 核对。下面这行只是起点，不是清单。
# 一个明确**不是**你的路径：src/executor/builtin_registry/definitions.rs
# （量棘轮时会临时改它；提交时它若还是 modified，说明还原没做干净，或棘轮真的动了——后者要上报）。
git add src/tools/scoped/gate_chain.rs src/config/types/policies/exec_tier.rs src/gateway/pty/session.rs src/gateway/pty/manager.rs src/gateway/handlers/pty.rs src/builtin_tools/self_config.rs src/config/types/policies/terminal.rs docs/reference/SECURITY.md
git commit -m "pty: gate the terminal switch's writer, record who spawned, document the limit

SECURITY.md states plainly what the cwd jail buys and what it does not: it
constrains the starting directory, not where a cd can go. Written down so
the next reader does not cite it as isolation."
```

⚠️ **具名路径，别用目录。** 这棵树上同时有多个 agent 在干活，目录暂存会把别人的在飞工作
一起提交进你的任务（`git add src/` 尤其——它扫得到整棵源码树）。提交前跑一次
`git diff --cached --stat` 并确认它印出来的就是上面这几条，多一条都要停下来查。

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
use unicode_width::UnicodeWidthStr;
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
    // ASCII only, and it must stay ASCII: the divisor below is `len()`, a BYTE
    // count. A non-ASCII sample would silently make every cell too narrow.
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
            // NOT `chars().count()` — see the note below this code block.
            let run_w = UnicodeWidthStr::width(run.text.as_str()) as f64 * m.width;
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

⚠️ **controller 在派单前查实（2026-08-29）：上面那个 `run_w` 原本写的是
`run.text.chars().count()`，那是错的，而且错在这个项目最常见的输入上。**

服务端 `diff.rs::row_runs` 对 spacer 是 `if cell.is_spacer() { continue; }` —— **跳过**。
而一个宽字形（CJK / emoji）在网格里占**两个单元**：字形本身 + 一个 spacer
（`grid.rs:168` 用 `UnicodeWidthChar::width(c).unwrap_or(0)` 决定）。
所以线上的一个 run 里，**一个中文字符 = 一列文本，但 = 两列屏幕**。

按 `chars().count()` 推进的后果：**一行里出现任何中文之后，其后每一个 run 都往左画偏一格，
且每多一个宽字形就多偏一格**。在一个中文项目里，这意味着**几乎每一行都渲染错位**——
而它不会报任何错。

这是同一条不变量在本计划里的**第四次**：前三次在服务端（`put` / `clear_range` / `Grid::resize`
各自漏掉「宽字形占两格」），这一次在客户端，成因是我把同一个错误的心智模型写进了渲染草稿。

**要求**：
1. `run_w` 与**背景矩形宽度**都用 `unicode_width::UnicodeWidthStr::width(run.text.as_str())`
   （两者共用 `run_w`，所以改一处即可——但要确认背景块也跟着对）。
2. `interfaces/webchat/Cargo.toml` 加 `unicode-width = "0.2"`。⚠️ **版本号必须是 `"0.2"`**——
   Cargo.lock 里**同时有 0.1.14 和 0.2.0 两份**（前者是某个三方包的传递依赖，根 `Cargo.toml`
   第 245 行的注释已经在警告这件事并给出核验命令 `cargo tree -p alephcore -i unicode-width`）。
   写 `"0.2"` 解析到**已经在的那一份**；写 `"0.1"` 会把 panel 钉到另一份上，那才是真的多一份拷贝。
   **这不违反「不引入新第三方依赖」**：
   该 crate 已在 Cargo.lock 里、同一版本、同一份拷贝（根 `Cargo.toml` 的 `[dependencies]` 已有它，
   但**不是** `[workspace.dependencies]`，而 panel 的 Cargo.toml 也不用 `workspace = true` 写法，
   所以这里要直接写版本号）。它是纯查表 crate，wasm32 可用。
3. 客户端**必须和服务端用同一个 crate 与同一套语义**——`UnicodeWidthStr::width` 逐字符求和
   `UnicodeWidthChar::width(c).unwrap_or(0)`，与 `grid.rs:168` 逐字对应。自己写一张宽度表
   就是「同一事实的两份表述」，而这一份的读者是像素。
4. **测试必须用真实宽字形**：一条含 CJK 的 run，断言其后一个 run 的 x 位置。
   只用 ASCII 的测试对这一整类**结构性失明**——这正是服务端三次都没被测到的原因。

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

## Task 19: 帧自报几何（Task 15 审查 C1 的真修法）

> **执行顺序：Task 11 落地之后、Task 17 之前。** 它改 `src/gateway/pty/session.rs`，而 Task 11
> 的暂存范围覆盖整个 `src/gateway/pty/`——两个 agent 不能同时在那里。

**Files:**
- Modify: `shared/protocol/src/pty.rs`、`src/gateway/pty/session.rs`、
  `src/gateway/pty/mod.rs`（唯一一条真走总线的测试，见下）、
  `interfaces/webchat/src/platform/wide/views/terminal/session.rs`

**Interfaces:**
- Consumes: `PtyScreenFrame`（Task 7）、`ClientScreen`（Task 15）
- Produces: `PtyScreenFrame { session_id, seq, rows, cols, patch }`

### 这个任务在修什么

Task 15 的审查判了一个 Critical：**窗口变大之后，客户端底部若干行永远空白，而且永不自愈。**
`ClientScreen::write_patch` 用 `self.grid.get_mut(row.row as usize)`，行号超出当前网格时**静默跳过**；
`dims()` 仍报旧尺寸；而 `seq` 始终连续 ⇒ **不会 gap ⇒ 不会重新 attach**。

⚠️ **别把 `ClientScreen::resize` 改成 public——那不是修法，是把缺陷换个位置。** controller 查实
（2026-08-29）：

- `handle_resize` 只返回 `{"ok": true}`，**不报生效尺寸**；
- 而尺寸是 **smallest-wins**（Task 10）：一个客户端要 40x120、另一个挂着 24x80 时，PTY 就是
  24x80。**它要到的和拿到的不是一回事**；
- 更糟的是**被别人挤小的那一个**——它自己没调用过任何东西，没有任何响应会告诉它；
- `PtyScreenFrame` 只有 `session_id` / `seq` / `patch`，**不带几何**。

所以今天客户端学到真实尺寸的**唯一**途径是重新 attach。公开 `resize` 只会让 Task 17 把它
**请求**的尺寸写进屏幕——一个恰好在共享场景下出错的第二真源，而共享正是 Task 10 存在的理由。

**真修法：让帧自报它的几何。** 与 `seq` 同层、与 `PtyAttachResponse` 同形（那个结构早就把
`rows`/`cols` 摆在 `patch` 旁边）。这样**每一个**附着的客户端都在下一帧学到新尺寸，包括从没调用过
resize 的那个——与判据「一帧带着自己的归属到达」同形。

- [ ] **Step 1: 写失败的测试**

`interfaces/webchat/src/platform/wide/views/terminal/session.rs` 的 `mod tests`：

⚠️ **先改夹具，而且 `(4, 20)` 不是随便填的。** 该模块已有 `run(text)` / `frame(seq,row,text)` /
`frame_for(session_id,seq,row,text)` 三个 helper，而**现存每一条测试都用 `ClientScreen::new(4, 20, ..)`**。
`PtyScreenFrame` 新增必填的 `rows`/`cols` 之后 `frame_for` 必须补上它们，**而补什么决定了整个套件
会不会静默改变行为**：

- 填 `(4, 20)` ⇒ 与现有屏幕尺寸一致，`resize` 早返回，**现存测试逐字节不变**。这是要的。
- 填别的 ⇒ 每条现存测试在第一帧就被 resize，`row_text` 的答案随之改变；
- 填 `(0, 0)` 最坏 ⇒ `resize` 会 clamp 到 `(1, 1)`，整个套件塌成一行，读起来像「我的改动
  把所有东西都弄坏了」，而真相是**夹具在撒谎**。

```rust
    // Existing helper, unchanged: `PtyStyleRun` does NOT derive `Default`.
    // fn run(text: &str) -> PtyStyleRun { .. }

    /// The dimensions every existing test's screen already has, so a frame
    /// built by `frame_for` resizes nothing and the suite is unchanged by
    /// this field's arrival. Any other value silently resizes on frame one.
    const FIXTURE_DIMS: (u16, u16) = (4, 20);

    fn frame_for(session_id: &str, seq: u64, row: u16, text: &str) -> PtyScreenFrame {
        frame_sized(session_id, seq, FIXTURE_DIMS.0, FIXTURE_DIMS.1, row, text)
    }

    fn frame_sized(
        session_id: &str,
        seq: u64,
        rows: u16,
        cols: u16,
        row: u16,
        text: &str,
    ) -> PtyScreenFrame {
        PtyScreenFrame {
            session_id: session_id.into(),
            seq,
            rows,
            cols,
            patch: PtyScreenPatch {
                rows: vec![PtyRowPatch { row, runs: vec![run(text)] }],
                ..Default::default()
            },
        }
    }

    /// A resize the client did not ask for still has to land. Sizing is
    /// smallest-wins across clients, so a client can be shrunk by someone
    /// else joining without ever calling `pty.resize` itself -- and a grow
    /// leaves `seq` contiguous, so nothing ever gaps and nothing self-heals.
    #[test]
    fn a_frame_carrying_new_geometry_grows_the_screen_before_its_rows_land() {
        let mut s = ClientScreen::new(24, 80, 5, SID);

        assert_eq!(s.apply(frame_sized(SID, 6, 40, 100, 39, "bottom")), ApplyOutcome::Applied);

        assert_eq!(s.dims(), (40, 100));
        // The ordering assertion: adopting AFTER write_patch leaves the grid
        // 24 rows long, `get_mut(39)` returns None, and the row is dropped
        // with no error anywhere.
        assert_eq!(
            s.row_text(39),
            "bottom",
            "the new geometry must be adopted before its rows are written"
        );
    }

    /// Shrink is the other half and it is not symmetric: rows past the new
    /// bottom must go away, or the renderer keeps painting content the
    /// server no longer has.
    #[test]
    fn a_shrinking_frame_drops_the_rows_below_the_new_bottom() {
        let mut s = ClientScreen::new(40, 100, 5, SID);
        assert_eq!(s.apply(frame_sized(SID, 6, 40, 100, 39, "gone")), ApplyOutcome::Applied);
        assert_eq!(s.row_text(39), "gone");

        // A frame with no row patches at all, carrying only the new size.
        let shrink = PtyScreenFrame {
            session_id: SID.into(),
            seq: 7,
            rows: 24,
            cols: 80,
            patch: PtyScreenPatch::default(),
        };
        assert_eq!(s.apply(shrink), ApplyOutcome::Applied);

        assert_eq!(s.dims(), (24, 80));
        assert_eq!(s.row_text(39), "", "a row past the new bottom must not survive");
    }

    /// A frame we are throwing away must not move the geometry either. Its
    /// dimensions are as old as its content.
    #[test]
    fn a_discarded_frame_does_not_move_the_geometry() {
        let mut s = ClientScreen::new(24, 80, 5, SID);
        let stale = frame_sized(SID, 3, 40, 100, 0, "old");
        assert!(matches!(s.apply(stale), ApplyOutcome::Discarded { .. }));
        assert_eq!(s.dims(), (24, 80), "a discarded frame carries stale dimensions too");
    }

    /// The replay path, which is the one a fix written only into `apply`
    /// misses entirely. Frames that arrive while an attach is in flight are
    /// buffered and replayed by `finish_attach` -- and a resize is exactly
    /// what can happen in that window, since sizing is smallest-wins and
    /// another client leaving grows this one without it calling anything.
    ///
    /// Without `settle`, replay writes rows straight into a grid still sized
    /// to the attach snapshot, `get_mut` returns None past the old bottom,
    /// the rows vanish, and `seq` advances anyway -- so nothing gaps and
    /// nothing ever heals. That is the same defect this task exists to fix,
    /// reproduced inside the code that was written to fix it.
    #[test]
    fn a_buffered_frame_that_grows_the_screen_lands_its_rows_on_replay() {
        let mut s = ClientScreen::new(24, 80, 5, SID);
        s.begin_attach();

        // Arrives mid-attach, carrying a geometry the snapshot will not have.
        assert_eq!(s.apply(frame_sized(SID, 7, 40, 100, 39, "late")), ApplyOutcome::Buffered);

        // The snapshot is older and smaller: 24x80 at seq 6.
        let resp = PtyAttachResponse {
            seq: 6,
            rows: 24,
            cols: 80,
            patch: PtyScreenPatch::default(),
            scrollback_len: 0,
        };
        assert_eq!(s.finish_attach(resp), AttachOutcome::Resynced);

        assert_eq!(s.dims(), (40, 100), "the replayed frame's geometry must be adopted too");
        assert_eq!(
            s.row_text(39),
            "late",
            "a replayed row past the snapshot's bottom must not be silently dropped"
        );
    }
```

`shared/protocol/src/pty.rs` 的 `mod tests` —— Task 7 已有一条**键集相等**的对账测试
（不是超集断言），它会因为这次新增而红。**那是它在正常工作**：更新期望键集，别放宽断言。

**还有第三个消费点，而且它是这次改动里最有价值的那条断言。**
controller 派单前数过：`PtyScreenFrame` 全仓有三处非定义点——生产者
（`pty/session.rs::feed_and_take_frame`）、客户端消费者（Panel 的 `session.rs`），
以及 `src/gateway/pty/mod.rs` 里 Task 9 那条
`a_write_reaches_a_real_subscriber_over_the_pty_screen_topic`。

**它是仓里唯一一处 `PtyScreenFrame` 真的走一遍总线的地方**——真 spawn、真写入、
真 flush loop、真订阅者、真 `from_value`。上面那两条客户端单测证明的是「`apply` 拿到
几何之后做得对」；**没有任何一条证明服务端真的把几何放上了线**。一个把 `rows`/`cols`
在锁外读、或者填错顺序（`(cols, rows)`）的实现，两条单测**全绿**。

那条测试用 `SpawnOptions { rows: 10, cols: 40, .. }` spawn，所以断言是现成的：

```rust
        // Geometry on the wire. The client-side unit tests prove `apply`
        // does the right thing WITH a frame's dimensions; only this one
        // proves the server puts them there. An implementation that read
        // dims outside the screen lock, or transposed them, passes every
        // other test in this change.
        assert_eq!(
            found,
            Some((10, 40)),
            "the frame must carry the geometry it was spawned with, rows first"
        );
```

把现有的 `let mut found = false;` / `found = true;` 改成携带几何的
`Option<(u16, u16)>` 即可；循环的退出条件与轮询形状**一个字都不要动**
（那是有意与 `a_child_write_reaches_the_server_held_screen` 保持同形的有界轮询，
不是固定 sleep）。

⚠️ 顺带一条**不要修**的：那个循环里的 `let Ok(frame) = .. else { continue; };`
是 fail-soft 跳过，读起来像判据清单里「跳过一条坏记录要问跳过之后还有谁看得见它」
的那一类。这里**不要**改成报错——一条解不出的帧在这个测试里的正确处置就是跳过，
而失败信号由循环结束后的断言承担（找不到就是 `None`，会响亮地失败并打印它期望的值）。

- [ ] **Step 2: 跑它，确认失败**

```bash
cargo test -p aleph-panel --lib views::terminal::session
cargo test -p aleph-protocol pty::
```
Expected: 两边都 FAIL。

- [ ] **Step 3: 实现**

**协议**（`shared/protocol/src/pty.rs`）—— `PtyScreenFrame` 加两个**必填**字段：

```rust
pub struct PtyScreenFrame {
    pub session_id: String,
    pub seq: u64,
    /// The screen's dimensions as of this frame.
    ///
    /// Beside `seq` rather than inside `patch`, and matching
    /// `PtyAttachResponse`'s shape, because geometry is a property of the
    /// frame rather than of the content delta. Carried on EVERY frame, not
    /// just the ones after a resize: sizing is smallest-wins across attached
    /// clients, so a client can be resized by someone else joining without
    /// having called anything, and there is no response it could read.
    pub rows: u16,
    pub cols: u16,
    pub patch: PtyScreenPatch,
}
```

⚠️ **不要加 `#[serde(default)]`。** 默认值 0 会是一句谎话，而这两个字段的消费者会拿它去 resize
网格。Part 1 是这个协议的第一个发行版，生产端与消费端同批发布，所以必填是可以的——**响亮失败强于
一个静默的 0**。

**服务端**（`src/gateway/pty/session.rs::feed_and_take_frame`）—— 几何必须在**取 patch 的同一把
锁里**读，否则会发出一份「patch 属于旧几何、dims 属于新几何」的撕裂帧：

```rust
        let (patch, rows, cols) = {
            let mut screen = self.screen.lock().unwrap_or_else(|e| e.into_inner());
            let patch = screen.take_patch()?;
            let (rows, cols) = screen.grid.dims();
            (patch, rows, cols)
        };
```

紧挨着的 `attach_snapshot` 已经是这个写法（同一把锁里同时读 dims 和快照），照它。

**客户端** —— 几何必须在写入行**之前**采纳。而这里有个陷阱，controller 在派单前查实
（2026-08-29）：**`write_patch` 有两个调用者，把采纳写进 `apply` 只覆盖了其中一个。**

`finish_attach` 的重放循环（Task 15 复审后新增的那段）调的是 `self.write_patch(&frame.patch)`，
**不是 `self.apply(frame)`**。所以「在 `apply` 里 resize」这个改法对重放路径是 no-op：

- attach 在飞的时候 PTY 被别人挤大（smallest-wins，另一个客户端离开）；
- 缓冲下来的帧带着**新**几何，而网格还是 attach 快照那么大；
- 重放直接 `write_patch` ⇒ `grid.get_mut(row)` 对超出旧底部的行返回 `None` ⇒ **静默丢行**；
- 而 `seq` 照常前进 ⇒ 不 gap ⇒ 不重新 attach ⇒ **永不自愈**。

那**逐字就是这个任务要修的那个 C1**，只不过发生在为修另一个 C1 而新写的代码里。而下面 Step 1 的
两条测试只走 `apply`，**它们会在这条路还坏着的时候全绿**。

**所以修法是结构性的，不是纪律性的：删掉 `write_patch`，把它的函数体并进一个必须同时收下几何的
私有方法。** 这样「先采纳几何」不再是一条要记住的规则，而是**唯一存在的写法**——没有第二个方法
可以伸手。

```rust
    /// Adopt this frame's geometry, then write its rows.
    ///
    /// The two steps are one method rather than two calls because the order
    /// between them is the whole correctness argument: `write_patch`'s
    /// `grid.get_mut(row)` silently returns `None` for a row past the current
    /// bottom, so writing before resizing drops rows with no error anywhere
    /// and no gap to trigger a re-attach. There were two call sites for the
    /// write half (`apply` and `finish_attach`'s replay loop) and a rule that
    /// only one of them followed; there is now no way to write a patch
    /// without handing over the geometry it belongs to.
    fn settle(&mut self, rows: u16, cols: u16, patch: &PtyScreenPatch) {
        if (rows, cols) != (self.rows, self.cols) {
            self.rows = rows.max(1);
            self.cols = cols.max(1);
            self.grid.resize(self.rows as usize, Vec::new());
        }
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
```

`fn resize` 与 `fn write_patch` **都删掉**（`resize` 的三行已经并进 `settle` 的头部；它此前唯一的
调用者就是 `finish_attach`）。三个调用点改成：

```rust
    // apply(), on the Applied path only:
        self.seq = frame.seq;
        self.settle(frame.rows, frame.cols, &frame.patch);
        ApplyOutcome::Applied

    // finish_attach(), the snapshot:
        self.seq = resp.seq;
        self.settle(resp.rows, resp.cols, &resp.patch);

    // finish_attach(), inside the replay loop:
            self.seq = frame.seq;
            self.settle(frame.rows, frame.cols, &frame.patch);
```

⚠️ **`Discarded` 与 `Gap` 两条臂不许采纳几何。** `Discarded` 的帧比我们手上的旧，它的几何也旧；
`Gap` 会触发重新 attach，而 attach 响应本来就带 `rows`/`cols`。在这两条臂上采纳等于让一个被丢弃的
帧改写当前状态。重放循环里 `frame.seq <= self.seq` 的 `continue` 同理——**那条臂什么都不许碰**。

⚠️ 顺带确认一件**不要改**的事：`finish_attach` 里对快照的 `settle` 排在重放循环**之前**，
顺序不变。快照是基线，缓冲帧是它之后的增量。

**顺手带一条 doc（你本来就在这个文件里）。** Task 15 的复审提了一条不值得单开一轮、但**理由目前
只活在一份报告里**的观察：`finish_attach` 的 `resp.seq < self.seq` 那条臂返回 `Resynced`，而它
表达的其实是第三种状态——「这份快照比我手上的还旧，我忽略了它并丢掉了缓冲」。那不完全是
「重新同步成功」，正是当初把 `AttachOutcome` 从 `ApplyOutcome` 拆出来的同一个形状，只是没贯彻到底。

**不要改枚举**（第三个变体会让每个调用点多一条它不知道该怎么处理的臂，而这条路径自愈：`seq` 没动，
下一个实时帧要么接上要么 gap）。**要做的是把理由从报告搬进 `finish_attach` 的 doc**：说清这条臂
何时可达（只有乱序/重复的 attach 响应；在「同一时刻只有一个 attach 在飞」的不变量下 `self.seq`
不可能跑到一份新快照前面）、为什么丢缓冲是对的（快照自身的有效性被否掉之后，就没有基线可供重放，
拿错基线重放正好会把这一轮修掉的跳洞 bug 装回来），以及它复用 `Resynced` 是一次**有意的**取舍。

判据：**一个发现如果只写在报告里，它就等于没写**——今天已经栽过一次（Task 7 的实现者发现
`cargo -p aleph_protocol` 解析不出包、自己绕过去了，那条发现在报告里躺到我预读 Task 18 才被重新
发现一遍）。

- [ ] **Step 4: 跑测试，确认通过**

```bash
cargo test -p aleph-protocol pty::
cargo test -p alephcore --lib gateway::pty::
cargo test -p aleph-panel --lib views::terminal
just wasm
```

⚠️ 那条 wire 测试在 `gateway::pty::` 的过滤范围里，但它是 `#[tokio::test(flavor = "multi_thread")]`
且要真起一个 PTY —— **确认它真的跑了**，别只看总数是绿的。

**并且做两次变异**（两次都要贴 RED 输出，做完各自改回来）：

1. **顺序**：把 `settle` 里那段 `grid.resize` 挪到写行的 `for` 循环**之后**。确认
   `a_frame_carrying_new_geometry_grows_the_screen_before_its_rows_land` **红**。
   顺序是这个修复的全部内容，而一条不能证伪顺序的测试证明不了顺序。

2. **覆盖面**：这次不改 `settle`，改**调用点**——把重放循环里的
   `self.settle(frame.rows, frame.cols, &frame.patch)` 换回「只写不采纳」
   （临时加一个本地的 `write_only` 内联那段 `for` 循环即可）。确认
   `a_buffered_frame_that_grows_the_screen_lands_its_rows_on_replay` **红**，
   而另外三条**仍然绿**。

第二次变异是有意义的那一次：它模拟的正是「把修复只写进 `apply`」这个最自然的错法。如果它红了
而第一条测试没红，说明两条测试各自守着一个真正不同的调用点——这正是要的。**如果第二次变异
一条都没红，别当成「守卫很稳」，去查那条测试有没有真的走到重放路径。**

- [ ] **Step 5: 提交**

```bash
git add shared/protocol/src/pty.rs src/gateway/pty/session.rs src/gateway/pty/mod.rs interfaces/webchat/src/platform/wide/views/terminal/session.rs
git commit -m "pty: carry the screen's geometry on every frame

A client could not learn its own dimensions without re-attaching.
pty.resize answers {ok: true} and nothing more, and sizing is
smallest-wins across attached clients -- so the size a client asks for
is routinely not the size the PTY got, and a client shrunk by someone
else joining called nothing and had nothing to read.

Growing was the silent case: write_patch skips a row index past the
grid, dims stayed stale, and seq stayed contiguous, so no gap was ever
reported and the blank rows never healed."
```

---

## Task 17: Panel —— 键盘映射 + 挂载 + 端到端

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/terminal/keymap.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/terminal/mod.rs`、`interfaces/webchat/src/platform/wide/views/terminal/render.rs`（Task 16 复审 Minor 1：字号的第二份表述，见 Step 3 第 1 点）

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

⚠️ **这是一次整体重写，而现有那 28 行里有两样东西没有读者——所以丢掉它们不会有任何东西变红。**
controller 派单前查实（2026-08-29），两样都要**原样保留**：

1. **模块 doc**（文件顶部那 12 行）。它解释的是**非显然的设计理由**——VT 在服务端所以本视图只是
   渲染器、**卸载是无损的（屏幕活在服务端、`pty.attach` 能恢复）所以订阅是 ephemeral 且这里
   没有 park/reveal 机件**、以及手机端为什么渲染空（与 `PanelMode::Projects` 同样处置）。
   下面的代码块**从 `pub mod keymap;` 开始，不含这段 doc**——那不是让你删掉它的意思。
   补一句 `pub mod keymap;` 进去即可。

2. **`data-terminal-view=""` 属性**。全仓**零消费者**（controller grep 过），但它是 Part 2 真机
   装置 `qa/terminal/run.sh` 的锚点。一个零消费者的钩子在整体重写里必然被静默丢掉，而它没有
   任何守卫——这正是判据「一个机制的存在理由如果只写在别的文件里，删它的人不会读到那里」，
   而这里的理由**根本没被写在任何地方**。现在写在这里了：保留它，并在它旁边留一行注释说明
   它是给谁用的。

**核心正确性点是 `resync` 那一段** —— gap 检测到之后必须 `begin_attach` → RPC → `finish_attach`，中间到达的帧全部走 `Buffered`。写成真代码而不是注释：

```rust
pub mod keymap;
pub mod render;
pub mod session;

use aleph_protocol::pty::{PtyAttachResponse, PtyScreenFrame, PtySpawnResponse, PTY_SCREEN_TOPIC};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::context::DashboardState;
use session::{ApplyOutcome, AttachOutcome, ClientScreen};

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
                        // `finish_attach` reports whether the replay of
                        // frames buffered during this RPC hit a hole. It
                        // stops AT the hole rather than skipping it, so the
                        // screen never claims to be more current than it is.
                        let outcome = screen.try_update_value(|s| {
                            s.as_mut().map(|s| s.finish_attach(resp))
                        });
                        repaint_tick.update(|n| *n = n.wrapping_add(1));
                        // Deliberately NOT a re-attach from here. `resync` is
                        // a closure and cannot call itself, and an immediate
                        // retry is the shape that loops when the bus is
                        // dropping faster than we attach. `finish_attach`
                        // leaves `seq` at the last frame it actually applied,
                        // so the next live frame gaps on its own and the frame
                        // handler's existing `Gap` arm re-attaches -- one path,
                        // already tested.
                        //
                        // The honest cost: on a terminal that goes quiet right
                        // after the hole, no live frame arrives, so the rows
                        // the missing frame would have touched stay wrong until
                        // it speaks again. Worth knowing, not worth a retry
                        // loop; if it ever matters the fix is a one-shot
                        // re-attach guarded by a flag, not recursion.
                        if let Some(Some(AttachOutcome::Gap { expected, got })) = outcome {
                            leptos::logging::log!(
                                "pty attach replay hit a hole: expected {expected}, got {got}"
                            );
                        }
                    }
                    Err(e) => error.set(Some(format!("attach decode failed: {e}"))),
                },
                // An Err is never read as an empty screen: the server said
                // something, and what it said is not "the terminal is idle".
                Err(e) => error.set(Some(e)),
            }
        });
    };

    // Mount: subscribe BEFORE resolving a session. Whatever the shell prints
    // on startup then cannot land in the gap between spawn and subscribe.
    //
    // Resolving is list-then-spawn, NOT spawn. See the note below this block.
    Effect::new(move |_| {
        let state = state;
        spawn_local(async move {
            if let Err(e) = state.subscribe_topic_ephemeral(PTY_SCREEN_TOPIC).await {
                error.set(Some(e));
                return;
            }

            // A live session already on the server IS this view's session.
            // A refresh, a second tab and a reconnect all arrive here.
            let existing: Option<String> = match state
                .rpc_call("pty.list", serde_json::json!({}))
                .await
            {
                Ok(v) => v.get("sessions").and_then(|s| s.as_array()).and_then(|arr| {
                    arr.iter()
                        .find(|s| {
                            s.get("closed").and_then(serde_json::Value::as_bool) == Some(false)
                        })
                        .and_then(|s| s.get("session_id"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                }),
                // A failed list is not evidence that there are no sessions.
                // Falling through to spawn is still right: a gateway broken
                // enough to fail `pty.list` fails `pty.spawn` too and says so.
                // What must not happen is showing the failure as an answer.
                Err(_) => None,
            };

            if let Some(sid) = existing {
                // Dimensions and seq arrive with the attach response and
                // `finish_attach` re-seats both, so this placeholder is never
                // observed. Setting the id BEFORE the RPC matters: the frame
                // handler DROPS frames whose session it cannot name, and a
                // dropped frame is not a buffered one.
                screen.set_value(Some(ClientScreen::new(24, 80, 0, sid.clone())));
                session_id.set_value(Some(sid.clone()));
                resync(sid);
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
                        screen.set_value(Some(ClientScreen::new(
                            resp.rows,
                            resp.cols,
                            resp.seq,
                            resp.session_id.clone(),
                        )));
                        session_id.set_value(Some(resp.session_id.clone()));
                        resync(resp.session_id);
                    }
                    Err(e) => error.set(Some(format!("spawn decode failed: {e}"))),
                },
                // Covers both refusals that have a way out: the gate
                // ([policies.terminal] enabled = false) and the cwd jail. The
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
            // NOT a `frame.session_id != mine` test here. `ClientScreen`
            // knows its own id and `apply` answers `WrongSession`; a second
            // copy of that comparison is a second answer to one question, and
            // the copy that drifts is always the one without the tests.
            let outcome = screen.try_update_value(|s| {
                s.as_mut().map_or(ApplyOutcome::Buffered, |s| s.apply(frame))
            });
            match outcome {
                Some(ApplyOutcome::Applied) => repaint_tick.update(|n| *n = n.wrapping_add(1)),
                Some(ApplyOutcome::Gap { .. }) => {
                    if let Some(mine) = session_id.get_value() {
                        resync(mine);
                    }
                }
                // Buffered / Discarded / WrongSession: nothing to draw and
                // nothing to recover. `Discarded` especially must NOT resync --
                // it means a frame we already hold arrived a second time, and
                // re-attaching on it turns a duplicate into a round trip.
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

⚠️ **controller 在派单前查实（2026-08-29），上面两处都改过，两处都是与已落地代码相反的：**

**其一，`pty.resize` 不带 `client_id`。** 原文写的是 `{session_id, rows, cols, client_id}`，
那是 Task 10 **明确否决过**的那条降级路（视口表按调用方自己挑的 id 键控 ⇒ 分级轴由被分级的一方
决定，且断线时没有任何东西释放它）。已落地的 `handle_resize` 从 task-local `current_caller_conn_id()`
取连接身份，`ResizeParams` 只有三个字段。而 serde **默认忽略未知键**——所以多发一个 `client_id`
不会报错，它只是被静默丢掉，然后在 wire 上留下一个看起来在起作用的字段，正好把 Task 10 用一段
doc comment 否掉的那个设计重新讲了一遍。

**其二，挂载路径必须先 `pty.list` 再决定 spawn。** 原文无条件 `pty.spawn`，而**同一份 brief 的
Step 4 里有两条 QA 断言的正好是相反的行为**：

- 第 6 条「**刷新页面 → 屏幕内容原样恢复**（这是服务端持屏的核心收益）」
- 第 9 条「开第二个标签页 → 两个标签页看到同一块屏」

无条件 spawn 之下，刷新拿到的是一个**新 shell 的新提示符**，第二个标签页拿到的是**第二个 shell**。
两条都必然不过。而这不只是两条 QA 失败——**Part 1 的服务端持屏架构会因此没有任何用户可见的收益**：
Task 8 把屏幕搬到服务端、Task 10 为多客户端共享建了 smallest-wins 视口表，而唯一能观测到这两件事
的那个动作（刷新）被客户端自己关掉了。

顺带还有一个不响的代价：旧会话**不会**被关闭（断线只 `release_conn` 释放视口，不 `pty.close`），
所以每刷新一次泄漏一个 shell，直到 `MAX_SESSIONS = 64` 的 FIFO 把最老的踢掉——**一块看不见的
64 个空闲 shell 的天花板**。

**这条判据本身值得记**：*一个「服务端持有状态」的架构，它的全部用户可见收益都压在客户端**重新
找回那个状态**的那一步上；客户端每次无条件新建，服务端那半就等于没做——而两边各自的测试都是绿的。*

⚠️ 关于共享是否安全：`pty.*` 整族在 `ADMIN_PREFIXES` 里（`server/handler.rs:512`），够得到它的
人本来就是 operator、本来就能自己 spawn 一个 shell，所以复用不授予任何他拿不到的东西。这正是
Task 10 的 smallest-wins 视口表所设想的形态，不是它的例外。

剩下三段按同样风格补：

1. **重绘**：一个 `Effect` 读 `repaint_tick`，取 canvas、`render::measure`、`render::paint`。**取 canvas 与测量收进一个私有函数**，不要在 `request_animation_frame` 回调里 `get_untracked()`。

   ⚠️ **在调 `measure` 之前，先修掉 `render.rs` 里那份重复的字号——你是第一个让它承重的调用者。**
   Task 16 的复审（Minor 1，**非实现者的偏差，出自计划本身**）指出：`measure(ctx, font)` 从
   font 串里解析出 px 来算行高，而 `paint` 自己拼
   `format!("{style}{weight}14px 'JetBrains Mono', monospace")`——**字号有两份表述**。
   今天两者相等纯属字面量恰好一致；而你这一步要做 DPR 缩放、并且是第一个真正传 font 串进去的人，
   一旦那个串不是 14px，网格按测出来的尺寸排版、字形按 14px 画，**静默错位且没有任何报错**。

   改成结构性的：让 `CellMetrics` 带着**它是在哪个字号下测出来的**，`paint` 从那里取，
   于是「按没测过的字号画」不再是一种可写出来的代码：

```rust
   pub struct CellMetrics {
       pub width: f64,
       pub height: f64,
       /// The font size, in CSS px, these metrics were measured at.
       ///
       /// Carried rather than re-stated because `paint` builds its own
       /// `set_font` string per run (bold and italic vary per run) and would
       /// otherwise be free to draw at a size the layout was not measured
       /// for -- the grid advancing by one size while glyphs draw at
       /// another, with nothing anywhere to report it.
       pub font_px: f64,
   }
```

   `measure` 把它**已经解析出来**的那个 px 存进去（现在算完行高就扔掉），`paint` 改用
   `format!("{style}{weight}{}px 'JetBrains Mono', monospace", m.font_px)`。

   字体族名仍是一份拷贝——**那个不要动**：`measure` 收的是完整 CSS font 串（调用者的事），
   拆开它去比对族名会得到一个解析器，那才是真的第二个真源。

   `render.rs` 三条既有测试构造 `CellMetrics { width: 8.0, height: 17.0 }`，同批补 `font_px: 14.0`
   ——补 14 让它们**逐字节不变**（同 Task 19 的 `FIXTURE_DIMS` 判据：夹具补什么，决定了现存套件
   会不会静默改变行为）。

2. **resize**：`ResizeObserver`（或窗口 resize 事件）→ `render::viewport_cells` → `rpc_call("pty.resize", {session_id, rows, cols})`。挂载时也跑一次，替换上面写死的 `(24, 80)`。

   ⚠️ **这一步只负责「把我的视口告诉服务端」，不许顺手更新本地 `ClientScreen` 的尺寸。**
   诱惑很自然（我刚要了 40x120，那就把屏幕设成 40x120），但它是错的：尺寸是 **smallest-wins**
   （Task 10），另一个客户端挂着 24x80 时 PTY 就是 24x80，而 `pty.resize` 只回 `{"ok": true}`
   ——**你要到的和你拿到的不是一回事**。本地写入会造出一个恰好在共享场景下出错的第二真源。
   Task 19 之后，尺寸由**帧自己**带回来（`ClientScreen::apply` 采纳 `frame.rows`/`frame.cols`），
   所以这一步**发完就完了**，`ClientScreen::resize` 也因此保持私有。
3. **keydown**：`encode_key` 返回 `Some(bytes)` 才 `prevent_default()` 并发 `rpc_call("pty.input", {session_id, data: BASE64(bytes), base64: true})`；返回 `None` 一律不拦（否则浏览器快捷键全被吞掉）。

**注意 `error` 的展示用 `{move || error.get().map(..)}` 而不是 `<Show>`** —— `<Show when=…>` 的守卫与 body 是两个独立的反应式作用域，body 在信号刚被清空时可以先跑一次新值，把 `expect("visible implies Some")` 变成整页崩溃。单次读 + `Option` 视图没有这个裂缝。

⚠️ **Task 15 落地后的真实签名（controller 2026-08-29 从磁盘核实，与本计划早先的草图不同）：**

- `ClientScreen::new(rows: u16, cols: u16, seq: u64, session_id: impl Into<String>)` —— **四个参数**。
  草图里的三参数版本编译不过。上面两处构造点都已按真实签名改写。
- `ApplyOutcome` 是**五**个变体：`Applied` / `Gap { expected, got }` / `Buffered` /
  `Discarded { seq }` / `WrongSession`。草图里 `Gap` 一个变体同时表示「漏了一帧」和「这帧我已经有了」——
  Task 15 把它拆开了，**这个拆分是承重的**：对 `Discarded` 触发 resync 会把一次重复帧变成一次
  往返，而重复帧在 `finish_attach` 重放缓冲之后是**正常**的。
- `ClientScreen` 自己持有 `session_id` 并在 `apply()` 里比对，所以帧回调里**不要**再写一遍
  `frame.session_id != mine`。
- 访问器：`row_runs(row) -> &[PtyStyleRun]` · `dims() -> (u16, u16)` · `cursor() -> (u16, u16)` ·
  `seq()` · `title() -> Option<&str>` · `alt_screen() -> bool`。Task 16 的 `paint` 用到的那几个
  逐个核对过，签名一致。

**实现时必须遵守的三条**（都是仓库已经踩过的坑）：
1. **`request_animation_frame` 回调里不许 `NodeRef::get_untracked()`** —— 回调晚一帧执行，那一帧足够组件卸载，`get_untracked` 会 unwrap 成整页崩溃。把测量与取 canvas 收进**一个**私有函数，只有一种拼法。
2. **`<Show when=…>` 的守卫与 body 是两个反应式作用域** —— 别在 body 里 `expect("visible implies Some")`；用单次读 + `Option` 视图。
3. **`Err` 不许读作"空屏"** —— `pty.spawn` / `pty.attach` 失败要显示拒绝原因（尤其 `[policies.terminal] enabled = false` 与 cwd jail 的拒绝，两者都是**有出路**的，措辞要说出出路）。

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
10. `[policies.terminal] enabled = false` 后 `config` 热应用 → 在飞会话被杀，新 spawn 被拒且拒绝语说得出怎么打开。

**每条不过就停下修**，不要攒到最后。

- [ ] **Step 5: 提交**

```bash
kill %1
git add interfaces/webchat/src/platform/wide/views/terminal/keymap.rs interfaces/webchat/src/platform/wide/views/terminal/mod.rs interfaces/webchat/src/platform/wide/views/terminal/render.rs
git commit -m "panel: keyboard encoding and the mounted terminal view

encode_key returns None for keys it does not claim rather than the key's
name, or an unhandled F13 types 'F13' into the shell. Ctrl-<letter> is the
letter's alphabet position: getting that arithmetic wrong means the user
cannot stop a runaway process."
```

⚠️ **具名路径，别用目录。** 这棵树上同时有多个 agent 在干活，目录暂存会把别人的在飞工作
一起提交进你的任务（`git add src/` 尤其——它扫得到整棵源码树）。提交前跑一次
`git diff --cached --stat` 并确认它印出来的就是上面这几条，多一条都要停下来查。

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
cargo test -p aleph-protocol pty::   # hyphen: `-p` does NOT accept the underscore
cargo test -p aleph-panel --lib
cargo check -p aleph-desktop-macos
cargo clippy --workspace --all-targets
just wasm
```

**全部必须绿。** 任何一条红就停下修，不要记为"已知问题"——**但下面这两类除外，它们是
controller 实测归类过的，别去修，也别为它们停下**。

**① 六条在 main 上就红的测试。** 本清单里的命令**都是限定范围的**，所以正常情况下你一条也
碰不到；只有当你自己去跑一次全量 `cargo test -p alephcore --lib` 时才会看见：

```
thinker::prompt_budget::tests::{truncate_long_content_preserves_head_tail,
    estimate_factor_tightens_token_gate,
    truncate_marker_intact_when_truncated_count_has_many_digits,
    truncate_marker_with_six_digit_count_stays_within_budget}
thinker::prompt_sanitizer::tests::supplement_does_not_overlap_with_unicode_guard_ssot
gateway::interfaces::whatsapp::wa_outbound::sender::tests::test_send_message_without_client_returns_error
```

归类方式是**测量**不是命名：每一条**单跑也红**，而 `git diff --name-only main...HEAD` 显示本
分支从未碰过 `src/thinker/` 或 whatsapp。另有两条**间歇性**的（`gateway::session_projector::
tests::projector_materializes_events_into_store_with_tokens` 与
`gateway::handlers::chat::tests::visibility_guards::history_serves_the_durable_execution_list`），
八次全量跑里出现一次，同样在本分支没碰过的子系统里。**都不归本计划**，也不要顺手修。

**② clippy 的既存警告。** `cargo clippy --workspace --all-targets` 在这棵树上本来就有警告
（`src/utils/instance_lock.rs`、`src/group_chat/session.rs`、`src/gateway/session_manager/`
等，全是本分支没碰过的文件）。判据不是"零警告"，是**本分支碰过的文件里零新增警告**：

```bash
# 本分支碰过哪些文件
git diff --name-only main...HEAD | grep '\.rs$' > /tmp/touched.txt
# clippy 的命中落在哪些文件
cargo clippy --workspace --all-targets 2>&1 | grep -oE '^\s+--> [^:]+' | sed 's/.*--> //' | sort -u
```
两者取交集，**必须为空**。交集非空才是你的。

⚠️ **`cargo test -p alephcore --lib gateway::pty` 不够，还要跑这一条：**

```bash
cargo test -p alephcore --lib -- gateway::handlers::pty config::live_apply
```

理由是它抓到过一个前者结构上抓不到的缺陷：`config::live_apply` 那条 `close_all` 测试动的是
**进程全局**的 `pty::manager()`，会杀掉 handler 测试正在断言的会话。实测这条命令曾 6 次红 5 次，
而 `gateway::pty` 单跑 6/6 绿、全量 `--lib` 8/8 绿。**一个缺陷可以同时躲过限定命令和全量命令，
只在这两者之间的那条命令下现形**——它现在由 `serial_test` 键 + 源码级 census 挡住，这条命令是
那道守卫的回归测试。

- [ ] **Step 2: 确认熵减清单已执行**

```bash
rg -n '"pty\.output"'     # 零命中
rg -n 'todo!|unimplemented!' src/gateway/pty/ shared/protocol/src/pty.rs interfaces/webchat/src/platform/wide/views/terminal/   # 零命中
rg -n 'BASE64' src/gateway/pty/session.rs   # 只应出现在 pty.input 相关处，或零命中
```

⚠️ **上面第一条带引号，所以它只看得见 Rust 字符串字面量——散文那一半它结构性看不见。**
controller 2026-08-29 已经踩了这一条：`pty.output` 在代码里确实零命中，而**三份文档仍在点名它**
（`src/gateway/CLAUDE.md` 的地雷 J、`docs/reference/SECURITY.md`、`FEATURE_LOCATOR §5.22`），
其中第一份是那条「一条连接有两个方向」判据的经典例子。已修（31f532333），这里留作判据：

```bash
# 散文那一半。plans/ 与 specs/ 是本次改动自己的叙述，正当保留，故排除。
rg -n 'pty\.output' -g '*.md' -g '!docs/superpowers/**'
```

**判据：一个被删掉的标识符，它的其余表述住在散文里；而对一份安全文档，散文正是会被读的那一半。**
⚠️ 修法**不是**把文档里的名字改掉——那三段记的是一个真实发生过的缺陷，改名等于伪造记录。
正确做法是**保住原名 + 补一个指针**说明它现在叫什么、形状变没变。

- [ ] **Step 2b: handler 级接线普查（Task 11 复审的 Major 推广而来）**

Task 11 的复审抓到：**没有任何测试用带 `cwd` 的参数跑过 `handle_spawn`**，所以一个根本不调
jail 的 handler 能通过全部测试——`jail.rs` 的五条测试证明 `resolve_spawn_cwd` 在孤立状态下正确，
而**没有一条证明它被接上了**。那一条已在 Task 11 的修复轮补上。

controller 顺手数了同族（2026-08-29），**还有两个**：

- `handle_input` 的两条测试都是拒绝臂（未知会话 / 坏 base64）。成功路径在 handler 层无测试——
  Task 8 那条 wire 测试走的是 `manager().write(&sid, input)`，**不经过 handler**。
- `handle_close` 是薄委托，**零测试**。

**这一步不是"再补两条测试"，是把这一问逐个回答出来并写进报告**。六个 handler 各一行：

| handler | 证明它被接上了的那条测试 |
|---|---|
| `handle_spawn` | （Task 11 修复轮补的那条） |
| `handle_attach` | |
| `handle_input` | |
| `handle_resize` | |
| `handle_close` | |
| `handle_list` | |

填不出来的格子写 **NONE**，别写"由 X 间接覆盖"——间接覆盖正是这条 Major 的成因
（`resolve_spawn_cwd` 被完美地间接覆盖着，而那个函数不是会坏的那一层）。

判据：**一条测试只有在「把被测的那一步删掉会让它红」时，才算证明了那一步。**
对每个 NONE，报告里给出一句话：删掉这个 handler 里那一步会发生什么、谁会发现。
补不补测试由 controller 按那句话裁定——**这一步的产出是那张表，不是新代码**。

- [ ] **Step 3: 确认没有留下第二条半接的路**

```bash
rg -n 'pty\.' --type rust -g '!*/tests*' src/gateway/method_census.rs
rg -n 'pty' src/gateway/handlers/mod.rs
```

⚠️ **controller 2026-08-29 改写了这一条——原来那版会因为本计划自己的改动而失效，而且失效的方式
是诱导你把一个数字改小。**

原文要求「`handlers/mod.rs` 里 `registry.register("pty...` 的行数 == 6」。但 **Task 12 会把
`pty.spawn` 那一行挪到 `src/bin/aleph-server/.../builder/handlers/` 的 `register_handler!`**
（它需要注入 `Arc<RwLock<Config>>`，而 `registry.register` 只接受 `Fn(JsonRpcRequest)`）。
于是那个 grep 会数出 **5**，而最顺手的「修法」是把 6 改成 5——**从此这条检查会静默接受一个
真正丢失的注册**。一条把数目写在散文里的检查，就是一张会腐烂的名单。

**改用仓库自己已经有的那个答案。** `src/gateway/method_census.rs` 里的扫描器是**源码级**的，
`register(` 与 `register_handler!(` 两种形状它都认（`literal_after_paren` 专门剥掉宏多出来的
receiver 实参），并且它自己就是双向的：census 里有而没注册、注册了而 census 里没有，两边都红。

```bash
cargo test -p alephcore --lib gateway::method_census
cargo test -p alephcore --bins        # bin 里的注册只有这条编译得到
rg -n 'register(_handler!)?\(.*"pty\.' src/gateway/handlers/mod.rs src/bin/    # 人眼过一遍：六个
```

**不要**再手写一个期望数目。要看的是那两条测试绿、以及 `rg` 列出来的六个名字与
`method_census.rs` 里那六行**逐个对得上**。

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
3. `pty.output` **两半**都零命中——代码那半 `rg '"pty\.output"'`，**散文那半**
   `rg 'pty\.output' -g '*.md' -g '!docs/superpowers/**'`。⚠️ 这一条原来只写了带引号的那个，
   而 Step 2 正好花了一段证明它结构性看不见散文——**一条判据在上游被推翻、在下游原样留着**，
   就是这个计划自己反复付账的那一类。三份文档曾在代码零命中的情况下仍然点着它的名。
4. `pty.*` 的注册数与 census 数相等。
5. spec §12（Phase 0 结论）与「Part 1 实施偏差」两节都已写实。
6. `rg 'todo!|unimplemented!'` 在 Step 2 那条命令**列出的那几条路径**下零命中
   （`src/gateway/pty/`、`shared/protocol/src/pty.rs`、
   `interfaces/webchat/src/platform/wide/views/terminal/`）。⚠️ 这一条原来写的是「四个目录」——
   一个写在散文里的**数目**，而命令里是三条路径且其中一条是文件不是目录。跑 Step 2 的那条命令，
   别按这句话去数目录。

**Part 1 交付物明确不含**（不是缺陷，是划线，见上方「Part 1 显式不做的」）：

- **中文 / 日文 / 韩文输入**（IME 归 Part 2）—— 交付时要向用户明说这一条，否则它读起来像 bug。
- **ESC 族转义序列**（`ESC 7`/`ESC 8` DECSC/DECRC、`ESC M` RI 归 Part 2；落回 vte 默认 no-op）—— `less` / `vim` 等全屏程序下可能出现光标位置错位，交付时要向用户明说这一条，否则它读起来像 bug。
- 向上滚动看历史（`pty.scrollback` 归 Part 2；服务端**已经在存**）。
- **会话退出的任何提示**（归 Part 2）—— 服务端发 `pty.exit`，Part 1 的 Panel 不订阅它。用户 `exit` 之后终端只是停止更新，不报错也不变灰。交付时要向用户明说这一条：它比另外两条更像 bug，因为一块不再响应的矩形和一块坏掉的矩形在屏幕上是同一个东西。**不过 Task 17 的 list-then-spawn 顺带给了它一条出路**——刷新一次就换来一个新 shell。⚠️ **但机制不是这里原来写的那个。** 原文说「退出的会话 `closed: true`，复用扫描会跳过它」，controller 2026-08-29 在 Task 11 修复轮里查实**不是**：`session.rs::spawn_reader` 的读线程在 EOF 时先 `closed.store(true)`、紧接着调 `manager().remove(&id)`，而 `remove` 是**从 map 里删掉整条**（`manager.rs:169-174`）。所以退出的会话不是「在列表里且 closed」，是**根本不在列表里**——`list()` 只映射还在 map 里的条目。结论（刷新换来新 shell）成立，路径不同。

  这一条要写进 spec 的「Part 1 实施偏差」，因为它影响两处读者：Task 17 的复用扫描过滤 `closed == Some(false)`，那个谓词在**子进程自己退出**这条路上永远不做事（该会话已被删）；它真正起作用的窗口只有一个——`close_all()` 杀掉会话之后、各读线程还没轮到 EOF 之前，那一瞬列表里确实有 `closed: true` 的条目。**保留那个过滤是对的**（它守的是那个窗口），但别把它读成「退出的会话会以 closed 出现在列表里」。缺口的严重度因此从「永久死掉的矩形」降到「刷新之前死掉的矩形」，交付措辞按后者写。⚠️ 这不是把它修好了：用户仍然不知道**为什么**要刷新，而「刷新一下试试」正是一句会掩盖真缺陷的话。
- Tab 条 / 分屏 / 选区 / 搜索（B 档结构，归 Part 2）。

达成后立刻写 Part 2（Phase 5–8：Tab 条 / 分屏树 / `pty.scrollback` + 滚动 / IME / 选区 / 搜索 / `qa/terminal/run.sh` / FEATURE_LOCATOR 与判据清单补充），引用「Part 1 实施偏差」里记录的真实签名。
