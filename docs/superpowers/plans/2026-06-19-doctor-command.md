# `/doctor` 检测命令 + 「按 f 维修」提醒 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 注册一个走 AI 回路的 `/doctor` 只读检测命令，发现问题时让 LLM 在 webchat 面板里提醒用户「输入 f 开始维修」。

**Architecture:** `/doctor` 完全镜像现有 `f` 热键流——webchat composer 客户端拦截 `/doctor`、换成只读检测 prompt、走正常 LLM 发送管线；LLM 调 `doctor(fix=false)` 写健康报告。一个 WebRich-gated 的 prompt 层（`DoctorRepairHintLayer`）让 LLM 在发现未解决问题时结尾提醒按 `f`。`f` 修复流不动。

**Tech Stack:** Rust · `alephcore`（thinker prompt 层）· `aleph-panel`（Leptos/WASM webchat）。

设计文档：`docs/superpowers/specs/2026-06-19-doctor-command-design.md`

## Global Constraints

- **R1/R4**：「按 f」提示只在 webchat 面板成立，**严禁**进 Core 的 `doctor` 工具输出；提示层必须 gate 在 `InteractionParadigm::WebRich`。
- **R7/R9**：检测推理与提醒措辞全交 LLM；代码只提供 affordance，不做语义判断。
- **R10**：**不碰 `src/harness/`**；改动只落 `src/thinker/layers/` 与 `interfaces/webchat/`。
- **`f` 修复流不变**：`hotkey.rs` / `repair_pulse` / `DOCTOR_REPAIR_PROMPT` 一行不动。
- **cargo 节制**：测试命令一律带 `-p <crate> --lib` 并尽量指定测试名，不跑全量。
- 代码注释用英文；提示文案用英文（与现有层一致）。

---

## File Structure

| 文件 | 职责 | 改动 |
|------|------|------|
| `src/thinker/layers/doctor_repair_hint.rs` | WebRich-gated「按 f」提示层 | **新建** |
| `src/thinker/layers/mod.rs` | 层模块声明与导出 | +`mod` +`pub use` |
| `src/thinker/prompt_pipeline.rs` | 层注册表 + priority 文档 + 计数测试 | +1 注册 +1 注释 +计数 42→43 |
| `interfaces/webchat/src/views/chat/composer/palette.rs` | slash 命令纯逻辑（可宿主测试） | +常量 +2 纯函数 +测试 |
| `interfaces/webchat/src/views/chat/composer/mod.rs` | composer 编排（send/enqueue/fetch 接线） | +use +3 处接线 |

---

## Task 1: `DoctorRepairHintLayer` 提示层（alephcore）

**Files:**
- Create: `src/thinker/layers/doctor_repair_hint.rs`
- Modify: `src/thinker/layers/mod.rs`（`mod voice_mode;` :64 附近 + `pub use voice_mode::VoiceModeLayer;` :129 附近）
- Modify: `src/thinker/prompt_pipeline.rs`（import 块 :7-16、priority 文档 :298 后、`default_layers()` :350 后、计数测试 :562 区）

**Interfaces:**
- Consumes: `PromptLayer` trait、`LayerInput`、`AssemblyPath`、`LayerStability`、`PromptMode`、`InteractionParadigm::WebRich`、`ResolvedContext.environment_contract.paradigm`（均已存在，模板见 `src/thinker/layers/voice_mode.rs`）。
- Produces: `pub struct DoctorRepairHintLayer`（单元结构体，priority 1120）。

- [ ] **Step 1: 写新层文件（含失败测试）**

Create `src/thinker/layers/doctor_repair_hint.rs`：

