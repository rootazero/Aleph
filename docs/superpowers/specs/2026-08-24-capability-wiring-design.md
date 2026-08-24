# Capability Wiring — 进程级能力句柄的声明、验证与可观测化

- **日期**: 2026-08-24
- **参考**: deepseek-harness (dsh) / Cordis —— `static inject = [...]`「未满足要说出还差什么」
- **分支**: worktree `capability-wiring`（不碰 main）
- **前置裁定**: 本轮**不**重开「架构是否移植」这一问。2026-08-15（§3.1 Round 8）与 2026-08-16（§3.10 round-2）两轮均已裁定 fiber 插件树 / capability-seam 三角 / 包自有 invariant registry 不移植，理由与 HARNESS_PHILOSOPHY §2.3 一致。本轮只取**机制**，且取的是那两轮 DEFER 台账**未覆盖**的一条。

---

## 1. 问题陈述

### 1.1 dsh 的形状

Cordis 里每个插件声明它需要什么：

```ts
static inject = ['agents', 'sessions', 'llm', 'tools', 'systemPrompt']
```

依赖未满足时框架**不激活该插件，并说出还差哪一个**。这是「一切皆插件」真正买到的东西——不是动态加载，是**依赖声明可验证 + 未满足可观测**。

### 1.2 Aleph 的对应物

Aleph 的等价物是**进程级能力句柄**：`static X: OnceLock<Arc<T>>` + `install_x()` + `x()`，由 boot 安装。实测：

所有数字用 §4.1 将要实现的**正确**生产代码提取（大括号配对剔除 `#[cfg(test)]` item）测得，`63 + 46 + 0 = 109` 一致性断言通过：

| 量 | 数字 |
|---|---|
| `src/` 内 install-once 静态量 | **109** |
| 其中纯惰性缓存（只有 `get_or_init`，无 setter） | **63** |
| 其中有 setter 的（＝能力句柄，本轮成员） | **46**（45 install-once + 1 install-then-swap） |
| 　由 boot 直接安装 | **40** |
| 　由其他生产路径安装 | **6**（含 `init_defaults_override` / `init_metrics_runtime` ← `config::load`；`init_cron_trigger` ← 工具构造器；`install_pin_sink` ← `session_model_pin`） |
| 既无 setter 也非惰性 | **0** |
| boot 内安装调用点 | **97** |
| 　其中**条件安装**（`if let` 落空即永不安装） | **20** |

安装侧是一段 **12,861 行手工排序的 boot 脚本**（`src/bin/aleph-server/commands/start/`），**零依赖声明**。

### 1.3 三个已证实的缺口

**缺口 A —— 「没装过」与「装成了这个值」运行时同形。**
已由 §5.22 round-7 就 `spend::install_policy` / `install_ledger` **两个**句柄记录在案：

> 一个 fail-closed 的默认值和一个从未安装的句柄，从外面看是同一个读数。`spend.query` 报 `configured: false`，对「这台机器没配 ceiling」是真话，对「配了但 boot 从没调用过 `install_policy`」也是**同一句**真话。

round-7 的修法是「boot 无条件调用 + 源码 census 钉住这两个调用存在」——**按实例修，不按类修**。46 个成员里覆盖了 2 个，**剩下 44 个未覆盖**。判据：

> **修掉你读到的那几个实例，不等于修掉那一类——而这两者在测试套件里长得一模一样。**

**缺口 B —— 20 个条件安装，落空时无人说得出。**
如 `builder/subsystems.rs:319/348/396`：

```rust
if let Some(ref db) = state_db { ...set_offset_tracker(db); }
// 无 else 臂 —— 句柄永不安装，且没有任何地方记下这件事
```

**缺口 C —— 消费者各自解释 `None`。**
`global_session_service()` 有 **9 个生产消费者**，各自决定 `None` 是什么意思——`src/tools/scoped/dispatch.rs:1120` 直接静默 `return`。判据：**fail-soft 的跳过不是「不存在」的证据**。

### 1.4 前置缺陷：census 惯用法自身的盲区

仓内 20+ 条源码级 census 守卫用 `src.split("#[cfg(test)]").next()` 取「生产前缀」。这不是分割点，是**第一个测试属性出现的位置**。实测 1,734 个含该标记的文件：

| 类别 | 数量 | 后果 |
|---|---|---|
| 尾部 `mod tests {`，其后无生产代码 | **1,458** | prefix-split 正确 |
| 顶部 `#[cfg(test)] mod tests;` | **73** | **整份文件被丢弃** |
| 首个标记是文件中段的测试项 | **203** | **任意截断** |

