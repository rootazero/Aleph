# Batch GEN5 (`src/generation/providers/openai_compat`) 静态 Occam's Razor 审查报告

对 `src/generation/providers/openai_compat` 全部 7 个文件进行了完整的静态审查，重点针对死代码/无用辅助、跨 edit/generate 重复转换、无谓内存与 DOM 分配、控制流扁平化、并发与流式响应、以及协议与错误契约不变量。

---

## 审查发现汇总表

| Finding ID | 类别 | 文件:行号 | 摘要 | 优先级 |
|---|---|---|---|---|
| **GEN5-01** | Redundant Allocation | `helpers.rs:198-205`<br>`edit.rs:77-78, 109-111` | 大 Base64 字符串上全量调用 `to_lowercase()` 导致数十兆瞬时内存分配 | **FIX-NOW** |
| **GEN5-02** | Dead Code / Duplicate Logic | `edit.rs:233-319`<br>`edit.rs:413-485` | 标准 edit 与 Ark edit 之间 70+ 行完全重复的响应提取与输出组装逻辑 | **FIX-NOW** |
| **GEN5-03** | Redundant Allocation / Complexity | `generate.rs:55-71, 82` | 请求序列化使用中间 `serde_json::Value` AST，产生二次遍历与大量节点分配 | **FIX-NOW** |
| **GEN5-04** | Redundant Allocation | `generate.rs:259, 289, 298-310` | 轮询循环（最多 200 次）内高频重复分配 Auth Header、状态小写转换及 URL 拷贝 | **FIX-NOW** |
| **GEN5-05** | Security / Resource Exhaustion | `generate.rs:96`<br>`edit.rs:217-219, 399-401` | HTTP 响应体采用无限制的 `response.text().await`，缺失 `text_with_limit` 保护 | **FIX-CAREFULLY** |
| **GEN5-06** | Redundant Allocation | `helpers.rs:227-244` | `build_video_prompt_text` 对每个参数执行临时 `format!()` 产生堆分配 | **FIX-NOW** |
| **GEN5-07** | Dead Code / Redundant Check | `builder.rs:183-188` | `supported_types` 在前文已验证非空，后续 `.ok_or_else()` 分支为不可达死代码 | **FIX-NOW** |
| **GEN5-08** | Redundant Allocation | `helpers.rs:89-90` | `build_request_body` 中互斥字段无谓执行 `size.clone()` | **FIX-NOW** |
| **GEN5-09** | Redundant Allocation | `provider.rs:112` | `OpenAiCompatProvider::new` 对已有默认值的 builder 重复分配并覆盖 `DEFAULT_MODEL` | **FIX-NOW** |
| **GEN5-10** | Redundant Allocation | `edit.rs:53, 69, 74, 191, 288` | 已拥有所有权的 `request: GenerationRequest` 字段在函数内被全量 `.clone()` | **FIX-NOW** |

---

## 详细审查项

### GEN5-01: 大 Base64 字符串上全量调用 `to_lowercase()` 导致数十兆堆内存分配
- **文件与行号**：`helpers.rs:198-205`, `edit.rs:77-78, 109-111`
- **修复方式**：改用无分配的 ASCII 前缀切片与 `eq_ignore_ascii_case` 检查。

### GEN5-02: 标准 edit 与 Ark edit 之间 70+ 行重复的响应解析与输出构建逻辑
- **文件与行号**：`edit.rs:233-319`, `edit.rs:413-485`
- **修复方式**：提取通用的响应装配函数 `build_edit_output`，直接将 `ImageData` 转移所有权。

### GEN5-03: 请求序列化使用中间 `serde_json::Value` AST，产生二次遍历与大量节点分配
- **文件与行号**：`generate.rs:55-71, 82`
- **修复方式**：直接序列化为字节向量 `serde_json::to_vec(&body)` 传入请求体。

### GEN5-04: 轮询循环（最多 200 次）内高频重复分配 Auth Header、状态小写转换及 URL 拷贝
- **文件与行号**：`generate.rs:259, 289, 298-310`
- **修复方式**：外提 auth header 格式化，使用 `eq_ignore_ascii_case`，为 `AsyncTaskData` 提供 `into_output_url`。

### GEN5-05: HTTP 响应体采用无限制的 `response.text().await`，缺失 `text_with_limit` 保护
- **文件与行号**：`generate.rs:96`, `edit.rs:217-219, 399-401`
- **修复方式**：使用 `text_with_limit(response, 32 * 1024 * 1024)`。

### GEN5-06: `build_video_prompt_text` 对每个参数执行临时 `format!()` 产生堆分配
- **文件与行号**：`helpers.rs:227-244`
- **修复方式**：使用 `std::fmt::Write; write!(text, ...)`。

### GEN5-07: `supported_types` 在前文已验证非空，后续 `.ok_or_else()` 分支为不可达死代码
- **文件与行号**：`builder.rs:183-188`
- **修复方式**：直接使用索引 `self.supported_types[0]`。

### GEN5-08: `build_request_body` 中互斥字段无谓执行 `size.clone()`
- **文件与行号**：`helpers.rs:89-90`
- **修复方式**：通过元组模式匹配转移所有权：`let (size, image_size) = if is_siliconflow { (None, size) } else { (size, None) };`。

### GEN5-09: `OpenAiCompatProvider::new` 对已有默认值的 builder 重复分配并覆盖 `DEFAULT_MODEL`
- **文件与行号**：`provider.rs:112`
- **修复方式**：仅当 `model.is_some()` 时调用 `.model()`。

### GEN5-10: 已拥有所有权的 `request: GenerationRequest` 字段在函数内被全量 `.clone()`
- **文件与行号**：`edit.rs:53, 69, 74, 191, 288`
- **修复方式**：解构或 move `request` 字段。