```rust
//! `DoctorRepairHintLayer` — webchat-only "press f to repair" hint (priority 1120).
//!
//! Closes the detect→repair loop for the `/doctor` slash command: when the
//! model runs the read-only `doctor` tool and finds unresolved problems, this
//! layer asks it to end the reply by reminding the user they can press `f` to
//! start automatic repair. Gated to the WebRich paradigm because `f` is a
//! webchat-panel hotkey (`interfaces/webchat/.../state/hotkey.rs`) — CLI /
//! Telegram have no such key, so the hint must never reach them (R1/R4). The
//! model decides *whether* to surface it (only on unresolved problems) and
//! phrases it (R9); this layer only supplies the affordance.

use crate::thinker::interaction::InteractionParadigm;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct DoctorRepairHintLayer;

impl PromptLayer for DoctorRepairHintLayer {
    fn name(&self) -> &'static str {
        "doctor_repair_hint"
    }
    fn priority(&self) -> u32 {
        1120
    }
    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full | PromptMode::Compact)
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        // `f` is a webchat-panel hotkey only — gate on WebRich so CLI /
        // Telegram / background runs never see a "press f" instruction.
        let is_web_panel = input
            .context
            .is_some_and(|ctx| ctx.environment_contract.paradigm == InteractionParadigm::WebRich);
        if is_web_panel {
            output.push_str(
                "## Self-Repair (web panel)\n\
\n\
After running the `doctor` tool in read-only mode (`fix=false`) and finding \
unresolved problems, end your reply by reminding the user they can press the \
`f` key to start automatic repair. If no problems remain, do not mention `f`.\n",
            );
            output.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::context::{ContextAggregator, ResolvedContext};
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::LayerInput;
    use crate::thinker::security_context::SecurityContext;

    fn ctx_for(paradigm: InteractionParadigm) -> ResolvedContext {
        ContextAggregator::resolve(
            &InteractionManifest::new(paradigm),
            &SecurityContext::permissive(),
            &[],
        )
    }

    fn render(ctx: &ResolvedContext) -> String {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(ctx));
        let mut out = String::new();
        DoctorRepairHintLayer.inject(&mut out, &input);
        out
    }

    #[test]
    fn metadata() {
        let layer = DoctorRepairHintLayer;
        assert_eq!(layer.name(), "doctor_repair_hint");
        assert_eq!(layer.priority(), 1120);
        assert!(matches!(layer.stability(), LayerStability::Dynamic));
    }

    #[test]
    fn injects_for_web_panel() {
        let out = render(&ctx_for(InteractionParadigm::WebRich));
        assert!(out.contains("## Self-Repair"));
        assert!(out.contains("`f`"));
        assert!(out.contains("doctor"));
    }

    #[test]
    fn skips_non_web_paradigms() {
        for p in [
            InteractionParadigm::CLI,
            InteractionParadigm::Messaging,
            InteractionParadigm::Background,
            InteractionParadigm::Embedded,
        ] {
            assert!(
                render(&ctx_for(p)).is_empty(),
                "{p:?} must not see the press-f hint (R1/R4)"
            );
        }
    }

    #[test]
    fn skips_when_no_context() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        DoctorRepairHintLayer.inject(&mut out, &input);
        assert!(out.is_empty());
    }
}
```

- [ ] **Step 2: 注册到 `mod.rs`**

In `src/thinker/layers/mod.rs`，在其它 `mod` 声明附近加：

```rust
mod doctor_repair_hint;
```

并在 `pub use` 区加：

```rust
pub use doctor_repair_hint::DoctorRepairHintLayer;
```

- [ ] **Step 3: 注册到 `prompt_pipeline.rs`**

(a) 在 import 块（:7-16，`use crate::thinker::layers::{...}`）按字母序加 `DoctorRepairHintLayer,`。

(b) 在 priority 文档注释 `/// 1100  \`SpecialActionsLayer\``（:298）后加一行：

```rust
    /// 1120  `DoctorRepairHintLayer` (WebRich-only press-f hint)
```

(c) 在 `default_layers()` 的 `Box::new(SpecialActionsLayer),`（:350）后加：

```rust
            Box::new(DoctorRepairHintLayer),
```

> 注：`PromptPipeline::new()` 内部按 priority 排序（注册列表本身无序，`test_default_layers_sorted` 验证排序后非降序），插入位置不影响正确性，置于 SpecialActionsLayer 后仅为可读性。

- [ ] **Step 4: 更新计数测试**

In `src/thinker/prompt_pipeline.rs` 的 `test_default_layers_count`（`assert_eq!(pipeline.layer_count(), 42);`），改为：

```rust
        // → 43 (DoctorRepairHintLayer @1120 — WebRich-only "/doctor → press f"
        // hint, 2026-06-19). See `default_layers`.
        assert_eq!(pipeline.layer_count(), 43);
```

- [ ] **Step 5: 跑测试验证通过**

Run: `cargo test -p alephcore --lib doctor_repair_hint`
Expected: 4 个测试 PASS（`metadata` / `injects_for_web_panel` / `skips_non_web_paradigms` / `skips_when_no_context`）。

Run: `cargo test -p alephcore --lib prompt_pipeline::tests::test_default_layers`
Expected: `test_default_layers_count` 与 `test_default_layers_sorted` PASS。

- [ ] **Step 6: 提交**

```bash
git add src/thinker/layers/doctor_repair_hint.rs src/thinker/layers/mod.rs src/thinker/prompt_pipeline.rs
git commit -m "thinker: add WebRich-gated DoctorRepairHintLayer for /doctor → press-f"
```

---

