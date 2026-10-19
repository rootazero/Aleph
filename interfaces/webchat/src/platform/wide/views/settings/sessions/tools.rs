//! Tool Outputs Tab
//!
//! Displays tool execution results from tool_outputs.md in a card layout.
//! Shows tool name, arguments, stdout, stderr, exit code, and duration.

use crate::api::session_management::SessionsApi;
use crate::context::DashboardState;
use crate::i18n::use_i18n;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Parse a single tool execution from markdown format
#[derive(Debug, Clone)]
struct ToolExecution {
    timestamp: String,
    tool_name: String,
    args: String,
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    duration_ms: u64,
}

/// Parse tool_outputs.md content into structured executions
fn parse_tool_outputs(content: &str) -> Vec<ToolExecution> {
    let mut executions = Vec::new();
    let mut current: Option<ToolExecution> = None;
    let mut in_stdout = false;
    let mut in_stderr = false;
    let mut in_args = false;

    for line in content.lines() {
        // New tool execution starts with "## [timestamp] tool_name"
        if let Some(header) = line.strip_prefix("## [") {
            // Save previous execution
            if let Some(exec) = current.take() {
                executions.push(exec);
            }

            // Parse header: "## [2024-10-18T10:30:45Z] bash"
            if let Some((timestamp, tool_name)) = header.split_once("] ") {
                current = Some(ToolExecution {
                    timestamp: timestamp.to_string(),
                    tool_name: tool_name.to_string(),
                    args: String::new(),
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    duration_ms: 0,
                });
                in_stdout = false;
                in_stderr = false;
                in_args = false;
            }
        } else if let Some(exec) = current.as_mut() {
            if line.starts_with("**Arguments:**") {
                in_args = true;
                in_stdout = false;
                in_stderr = false;
            } else if line.starts_with("**stdout:**") {
                in_stdout = true;
                in_stderr = false;
                in_args = false;
            } else if line.starts_with("**stderr:**") {
                in_stderr = true;
                in_stdout = false;
                in_args = false;
            } else if line.starts_with("**Exit code:**") {
                if let Some(code_str) = line.strip_prefix("**Exit code:** ") {
                    exec.exit_code = code_str.parse().ok();
                }
                in_stdout = false;
                in_stderr = false;
                in_args = false;
            } else if line.starts_with("**Duration:**") {
                if let Some(duration_str) = line.strip_prefix("**Duration:** ") {
                    if let Some(ms_str) = duration_str.strip_suffix("ms") {
                        exec.duration_ms = ms_str.trim().parse().unwrap_or(0);
                    }
                }
            } else if line.starts_with("```") {
                // Toggle code block (don't add the fence itself)
                continue;
            } else if in_args {
                if !exec.args.is_empty() {
                    exec.args.push('\n');
                }
                exec.args.push_str(line);
            } else if in_stdout {
                if !exec.stdout.is_empty() {
                    exec.stdout.push('\n');
                }
                exec.stdout.push_str(line);
            } else if in_stderr {
                if !exec.stderr.is_empty() {
                    exec.stderr.push('\n');
                }
                exec.stderr.push_str(line);
            }
        }
    }

    // Save last execution
    if let Some(exec) = current {
        executions.push(exec);
    }

    executions
}

#[component]
#[must_use]
pub fn ToolOutputsTab(session_id: String) -> impl IntoView {
    let _i18n = use_i18n();
    let state = expect_context::<DashboardState>();

    let executions = RwSignal::new(Vec::<ToolExecution>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);

    let session_id_clone = session_id.clone();
    Effect::new(move || {
        if !state.is_connected.get() {
            return;
        }

        let session_id = session_id_clone.clone();
        spawn_local(async move {
            loading.set(true);
            match SessionsApi::get(&state, &session_id).await {
                Ok(data) => {
                    let parsed = parse_tool_outputs(&data.tool_outputs);
                    executions.set(parsed);
                    error.set(None);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to load tool outputs: {}", e)));
                }
            }
            loading.set(false);
        });
    });

    view! {
        <div class="space-y-4">
            {move || error.get().map(|e| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                    {e}
                </div>
            })}

            {move || {
                if loading.get() {
                    return view! {
                        <div class="text-text-secondary py-8 text-center text-sm">
                            "Loading tool outputs..."
                        </div>
                    }.into_any();
                }

                let execs = executions.get();
                if execs.is_empty() {
                    return view! {
                        <div class="text-text-tertiary py-8 text-center text-sm">
                            "No tool executions recorded"
                        </div>
                    }.into_any();
                }

                view! {
                    <div class="space-y-3">
                        {execs.into_iter().map(|exec| {
                            let has_stderr = !exec.stderr.is_empty();
                            let exit_failed = exec.exit_code.is_some_and(|c| c != 0);

                            view! {
                                <div class="bg-surface-raised border border-border rounded-xl p-4">
                                    // Header: tool name and timestamp
                                    <div class="flex items-start justify-between gap-3 mb-3">
                                        <div class="flex items-center gap-2">
                                            <span class="font-mono text-sm font-medium text-text-primary">
                                                {exec.tool_name.clone()}
                                            </span>
                                            {exit_failed.then(|| view! {
                                                <span class="px-1.5 py-0.5 rounded text-[10px] font-semibold bg-danger-subtle text-danger">
                                                    "FAILED"
                                                </span>
                                            })}
                                        </div>
                                        <div class="flex items-center gap-3 text-xs text-text-tertiary">
                                            <span>{exec.timestamp.clone()}</span>
                                            <span>{exec.duration_ms}"ms"</span>
                                        </div>
                                    </div>

                                    // Arguments
                                    {(!exec.args.is_empty()).then(|| view! {
                                        <div class="mb-3">
                                            <div class="text-xs font-medium text-text-secondary mb-1">
                                                "Arguments"
                                            </div>
                                            <pre class="bg-surface-sunken rounded p-2 text-xs font-mono text-text-primary overflow-x-auto">
                                                {exec.args.clone()}
                                            </pre>
                                        </div>
                                    })}

                                    // Stdout
                                    {(!exec.stdout.is_empty()).then(|| view! {
                                        <div class="mb-3">
                                            <div class="text-xs font-medium text-text-secondary mb-1">
                                                "stdout"
                                            </div>
                                            <pre class="bg-surface-sunken rounded p-2 text-xs font-mono text-text-primary overflow-x-auto max-h-48 overflow-y-auto">
                                                {exec.stdout.clone()}
                                            </pre>
                                        </div>
                                    })}

                                    // Stderr (red if present)
                                    {has_stderr.then(|| view! {
                                        <div class="mb-3">
                                            <div class="text-xs font-medium text-danger mb-1">
                                                "stderr"
                                            </div>
                                            <pre class="bg-danger-subtle border border-danger/20 rounded p-2 text-xs font-mono text-danger overflow-x-auto max-h-48 overflow-y-auto">
                                                {exec.stderr.clone()}
                                            </pre>
                                        </div>
                                    })}

                                    // Exit code
                                    <div class="flex items-center gap-4 text-xs">
                                        {exec.exit_code.map(|code| view! {
                                            <div class="flex items-center gap-1">
                                                <span class="text-text-tertiary">"Exit code:"</span>
                                                <span
                                                    class="font-mono font-medium"
                                                    class=("text-danger", move || code != 0)
                                                    class=("text-success", move || code == 0)
                                                >
                                                    {code}
                                                </span>
                                            </div>
                                        })}
                                    </div>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }}
        </div>
    }
}
