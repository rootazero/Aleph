# Classification & Routing — GEN2 (Jev Tier)

| ID | Category | Route | fix_shape | risk | note |
|---|---|---|---|---|---|
| GEN2-01 | ERROR-SEMANTICS | **FIX-NOW** | local-edit | low | 404 Model Not Found Error Reporting Hardcodes `DEFAULT_MODEL |
| GEN2-02 | REDUNDANT-ALLOCATION | **FIX-CAREFULLY** | local-edit | none | `GoogleImagenProvider::determine_aspect_ratio` Ignores `requ |
| GEN2-03 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | Missing Trailing Slash Trim on `base_url` Produces Double-Sl |
| GEN2-04 | DEAD-CODE | **FIX-CAREFULLY** | local-edit | none | Hardcoded `extras: None` in `bfl/mod.rs` Drops Model Knobs a |
| GEN2-05 | ERROR-SEMANTICS | **FIX-NOW** | local-edit | low | Missing `Retry-After` Header Extraction on HTTP 429 Breaks ` |
| GEN2-06 | REDUNDANT-ALLOCATION | **FIX-CAREFULLY** | local-edit | none | Redundant String Allocations in High-Frequency Fal Polling L |
| GEN2-07 | REDUNDANT-ALLOCATION | **FIX-CAREFULLY** | local-edit | none | Redundant `.clone()` of URLs, Base64 Strings, and Endpoint i |
| GEN2-08 | DEAD-CODE | **FIX-CAREFULLY** | local-edit | none | Dead `Serialize` Derive on Internal Struct `FalStatusRespons |
| GEN2-09 | ERROR-SEMANTICS | **FIX-NOW** | local-edit | low | Duplicated `reqwest::Client` Inline Construction Bypassing ` |
| GEN2-10 | REDUNDANT-ALLOCATION | **FIX-CAREFULLY** | local-edit | none | Inefficient Text Buffering in BFL Poll Loop and Redundant Sa |
