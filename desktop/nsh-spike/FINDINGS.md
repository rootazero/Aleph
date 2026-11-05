# NSH Spike Findings

## Task 1: Spike crate skeleton + NSH compile verification

- **状态**: ✅ cargo build exit 0
- **NSH commit**: `2ab3b6bc27f151afd5d932c63746a8b4b15a00e9` (describe: `v0.4.0-24-g2ab3b6bc`)
  - 注：本地 clone 超出精确 tag v0.4.0 24 commits。Phase 1 正式实施时需锁定精确 commit。

### API 偏差（设计文档 §4.3 需更新）

| 设计文档假设 | 实际 API | 影响 |
|---|---|---|
| `type Root = Box<dyn Fn() -> AnyElement>` | `Root` 是 struct，通过 `Root::of::<C>()` 构建（`nsh_core/src/element/component.rs:535`） | 低风险——编译器直接报错；改为 `Root::of::<App>()` 即可 |
| `AppOptions::default()` 可能需要显式字段 | `AppOptions::default()` 可直接使用，所有字段有默认值 | 无影响，简化了初始化 |
| `#[component]` 宏生成函数 | 实际生成类型（struct），不能 `App()` 调用，要用 `Root::of::<App>()` | 低风险——编译器报 E0618 明确指示 |

### 必要的结构修复

- **nested workspace**: spike crate 需要本地空 `[workspace]` 段，否则被 Aleph 根 workspace 拒绝（exit 101）。
  已修复：在 `Cargo.toml` 顶部加 `[workspace]`（空 members）。根 `Cargo.toml` 零修改。

### 依赖验证

- `nsh-engine` features: `[]`（空）——QuickJS/nsh-js 未引入 ✅
- 无 `nsh-js`、`rquickjs`、`quickjs` resolved 节点 ✅
- 根 `Cargo.toml`/`Cargo.lock` diff = 0 ✅
- Mach-O arm64 二进制含字符串 `NSH Spike: compiles OK` ✅

### objc2 版本冲突风险（Task 1 验证结果）

- NSH 依赖 `objc2 0.6.5`（workspace 锁定）
- Aleph `desktop/shell` 用 `objc2 =0.6.5`（精确钉版）
- Aleph `desktop/macos` 用 `objc2 0.6`
- 三者版本完全兼容，**无冲突** ✅（之前设计文档标为「高风险」可降级为「低风险」）

### tokio 版本（待 Task 3 验证）

- NSH 用 `tokio = "1"`（semver 宽松）
- Aleph 用 `tokio = "1.53"`
- 理论上兼容，Task 3 实测确认。

## Task 2: 待填写

## Task 3: 待填写
