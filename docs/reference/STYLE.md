# Code & Language Style

> rustfmt / clippy / 命名 / 错误处理 / 不变性 / Python 工具链的工程约定。**Tier 1 只放指针**；详情在这里。

---

## Code Style

**rustfmt** (4-space indent, 100 char width) + **clippy** (`-D warnings`).

| Item | Convention |
|------|-----------|
| Modules/functions/variables | `snake_case` |
| Types/traits/enums | `PascalCase` |
| Constants | `SCREAMING_SNAKE_CASE` |
| Visibility | Default private, `pub(crate)` for internal sharing |

**Error handling**: Libraries use `thiserror`; applications use `anyhow`. Use `?` for propagation. Never `unwrap()` in production.

**Immutability**: Variables are immutable by default. Use `let mut` only when required.

工程规范全文 → [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md) · [DESIGN_PATTERNS.md](docs/reference/DESIGN_PATTERNS.md)

---

## Python Toolchain

- **不要安装系统级 Python** —— Agent 不要触发系统 Python 的安装/调用；机器上已有的 system `python` / `python3`（WindowsApps stub）**不可靠**（已知该 stub 退出码 49、调用即失败）。
- **确实需要 Python 时，用 [`uv`](https://docs.astral.sh/uv/) 搭虚拟环境**：

  ```bash
  # 一次性：建项目级 venv
  uv venv .venv
  uv pip install -p .venv/Scripts/python.exe <pkg>   # Windows
  # uv pip install -p .venv/bin/python   <pkg>       # Unix
  .venv/Scripts/python.exe script.py                  # Windows
  ```
- **跨平台脚本优先 Node.js**（`node` 在 PATH 中稳定可用），再考虑 `.venv/Scripts/python.exe`。
- **不要在 `D:/Workspace/Aleph/`** 落 `python.exe`、`.python/`、系统级 site-packages；所有 venv 落在仓库子目录（如 `.venv/`），并已在 `.gitignore` 内。