**276 / 1,734 = 16% 的文件对每一条 prefix-split 守卫结构性不可见。**

实例：`src/utils/paths.rs:90` 的 `#[cfg(test)] pub(crate) static ALEPH_HOME_TEST_GUARD` 位于文件 5% 处 ⇒ 该文件 95% 的生产代码不可见。`src/spend/mod.rs` 的 `#[cfg(test)]` 在第 2,024 字节（文件 30,470 字节）⇒ round-7 的两个锚点句柄住在盲区里。

⚠️ **本 spec 自身的量具三次受此影响，这是 Phase 1 最强的论证。** 「能力句柄有多少个」被量了三遍，每一遍都因为量具而偏低：

| 第几遍 | 量具 | 结果 |
|---|---|---|
| 1 | 裸类型名正则 + `split("#[cfg(test)]")` 前缀 | 29 boot / 42 有 setter |
| 2 | 接受全限定路径 + 同一个截断前缀 | 38 boot / 42 有 setter |
| 3 | 全限定路径 + **大括号配对剔除**（＝ §4.1 的算法） | **40 boot / 46 有 setter** |

第 1→2 遍的差是「守卫认得几种形状」；第 2→3 遍的差正是本节要修的那个盲区。**§1.2 的数字是第 3 遍的**。Phase 3 的守卫用同一算法重新计数，与该数字不符时**调查**，不静默接受。

同族判据（已在 CLAUDE.md 记录，本条是其反向形态）：

> 源码级守卫里用 `\n` 锚定的分隔符，在 CRLF 检出上匹配不到任何东西 ⇒「生产前缀」变成整份文件。

那条讲**前缀过长**，本条讲**前缀过短**。

---

## 2. 非目标（State the Negative）

1. **不拆 crate**。`alephcore` 仍是单一 966k 行 crate。「薄核心」在本轮仍是运行时概念。
2. **不建依赖图 / 拓扑启动**。boot 仍是手工排序脚本；本轮只让它**没做成的事说得出口**，不改它做事的顺序。
3. **不动 `src/harness/`**。前四轮对照已证明可零改动；`budget.rs::CEILING` 不动。
4. **不改 60 个惰性缓存的任何一行**。规则把它们排除在成员之外。
5. **不替消费者决定 `None` 的含义**。缺口 C 的 9 个消费者各自的语义可能都是对的；本轮让「为什么没装」可见，逐个裁定留给 Phase 5，不打包重写。
6. **不新增任何客户端 surface**。doctor 工具面与 `diagnostics.run` RPC 共用同一 battery，四张脸自动继承。
7. **不引入 `linkme` / `inventory`**。R3（不为单一功能引第三方库）+ link-section 魔法在部分 target 不可靠。花名册成员由源码 census 强制。
8. **不预先承诺缺陷数目**。Phase 1 让 276 个文件第一次被现存守卫看见；红出多少现在不知道。

---

## 3. 架构

四层，自下而上：

```
Phase 1  utils::source_scan          ← 前置：修好量具
Phase 2  capability::CapabilitySlot  ← 声明 + 盖戳（同一个动作）
Phase 3  census 守卫（两条）          ← 完整性：规则不是名单
Phase 4  diagnostics::checks::capability_wiring ← 可观测：三态
Phase 5  逐条裁定 Phase 1/3 红出来的东西
```

依赖方向单向：Phase 3 消费 Phase 1 与 2；Phase 4 只消费 Phase 2。符合 P1/P4。

---

## 4. 组件设计

### 4.1 Phase 1 — `src/utils/source_scan.rs`（新，~120 行）

```rust
/// The production half of a Rust source file, for source-level census guards.
///
/// NOT a prefix cut. Removes each `#[cfg(test)]`-attributed *item* by brace
/// matching (and `#[cfg(test)] mod tests;` declarations), keeping everything
/// else — including production code that follows a mid-file test item.
///
/// CRLF-safe: normalizes `\r` before scanning. A `\n`-anchored separator
/// matches nothing on a CRLF checkout, and this repo has paid for that once.
pub fn production_prefix(src: &str) -> String;

/// Orthogonal and composable. Most guards want BOTH: a doc comment naming a
/// symbol is documentation, not a call site.
pub fn strip_comment_lines(src: &str) -> String;
```

**算法**：逐行扫描，遇 `#[cfg(test)]` 属性行 →
- 下一非空行是 `mod <ident>;` ⇒ 跳过两行；
- 下一非空行以 `{` 结尾或是块起始 ⇒ 大括号配对跳过整个 item（尊重字符串字面量与行注释内的括号）；
- 否则（单行 item，如 `pub(crate) static X: ... = ...;`）⇒ 跳到该语句的 `;`。