## Task 2: `/doctor` slash 命令纯逻辑（aleph-panel / palette.rs）

**Files:**
- Modify: `interfaces/webchat/src/views/chat/composer/palette.rs`（在文件末尾 `#[cfg(test)] mod tests` 之前加常量与函数；测试加进现有 `mod tests`）
- Test: 同文件 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `CommandInfo`（同文件 :16，字段 `key/description/is_namespace/param_hint/children`）。
- Produces:
  - `pub(super) const DOCTOR_DETECT_PROMPT: &str`
  - `pub(super) fn expand_doctor_command(text: &str) -> Option<String>`
  - `pub(super) fn doctor_command_info() -> CommandInfo`

- [ ] **Step 1: 写失败测试**

In `interfaces/webchat/src/views/chat/composer/palette.rs` 的 `#[cfg(test)] mod tests` 内追加：

```rust
    #[test]
    fn expand_doctor_matches_only_bare_command() {
        assert!(expand_doctor_command("/doctor").is_some());
        assert!(expand_doctor_command("/Doctor").is_some()); // case-insensitive
        assert!(expand_doctor_command("  /doctor  ").is_some()); // trimmed
        assert_eq!(
            expand_doctor_command("/doctor").unwrap(),
            DOCTOR_DETECT_PROMPT
        );
    }

    #[test]
    fn expand_doctor_rejects_non_matches() {
        assert!(expand_doctor_command("/doctorx").is_none());
        assert!(expand_doctor_command("/doctor now").is_none()); // args not supported in v1
        assert!(expand_doctor_command("/doc").is_none());
        assert!(expand_doctor_command("hello").is_none());
        assert!(expand_doctor_command("").is_none());
    }

    #[test]
    fn doctor_command_info_is_a_leaf() {
        let info = doctor_command_info();
        assert_eq!(info.key, "doctor");
        assert!(!info.is_namespace);
        assert!(info.children.is_empty());
        assert!(!info.description.is_empty());
    }
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p aleph-panel --lib composer::palette::tests::expand_doctor`
Expected: FAIL，编译错误 `cannot find function expand_doctor_command` / `DOCTOR_DETECT_PROMPT`。

- [ ] **Step 3: 实现常量与函数**

In `interfaces/webchat/src/views/chat/composer/palette.rs`，在 `#[cfg(test)]` 之前加：

```rust
/// Seeded when the user runs `/doctor` — a read-only health check. Mirrors
/// the `f`-hotkey `DOCTOR_REPAIR_PROMPT` (in `composer/mod.rs`) but never
/// repairs. The model writes a natural-language report; `DoctorRepairHintLayer`
/// (WebRich-gated, alephcore) appends the "press f" reminder when problems
/// remain.
pub(super) const DOCTOR_DETECT_PROMPT: &str = "运行 doctor 工具只读诊断系统健康状况（fix=false，不要修复任何东西）。如实汇报发现的问题及其严重度。";

/// If `text` is exactly the bare `/doctor` command (trimmed, case-insensitive),
/// return the detection prompt to send through the normal LLM pipeline instead
/// of the literal slash command (which would hit the deterministic fast path).
/// Args are not supported in v1 — `/doctor <anything>` does not match.
pub(super) fn expand_doctor_command(text: &str) -> Option<String> {
    if text.trim().eq_ignore_ascii_case("/doctor") {
        Some(DOCTOR_DETECT_PROMPT.to_string())
    } else {
        None
    }
}

/// Static palette entry so `/doctor` is discoverable when the user types `/`,
/// even though it is intercepted client-side (not a Gateway tool-backed
/// command). Merged into the fetched catalogue by `composer::fetch_commands`.
pub(super) fn doctor_command_info() -> CommandInfo {
    CommandInfo {
        key: "doctor".to_string(),
        description: "只读检测系统健康，发现问题后可按 f 维修".to_string(),
        is_namespace: false,
        param_hint: None,
        children: Vec::new(),
    }
}
```

- [ ] **Step 4: 跑测试验证通过**

