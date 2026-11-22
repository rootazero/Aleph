# Classification & Routing — GC1 (Jev Tier)

| ID | Category | Route | fix_shape | risk | note |
|---|---|---|---|---|---|
| GC1-01 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | Command Parser Whitespace Sensitivity and Repetitive Prefix  |
| GC1-02 | ERROR-SEMANTICS | **FIX-NOW** | local-edit | low | Missing Adjacent Flag Check on `--preset` Parameter |
| GC1-03 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | `strip_markdown_fences` Case-Sensitive `"json"` Tag Breakage |
| GC1-04 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | Double-Pass Allocation in `json_safe_field` |
| GC1-05 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | Redundant Intermediate `format!` Allocations in Prompt Build |
| GC1-06 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | Unnecessary `BTreeSet` & `Vec` in `build_history_text_window |
| GC1-07 | REDUNDANT-ALLOCATION | **FIX-CAREFULLY** | local-edit | none | Redundant Arc Clones and String Allocations in `persist_turn |
| GC1-08 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | Inverted `truncate` / `sort_by_key` Discarding Priority Resp |
| GC1-09 | REDUNDANT-ALLOCATION | **FIX-CAREFULLY** | local-edit | none | Case-Sensitive Deduplication vs Case-Insensitive Participant |
| GC1-10 | REDUNDANT-ALLOCATION | **FIX-CAREFULLY** | local-edit | none | Redundant `CancellationToken::clone()` in Select Loops |
| GC1-11 | REDUNDANT-ALLOCATION | **FIX-CAREFULLY** | local-edit | none | Triple Cloning of LLM Responses Across Staging and Emission |
| GC1-12 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | Case-Sensitive Duplicate Check in `create_session` Permittin |
| GC1-13 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | Missing Whitespace-Only Validation in `Persona::validate` |
| GC1-14 | REDUNDANT-ALLOCATION | **FIX-CAREFULLY** | local-edit | none | Double Clone and Unbounded Map Growth in `PersonaRegistry::f |
