//! Sub-agent management tools — spawn, steer, kill sub-agents at runtime.

pub mod kill;
pub mod spawn;
pub mod steer;

pub use kill::{SubagentKillArgs, SubagentKillOutput, SubagentKillTool};
pub use spawn::{SubagentSpawnArgs, SubagentSpawnOutput, SubagentSpawnTool};
pub use steer::{SubagentSteerArgs, SubagentSteerOutput, SubagentSteerTool};
