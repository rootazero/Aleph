# runtimes review — 2026-10-01

## 概览
- 模块规模：11 文件，6343 行
- 角色（graphify-out/GRAPH_REPORT.md）：runtime 安装、版本探测、能力注册、ledger 簿记、GitHub release/npm global 安装的全栈子模块；是 alephcore 的"运行时供给层"
- 估计节省：约 15–20 行可精简（主要是 to_string_lossy 冗余链），REGEX_CACHE 可省约 5 行

---

## P0 — 安全/阻塞（无）

---

## P1 — 逻辑/正确（无高置信度 P1）

---

## P2 — 逻辑/正确

- **`src/runtimes/ledger.rs:296` — `to_string_lossy().to_string()` 冗余分配（置信度 95%）**
  - 现象：`std::env::join_paths` 返回 `PathBuf`，`to_string_lossy()` 将 `OsString` 转 `Cow<str>`（分配），再 `.to_string()` 在 `Cow<str>` 上**再次无条件分配**（忽略可能已经 borrowed）
  - 证据：同一文件 301 行、github_release.rs:1625 有完全相同模式；`Cow<str>::to_string` 永远分配新 String，无论 `Cow` 实际是 `Borrowed` 还是 `Owned`
  - 建议：`join_paths` 返回 `PathBuf` → `.to_string()` 在 valid UTF-8 路径上直接返回 `String`，无需 lossy 包装。若需 lossy 安全网，用 `.to_string_lossy().into_owned()` 语义更清晰。实际路径几乎必然是 valid UTF-8，删除 lossy 层也完全安全

- **`src/runtimes/github_release.rs:1625` — 同上，测试中的文件名提取（置信度 95%）**
  - 现象：`std::fs::read_dir` → `file_name()` → `to_string_lossy().to_string()`
  - 证据：与 ledger.rs:296 完全相同的反模式
  - 建议：同上；测试 fixture 路径名同样是 ASCII，`.to_string()` 即可

- **`src/runtimes/ledger.rs:301` — 同上（置信度 95%）**
  - 现象：`paths.iter().map(|p| p.to_string_lossy().to_string())` 在 fallback 分支中
  - 证据：fallback 在 `join_paths` 失败时触发（跨平台路径拼接），路径名仍是 valid UTF-8
  - 建议：同上

- **`src/runtimes/github_release.rs:704` — `ReleaseSource` 字段公开可写（置信度 85%）**
  - 现象：`pub struct ReleaseSource { pub api_host: String, pub asset_host: String }` — 两个字段均 `pub`，无构造后不变量保护。`for_runtime` 构造时设定了 `api_host = DEFAULT_DOWNLOAD_HOST` 的不变性，但任何外部调用方可以 `ReleaseSource { api_host: "http://evil".into(), asset_host: ... }` 绕过
  - 证据：grep 全仓库，`for_runtime` 是唯一构造点（bootstrap.rs:453），但 struct 本身无 `#[non_exhaustive]`；`pub` 字段意味着任何子模块或测试都能绕过
  - 建议：将两个 `pub` 字段改为 `pub(crate)` 或 `pub(super)`，迫使所有构造经 `for_runtime`（或 `#[cfg(test)]` 下允许公开用于 fixture）

---

## P3 — 复杂度/性能（3 条）

- **`src/runtimes/probe.rs:411-426` — `get_compiled_regex` 的 `LazyLock`+`Mutex` 双保险多余（置信度 80%）**
  - 现象：`static REGEX_CACHE: LazyLock<Mutex<HashMap<&'static str, Regex>>>`，但 `LazyLock` 本身已保证初始化只执行一次（编译期保证）。内部 `Mutex` 仅保护 `get_or_insert` 的并发竞争；若去掉 `Mutex`，两个并发首调会各编译一次 regex（第二次的 `insert` 被 `HashMap` 内部拒绝），但 regex 编译是幂等的。`#[allow(excessive_clone)]` 标注两处 `.clone()`（regex::Regex 不实现 Clone——`insert(re.clone())` 编译是因为 `once_cell::sync::Lazy` 的 initializer 在每次调用中运行，re 在 initializer 作用域内只创建一次）
  - 建议：去掉 `Mutex<HashMap>`，改为 `LazyLock<HashMap<&'static str, Regex>>`，用 `get().cloned()` 或 `get().copied()`（若 regex Regex 支持）。省去两处 `#[allow]` 注释