Run: `cargo test -p aleph-panel --lib composer::palette::tests`
Expected: 新增 3 个测试 + 原有 palette 测试全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add interfaces/webchat/src/views/chat/composer/palette.rs
git commit -m "panel: add /doctor detection prompt + expand/info helpers"
```

---

## Task 3: 把 `/doctor` 接进 composer（aleph-panel / mod.rs）

**Files:**
- Modify: `interfaces/webchat/src/views/chat/composer/mod.rs`（use :17-19、`send_message` :144、`enqueue_message` :227、`fetch_commands` :361）

**Interfaces:**
- Consumes: `expand_doctor_command`、`doctor_command_info`（Task 2，`palette` 模块）。
- Produces: 无导出（纯 UI 接线）。

- [ ] **Step 1: 扩展 palette use**

In `mod.rs` 的 `use palette::{ ... };`（:17-19），加入 `doctor_command_info, expand_doctor_command,`：

```rust
use palette::{
    build_palette_entries, doctor_command_info, expand_doctor_command, parse_command_info,
    CommandInfo, PaletteEntry, PaletteLabels,
};
```

- [ ] **Step 2: `send_message` 拦截 `/doctor`**

In `send_message`（:144），把：

```rust
        let text = input_text.get_untracked().trim().to_string();
```

改为（命中则展开成检测 prompt；气泡显示展开文本 = 决策 A 镜像 f 流）：

```rust
        let raw = input_text.get_untracked().trim().to_string();
        // `/doctor` → seed the read-only detection prompt and route it through
        // the normal LLM pipeline, mirroring the `f`-hotkey repair flow. Done
        // before send so the literal slash command never reaches the gateway
        // fast path (which would run the tool deterministically, no LLM).
        let text = expand_doctor_command(&raw).unwrap_or(raw);
```

- [ ] **Step 3: `enqueue_message` 同样拦截**

In `enqueue_message`（:227），把：

```rust
        let text = input_text.get_untracked().trim().to_string();
```

改为：

```rust
        let raw = input_text.get_untracked().trim().to_string();
        let text = expand_doctor_command(&raw).unwrap_or(raw);
```

- [ ] **Step 4: `fetch_commands` 追加静态 `/doctor` 条目**

In `fetch_commands`（:361），把：

```rust
                    all_commands.set(cmds.clone());
```

改为（去重：服务端若也暴露 doctor 则不重复）：

```rust
                    if !cmds.iter().any(|c| c.key == "doctor") {
                        cmds.push(doctor_command_info());
                    }
                    all_commands.set(cmds.clone());
```

- [ ] **Step 5: 编译验证（wasm 目标）**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown`
Expected: 编译通过，无 error / 无未使用导入警告。

- [ ] **Step 6: 提交**

```bash
git add interfaces/webchat/src/views/chat/composer/mod.rs
git commit -m "panel: wire /doctor command into composer send/enqueue/palette"
```

- [ ] **Step 7: 手动 E2E 验证（执行期，需重编 wasm + 刷新二进制）**

部署刷新链（见 `docs/reference/DESKTOP_SHELL.md`）：`just wasm` → 重编 `aleph-server` binary → 替换运行中 binary。然后在 webchat 面板：

1. 输入 `/` → 看到 `/doctor` 条目。验证可发现性。
2. 发送 `/doctor` → AI 跑 doctor 只读检测、出健康报告。
   - 若有问题：报告结尾提醒「输入 f 开始维修」。
   - 若无问题：报告健康，**不**提 f。
3. 按 `f`（焦点不在输入框）→ 触发现有修复流（未回归）。

---

## Self-Review

**Spec coverage（对照 spec 各节）：**

- §6.1 `DOCTOR_DETECT_PROMPT` → Task 2 Step 3 ✓
- §6.2 `expand_doctor_command` + send/enqueue 拦截 → Task 2（纯函数）+ Task 3 Step 2/3 ✓
- §6.3 调色板静态条目（决策 B）→ Task 2 `doctor_command_info` + Task 3 Step 4 ✓
- §6.4 `DoctorRepairHintLayer`（gate WebRich、priority 1120、注册三处、计数 42→43）→ Task 1 全部 ✓
- §7 决策 A（气泡显示展开 prompt）→ Task 3 Step 2 注释明确 ✓
- §8 边界（仅纯 `/doctor`、无问题不提 f、非 webchat 不泄漏）→ Task 2 测试 `expand_doctor_rejects_non_matches` + Task 1 测试 `skips_non_web_paradigms` + 层文案 "If no problems remain, do not mention f" ✓
- §9 测试三组 → Task 1 Step 1 + Task 2 Step 1 + Task 3 Step 7 ✓

**Placeholder scan:** 无 TBD/TODO；每个代码步骤含完整可粘贴代码。✓

**Type consistency:**
- `expand_doctor_command(&str) -> Option<String>`：Task 2 定义、Task 3 调用一致 ✓
- `doctor_command_info() -> CommandInfo`：Task 2 定义、Task 3 调用一致 ✓
- `DoctorRepairHintLayer`：Task 1 定义、prompt_pipeline 注册一致 ✓
- `CommandInfo` 字段（key/description/is_namespace/param_hint/children）与 palette.rs:16 一致 ✓
