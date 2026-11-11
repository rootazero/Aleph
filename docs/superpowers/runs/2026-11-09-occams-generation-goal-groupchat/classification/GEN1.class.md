# Classification & Routing — GEN1 (Jev Tier)

| ID | Category | Route | fix_shape | risk | note |
|---|---|---|---|---|---|
| GEN1-01 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | Parity Violation in `GenerationProviderRegistry::remove` for |
| GEN1-02 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | Auth Header Mismatch and False Positives in `probe_generatio |
| GEN1-03 | PANIC-RISK | **FIX-NOW** | local-edit | low | `NaN` Propagation and JSON Serialization Failure in `Generat |
| GEN1-04 | REDUNDANT-ALLOCATION | **FIX-CAREFULLY** | local-edit | none | Redundant Heap Allocations in `probe_url` |
| GEN1-05 | INVARIANT-TIGHTENING | **FIX-CAREFULLY** | local-edit | low | Redundant Vector Allocations and Dynamic Dispatch Smells in  |
| GEN1-06 | ERROR-SEMANTICS | **FIX-NOW** | local-edit | low | Loss of Diagnostic Context in `From<GenerationError> for Ale |
| GEN1-07 | REDUNDANT-ALLOCATION | **FIX-CAREFULLY** | local-edit | none | Redundant `.unwrap_or(401)` and Unnecessary String Allocatio |