- **`src/runtimes/ensure.rs:40` — `Arc::new(Mutex::new(())).clone()` 的 `#[allow(excessive_clone)]`（置信度 85%）**
  - 现象：`capability_lock()` 返回 `Arc<tokio::sync::Mutex<()>>`，`.clone()` 仅为 bump refcount（heap ptr copy），注释自己也说"small fixed list"。但 `Arc::clone` 的语义是"共享所有权"，而非"复制数据"——clippy 标注 `excessive_clone` 的意图是检测"不必要的数据复制"，而非 refcount bump。此标注是误报
  - 建议：移除 `#[allow(excessive_clone)]` 注释；在 Rust 中 `Arc::clone` 是惯用 idiom，不需要 lint 抑制。若 clippy 版本对此有异议，检查是否来自 `clippy::clone_on_ref_ptr`（对应 `Arc::clone` 的正确 lint）

- **9 处 `excessive_clone` 标注（ledger×3、ensure×3、probe×3）— 架构气味，非代码错误（置信度 90%）**
  - 现象：5 个文件共 9 处 `// rust-doctor-disable-next-line excessive-clone`，均落在 `PathBuf::clone()` 或 `Arc::clone()` 上。`PathBuf::clone()` 是 `Vec<u8>` 的堆数据复制（PathBuf 是 `OsString` wrapper，内含 `Vec<u8>`）；`Arc::clone()` 是 refcount bump
  - 证据：ledger.rs:181（`entry.name.clone()` 是 String copy，但 `entry` 被 `insert` 移入 HashMap，name 必须 clone）、ledger.rs:278,386（迁移旧账本时 JSON value → String 转换），ensure.rs:146,235（`bin_path.clone()`），probe.rs:198（`prepended.push(cand.clone())`）
  - 建议：`PathBuf::clone()` 不可省（必须 ownership move）；`Arc::clone()` 是 Rust idiom。架构上可以考虑让 `CapabilityEntry` 持有 `&'static Path`（来自预定义的静态 PathBuf 表）以消除 `PathBuf` clone，但这需要更大的重构。这些标注应降级为"架构待办"而非 per-site 抑制

---

## 误报自检

| 曾考虑 | 放弃理由 |
|--------|----------|
| `ReleaseSource::for_runtime` 的 `api_host` 固定 `DEFAULT_DOWNLOAD_HOST` 而非从配置读取 | 代码注释（ll.716-721）明确说明这是**设计决策**：checksum 必须来自 provenance 而非 mirror；这是一个已知不变量，不是遗漏 |
| `MirrorKey::read: fn(&Config) -> Option<String>` 不导出 | `MirrorKey` 唯一消费者是 `configured_host`（ll.547），内部细节；不导出是正确的封装 |
| `strategy_supported_here` 中的 `asset(os, arch).is_some()` 对 `GithubRelease` 每次线性搜索 | `SPECS` 是 `&'static [&RuntimeSpec]`，长度约 10；线性搜索 10 个元素的常量表 O(10) 可忽略 |
| `github_release.rs` 1895 行无内部模块拆分 | 该文件是 `ReleaseSource`（含配置读取）+ `install_release`（含 download+extract）+ 完整 fixture server，每块都围绕同一数据类型的有状态操作；拆分反而增加间接性 |
| `NpmGlobal` variant 缺少 `env_var_name` 字段（早期设计有，specs.rs 注释可推断） | grep 全仓库无任何 `env_var_name` 引用；`npm_global::prefix` 是唯一路径来源，正确 |

---

## 未审查清单（盲区明示）

1. **`post_install.rs` 的 `run_cmd_with_timeout` 超时机制**：涉及 subprocess 持有 tokio task 的生命周期，未深入审查
2. **`github_release.rs` 的 HTTP 重定向跟随逻辑**：reqwest 默认行为，`Follw` 状态机细节未读
3. **`bootstrap.rs` 的 PATH read-modify-write 竞争**（`PATH_LOCK` 注释了存在性但未验证正确性）：`std::env::set_var` 的 thread-safety 边界
4. **`ensure.rs` 递归深度 `depth + 1` 的上限保护**：存在 `MAX_RECURSIVE_DEPTH` 之类吗？未搜
6. **`probe.rs` 的 `enrich_path_for_reprobe`** 的跨平台路径拼接边界情况（Windows PATH separator）
7. **`github_release.rs` 的 fixture `start_declaring`/`start` server** 的 graceful shutdown 时序：tasks join on drop，但 listener close 后最后几个 accept 的任务可能逃逸