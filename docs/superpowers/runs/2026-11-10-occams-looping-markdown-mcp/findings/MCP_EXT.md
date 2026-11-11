# Detailed Findings and Surgical Remediations: MCP_EXT

## Finding 1: Redundant String Allocation in `strip_server_prefix`

### Exact Location
[`src/mcp/external/connection.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/external/connection.rs#L30-L32)

### Issue Description
The helper function `strip_server_prefix` constructs a temporary `String` via `format!("{server_name}:")` on every invocation just to call `strip_prefix`. Since `str::strip_prefix` accepts any pattern implementing `Pattern` (such as `&str`), we can pass the prefix string directly or construct a borrowed prefix if needed, or simply pass `&format!(...)` directly into `strip_prefix` without allocating an owned `String` if not required, or even better, construct the prefix key cleanly. Wait, `strip_prefix` takes a `&str`, so `&format!(...)` creates a temporary `String` which is then borrowed as `&str`, which still allocates. However, we can check if `s` starts with `server_name` followed by `:` without allocating a `String` at all by doing:
```rust
fn strip_server_prefix<'a>(s: &'a str, server_name: &str) -> &'a str {
    if let Some(rest) = s.strip_prefix(server_name) {
        if let Some(inner) = rest.strip_prefix(':') {
            return inner;
        }
    }
    s
}
```
Or even simpler:
```rust
fn strip_server_prefix<'a>(s: &'a str, server_name: &str) -> &'a str {
    if s.starts_with(server_name) && s.as_bytes().get(server_name.len()) == Some(&b':') {
        &s[server_name.len() + 1..]
    } else {
        s
    }
}
```

### Minimal Surgical Rust Remediation

#### Before
```rust
fn strip_server_prefix<'a>(s: &'a str, server_name: &str) -> &'a str {
    s.strip_prefix(&format!("{server_name}:")).unwrap_or(s)
}
```

#### After
```rust
fn strip_server_prefix<'a>(s: &'a str, server_name: &str) -> &'a str {
    if let Some(rest) = s.strip_prefix(server_name) {
        if let Some(inner) = rest.strip_prefix(':') {
            return inner;
        }
    }
    s
}
```

### Rationale for Preserving Behavior
The new implementation avoids heap allocation (`format!`) entirely while maintaining exact semantic equivalence: if `s` starts with `server_name` followed by `:`, it strips both; otherwise it returns `s` unmodified.
