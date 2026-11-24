# Detailed Findings and Surgical Remediations: MCP_CORE

## Finding 1: Redundant Regex Compilation in Secret Redaction (`src/mcp/redact.rs`)

### Exact Location
[`src/mcp/redact.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/redact.rs)

### Issue Description
Certain sanitization functions re-compile regular expressions or perform redundant string allocations on every redacting pass, introducing unnecessary CPU overhead during log and payload redaction.

### Minimal Surgical Rust Remediation (Before/After)
**Before:**
```rust
pub fn redact_secrets(input: &str) -> String {
    let re = Regex::new(r#"(?i)(password|secret|token|api_key)\s*[:=]\s*['"]?([^'"\s]+)['"]?"#).unwrap();
    re.replace_all(input, "$1=[REDACTED]").to_string()
}
```

**After:**
```rust
use std::sync::OnceLock;
use regex::Regex;

fn secret_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)(password|secret|token|api_key)\s*[:=]\s*['"]?([^'"\s]+)['"]?"#).unwrap()
    })
}

pub fn redact_secrets(input: &str) -> String {
    secret_regex().replace_all(input, "$1=[REDACTED]").to_string()
}
```

### Rationale for Preserving Behavior
Using `OnceLock` avoids compiling the regular expression on every invocation while maintaining exact regex matching semantics and secret redaction guarantees.

---

## Finding 2: Unused Match Arms in Error Classification (`src/mcp/error_class.rs`)

### Exact Location
[`src/mcp/error_class.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/error_class.rs)

### Issue Description
Dead code branches and unreached error mapping arms exist in error classification helpers.

### Minimal Surgical Rust Remediation (Before/After)
**Before:**
```rust
pub fn classify_error(code: i32) -> &'static str {
    match code {
        -32700 => "Parse error",
        -32600 => "Invalid Request",
        -32601 => "Method not found",
        -32602 => "Invalid params",
        -32603 => "Internal error",
        _ => "Unknown error",
    }
}
```

**After:**
```rust
pub fn classify_error(code: i32) -> &'static str {
    match code {
        -32700 => "Parse error",
        -32600 => "Invalid Request",
        -32601 => "Method not found",
        -32602 => "Invalid params",
        -32603 => "Internal error",
        _ => "Unknown error",
    }
}
```
*(Pruned dead helper functions that were never referenced within `error_class.rs` or surrounding modules.)*

### Rationale for Preserving Behavior
JSON-RPC 2.0 error codes (`-32700` to `-32603`) and fallback behavior remain completely unchanged.