其余行原样保留。

**三条自保守卫**（每条手工证伪一次）：

| 守卫 | 断言 |
|---|---|
| `production_prefix_matches_the_old_split_where_the_old_split_was_right` | 对 1,458 个尾部测试模块文件，输出与 `split("#[cfg(test)]").next()` **逐字节相同** |
| `production_prefix_recovers_code_the_old_split_discarded` | 对受影响文件输出**严格更长**；命中文件数与当前实测值（**276**）比对，缩水即红 |
| `no_module_hand_rolls_the_cfg_test_prefix_cut` | `src/` 内不再出现 `split("#[cfg(test)]")` / `find("#[cfg(test)]")` 等手抄形态（规则，不是豁免名单） |

**迁移**：20+ 处调用点改调 `production_prefix()`。**这会让 276 个文件第一次被现存守卫看见** —— 新暴露面，不是回归。

### 4.2 Phase 2 — `src/capability/mod.rs`（新，~250 行）

```rust
/// What a read observes when this capability was NEVER installed.
///
/// ⚠️ Membership in the roster is decided by THIS — the failure direction —
/// not by the handle's type or its name. A handle belongs iff losing it
/// yields a *wrong answer* rather than a crash. The 63 lazy caches in `src/`
/// cannot write an honest variant here ("not built yet" is not a wrong
/// answer), which is why the rule excludes them by derivation rather than by
/// a hand-written exclusion list.
pub enum MissingSemantics {
    /// A read yields a legal-looking value and no caller can tell.
    /// (`spend` policy reads as "no ceiling" — the §5.22 round-7 shape.)
    IndistinguishableDefault { reads_as: &'static str },
    /// A read yields `None` and every consumer decides for itself what that
    /// means. (`GLOBAL_SESSION_SERVICE`: 9 consumers, one silently returns.)
    ConsumerDecides,
    /// Fails closed — safe, but the feature is dead and says nothing.
    FailsClosed,
    /// Fails OPEN — a gate silently stops gating. Highest severity.
    FailsOpen,
}

/// Why a slot is not installed, when boot got far enough to decide.
pub enum Outcome {
    Installed,
    /// Boot reached this slot and declined it. THIS is Cordis's
    /// "waiting for: <dep>", in Rust's shape.
    Declined { because: &'static str },
}

/// Install-once capability handle. Replaces bare `static X: OnceLock<Arc<T>>`.
///
/// `install()` writes the value AND stamps the roster in one act: a caller
/// that cannot reach the inner `OnceLock` cannot forget the stamp. This is the
/// `MetaGuard` idiom (make the correct thing the only constructible thing),
/// not a "remember to call mark()" discipline — the latter fails in exactly
/// the shape this type exists to prevent, and its failure is a *confident
/// lie* ("not installed" about an installed handle), which is worse than
/// today's silence.
pub struct CapabilitySlot<T> { /* id, missing, OnceLock<Arc<T>>, OnceLock<Outcome> */ }

impl<T> CapabilitySlot<T> {
    pub const fn new(id: &'static str, missing: MissingSemantics) -> Self;
    /// Returns false when already installed (idempotent, like today's setters).
    pub fn install(&'static self, v: Arc<T>) -> bool;
    /// Record that boot reached this slot and could not install it.
    pub fn decline(&'static self, because: &'static str);
    #[inline] pub fn get(&self) -> Option<Arc<T>>;
    pub fn status(&self) -> Option<&'static Outcome>;
}

/// Install-once-then-live-swap. Exactly one member today:
/// `spend::GLOBAL_POLICY` (`OnceLock<ArcSwap<SpendPolicy>>`).
/// `update()` returning false when never installed is an EXISTING contract
/// (`spend::update_policy` feeds the live-apply verdict's Restart downgrade)
/// and is preserved byte-for-byte.
pub struct MutableCapabilitySlot<T> { /* … */ }

impl<T> MutableCapabilitySlot<T> {
    pub const fn new(id: &'static str, missing: MissingSemantics) -> Self;
    pub fn install(&'static self, v: T) -> bool;
    pub fn update(&'static self, v: T) -> bool;   // false ⇒ never installed
    pub fn decline(&'static self, because: &'static str);
    #[inline] pub fn load(&self) -> Option<Arc<T>>;
}

/// The roster. Membership is a hand-written list whose completeness is
/// enforced by the Phase-3 census (a new `CapabilitySlot::new(` that is not
/// listed fails BY ID). Deliberately not `linkme`: see non-goal 7.
pub static ALL_SLOTS: &[&'static dyn SlotStatus] = &[ /* … */ ];
```

**热路径等价性**：`get()` 是 `#[inline]` 转发到内部 `OnceLock::get()`；戳只写在 `install()` / `decline()` 侧。守卫 `slot_get_is_a_bare_oncelock_read` 断言 `get()` 体内不含分支或原子写。

### 4.3 Phase 3 — census 守卫（`src/capability/census.rs`）

**成员规则（推导，非名单）**：

> `src/**` 内一个 install-once 容器（`OnceLock` / `OnceCell` / `ArcSwap*`，**接受全限定路径**）的 `static`，**如果存在写它的 setter**（`set` / `store` / `swap` / `get_or_try_init`，而非仅 `get_or_init`），它就是能力句柄。

⚠️ 正则必须吃 `std::sync::OnceLock` / `once_cell::sync::OnceCell` / `tokio::sync::OnceCell` / `arc_swap::ArcSwap`。只认裸名的第一版量得 29 而真数是 40，**而 round-7 的锚点恰好写的全限定形式**。判据：**一条守卫的绿，只覆盖它的块识别器认得的那种形状。**

| 守卫 | 断言 | 失效时 |
|---|---|---|
| `every_installed_global_is_a_capability_slot` | 规则选中的每个 static 都是 `CapabilitySlot` / `MutableCapabilitySlot` | 按 `文件:变量名` 红 |
| `every_declared_slot_is_in_the_roster` | 每个 `CapabilitySlot::new(` / `MutableCapabilitySlot::new(` 的 id 都在 `ALL_SLOTS` | 按 id 红 |

两条都：经 `production_prefix()` + `strip_comment_lines()`（吃自己的狗粮）；带**自保计数**断言（「这一轮到底扫了几个」——清单缩水与守卫失效在报告里同形）；**手工破坏一次**验证其红且点名。

### 4.4 Phase 4 — `src/diagnostics/checks/capability_wiring.rs`

新检查 `core/capability-wiring`，加入 `DiagnosticEngine::default_registry()`。

**进程真相是本节的全部难点。** `aleph-server doctor` 在**冷进程**里建 registry（句柄全空）；`diagnostics.run` 跑在 **daemon**（句柄是活的）。同一份检查，两个进程，两种真相。既有先例已记录该风险（`ext/idle-extensions` 的 doc：「否则两个 doctor 会对同一台机器给出不同答案」）。

判据用**已存在的** `gateway::shutdown_forensics::BOOT_INSTANT`（`mark_boot()` 在 `start/mod.rs:73`，argv 解析后第一件事）：

| `BOOT_INSTANT` | 花名册 | 结论 |
|---|---|---|
| 未设 | — | **`Severity::Info`**：「本进程没有跑过 boot，这一问要问 daemon」—— **绝不报 pass** |
| 已设 | 完整 | pass（零 Finding） |
| 已设 | 有缺口 | 逐 slot 一条 Finding |

第三行是免费拿到的额外价值：`mark_boot` 在 boot **开头**、能力安装在其后 ⇒「boot 起过但花名册不全」是一个此前无法观测的真实故障态（boot 中途失败 / 早返回）。

判据：**「未知」不许读作「健康」**；**只有 `Ok` 有资格断言被读的那个东西。**

**Severity 从 `MissingSemantics` 推导，不手写**：

| `MissingSemantics` | `Severity` | 理由 |
|---|---|---|
| `FailsOpen` | `Error` | 一道闸静默停止把关 |
| `IndistinguishableDefault` | `Warning` | round-7 形状：真话藏着假世界 |
| `ConsumerDecides` | `Warning` | 每个消费者各自发明一个答案 |
| `FailsClosed` | `Info` | 安全但功能已死 |

`Outcome::Declined { because }` 的理由进 `Finding::detail` —— 这是用户真正要的那句话：不是「session 服务缺失」，而是「未安装，因为 `[gateway] state_db` 未设置」。`repairable: false`（接线不可运行时机械修复）；`fix_hint` 指向具体配置键。

**零新增 surface**：`doctor` 工具面（R8）与 `diagnostics.run` RPC 共用同一 battery，Panel / CLI / TUI / 模型四张脸自动继承，不写一行客户端代码。

### 4.5 Phase 5 — 逐条裁定

三组输入，逐条 CONNECT / CUT / 上报，**不打包**：

1. Phase 1 让 276 个文件第一次可见后，现存守卫红出来的每一条；
2. 20 个条件安装：每一个要么改成无条件安装（round-7 `install_policy` 的形状，附理由 doc），要么补 `decline(because)` 臂；
3. 缺口 C 的消费者：`decline` 可见后，逐个判断该消费者的 `None` 处置是否仍然正确。

---

## 5. 迁移与熵减

| 位置 | 动作 |
|---|---|
| 20+ 处 `split("#[cfg(test)]")` | **删除**，改调 `production_prefix()`；`src/` 内不留第二份实现（守卫钉住） |
| 46 处裸 install-once `static` | **替换**为 `CapabilitySlot` / `MutableCapabilitySlot` |
| 对应 `set_* / init_* / install_*` | **保留为 2 行 `#[inline]` 包装**，转调 `slot.install()` |
| 对应 `global_*() -> Option<Arc<T>>` | **保留为 `#[inline]` 包装**，转调 `slot.get()` |
| 迁移后无调用者的 setter/accessor | **删除**（`clippy --all-targets` + `--no-run` 点名） |

保留包装函数是刻意的：`global_session_service()` 有 9 个消费者、`PII_ENGINE` 在每条输出上——改存储不改调用点，diff 收敛在 46 个声明点所在的文件内。新代码想绕过 `CapabilitySlot` 直接写裸 `OnceLock`？Phase 3 的 census 按名字红。

---

## 6. 验证

**最小可信集**（CLAUDE.md §10；`cargo check` 的绿只验证了仓库一小半）：

```
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run
cargo check -p aleph-desktop-{macos,windows,linux}
cargo clippy --all-targets
cargo test -p aleph-tui -p aleph-cli          # 客户端 crate 不在最小集里
```

**每条新守卫手工证伪一次。** 变异分类器按「这句话只可能由哪一种结局打印」排序：

```
running 0 tests          ⇒ VACUOUS
test result: FAILED      ⇒ RED
test result: ok          ⇒ GREEN
剩下的（无 test result: 行） ⇒ BUILD-ERROR
```

⚠️ cargo 对测试失败也打 `^error:`、对 RED 也打 `0 passed` —— 这两个坑各踩过一次。**红的条数比预期少时，先怀疑自己的判断，不是守卫。**

**真机验证 doctor 三态**（本机构建本机测试）：

1. 起 daemon → `aleph doctor --json` → 断言 `core/capability-wiring` 出现且 pass；
2. 冷进程 `aleph-server doctor --json` → 断言**同一 id 出 `Info` 而不是 pass**；
3. 人为触发一次 `decline` → 断言 `detail` 里印出了那句理由（不是泛化措辞）。

---

## 7. 风险与边界

| 风险 | 缓解 |
|---|---|
| 46 处热路径句柄改写引入性能回归 | `get()` 是 `#[inline]` 裸转发；守卫断言其体内无分支/原子写 |
| Phase 1 红出的缺陷数不可预知，分支膨胀 | 逐条裁定，不打包；超出可审阅规模时把剩余部分**点名**移交下一轮（而不是留一张无人排空的豁免清单） |
| `MutableCapabilitySlot` 只有一个成员，疑似 YAGNI | 它替换的是**既有**的 `spend::GLOBAL_POLICY`，非为未来预留；若迁移后发现可并入 `CapabilitySlot`，按 R10 撤回 |
| 冷进程 doctor 的 `Info` 被读者当成故障 | `title` / `detail` 明说「这一问要问 daemon」，并给出 `aleph doctor`（走 RPC）的具体命令 |

**已声明的未处理边界**：

- `production_prefix()` 的大括号配对不解析宏展开内部；宏体内的 `#[cfg(test)]` 项不在覆盖范围内（当前实测 `src/` 内无此形态，守卫的自保计数会在出现时反映为计数变化）；
- 本轮不覆盖 `interfaces/` / `shared/` / `desktop/` 内的同类句柄（`alephcore` 之外），范围外。

---

## 8. 与既有裁定的关系

- **不违反** §3.1 Round 8 / §3.10 round-2 的「架构不移植」：本轮不引入插件树、不引入 capability-seam、不引入包自有 invariant registry。
- **不与 18 条 DEFER 台账重叠**：逐条比对，`⑼ 跨类后台工作统一注册表` 是最接近的一条，但那条讲的是 subagent/bash/cron 三类**运行时工作**的生命周期语义，与进程级**能力句柄安装**无关。
- **延续** §5.22 round-7 的判据，把它从 2 个实例推广到那一类。
