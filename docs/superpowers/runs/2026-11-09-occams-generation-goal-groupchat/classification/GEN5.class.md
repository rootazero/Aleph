# Classification & Routing — GEN5 (Jev Tier)

| ID | Category | Route | fix_shape | risk | note |
|---|---|---|---|---|---|
| GEN5-01 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | 大 Base64 字符串上全量调用 `to_lowercase()` 导致数十兆堆内存分配 |
| GEN5-02 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | 标准 edit 与 Ark edit 之间 70+ 行重复的响应解析与输出构建逻辑 |
| GEN5-03 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | 请求序列化使用中间 `serde_json::Value` AST，产生二次遍历与大量节点分配 |
| GEN5-04 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | 轮询循环（最多 200 次）内高频重复分配 Auth Header、状态小写转换及 URL 拷贝 |
| GEN5-05 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | HTTP 响应体采用无限制的 `response.text().await`，缺失 `text_with_limit`  |
| GEN5-06 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | `build_video_prompt_text` 对每个参数执行临时 `format!()` 产生堆分配 |
| GEN5-07 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | `supported_types` 在前文已验证非空，后续 `.ok_or_else()` 分支为不可达死代码 |
| GEN5-08 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | `build_request_body` 中互斥字段无谓执行 `size.clone()` |
| GEN5-09 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | `OpenAiCompatProvider::new` 对已有默认值的 builder 重复分配并覆盖 `DEFAULT |
| GEN5-10 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | 已拥有所有权的 `request: GenerationRequest` 字段在函数内被全量 `.clone()` |
