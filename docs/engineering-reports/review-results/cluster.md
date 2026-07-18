ISSUE|src/cluster/registry.rs:228|medium|production .expect() in resolve() can panic; return ResolveError instead|.expect("match_id returns an id that is present in nodes_by_id")
ISSUE|src/cluster/reverse_rpc.rs:153|low|production .expect() used for JSON serialization; should return a ReverseRpcError variant|serde_json::to_string(&req).expect("JsonRpcRequest serialization is infallible")
ISSUE|src/cluster/node_file_cmd.rs:80|high|base64 payload decoded into memory before MAX_FILE_BYTES size cap, allowing OOM|B64.decode(content_b64) runs before bytes.len() > MAX_FILE_BYTES check
ISSUE|src/cluster/node_file_cmd.rs:135|medium|file size metadata check is TOCTOU and does not bound std::fs::read allocation|size from metadata is checked but file may grow before read at line 143
ISSUE|src/cluster/registry.rs:309|low|malformed node command declarations are silently ignored|serde_json::from_value::<Vec<CommandDescriptor>>(v.clone()).ok() discards errors
ISSUE|src/cluster/registry.rs:145|low|environment list order is nondeterministic due to HashMap iteration|list_environments() collects from HashMap values without sorting
ISSUE|src/cluster/node_runtime.rs:40|low|CommandTable::register silently overwrites existing commands|self.commands.insert(name.into(), cmd) with no duplicate warning
