//! Per-origin tool invocation records — the `tool_usage.json` sidecar.
//!
//! Answers one question: **which installed MCP server / plugin is nobody
//! actually calling?** That is the evidence `doctor`, the `tool_usage` tool and
//! the Panel's extension pages need before anyone can responsibly uninstall
//! something.
//!
//! ## Why a new store rather than a existing one
//!
//! Three neighbours look like they could answer this and cannot:
//!
//! * [`crate::identity::ledger`] writes at the same chokepoint, but records
//!   **only mutating calls and refusals** by design — and MCP tools are
//!   overwhelmingly reads, i.e. exactly the calls it drops. Its `target` is
//!   also a bare tool name with no provenance. It is an accountability chain,
//!   not usage telemetry.
//! * `session_events` persists `ToolCallRequested { name }` and could be mined
//!   like Claude Code mines its transcripts. But the `name → server` mapping
//!   lives only in the live registry, so the moment a server is disconnected or
//!   uninstalled its history becomes unattributable — and those are precisely
//!   the rows a cleanup report is about. Events are also retired by compaction.
//! * [`crate::builtin_tools::command_ledger`] is per-session, in-memory and
//!   LRU-evicted; after a restart it reports "never happened" for work that did.
//!
//! ## Shape and scope
//!
//! Keyed by [`UsageOrigin::key`] — `mcp:<server_id>` / `plugin:<plugin_id>`.
//! **Builtins are deliberately not recorded**: nobody uninstalls `file_read`,
//! and 159 always-present rows would be pure noise (R10 — zero consumers). That
//! exclusion is expressed in the type, not in a filter: [`LoopTool::usage_origin`]
//! returns `None` for a builtin, so a builtin call pays one `Option::is_none`
//! and never touches the disk.
//!
//! `call_count` counts calls that **reached the tool**, success or failure.
//! Refusals (permission deny, hook block, approval denial) are not usage — the
//! agent never got to exercise the server — and they are already recorded, with
//! their reason, in the signed ledger.
//!
//! ## Absence of a row is the "never used" signal
//!
//! Nothing seeds a zero row at registration. "Installed but never called" is
//! recovered at report time by joining this map against the live list of
//! configured servers / installed plugins, which is where that list already
//! lives. Seeding would instead leave orphan rows behind every uninstall and
//! make `first_used_at` mean two different things.
//!
//! Best-effort throughout, mirroring [`crate::skill::usage`]: a corrupt or
//! unwritable sidecar degrades to a `warn` and never breaks the tool call.

pub mod report;
pub mod store;

pub use report::{build_report, ExtensionUsageReport, UsageEntry, UsageInventory};
pub use store::{record_call_detached, OriginUsage, ToolUsageStore, UsageOrigin};
