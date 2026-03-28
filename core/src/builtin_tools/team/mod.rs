//! Team management tools.

mod create;
mod delegate;
mod disband;
mod status;
pub mod task_read_artifact;
pub mod task_submit;

pub use create::{
    CreateAgentSpec, EnrolledMember, MemberSpec, TeamCreateArgs, TeamCreateOutput, TeamCreateTool,
};
pub use delegate::{
    DelegateStatus, TeamDelegateArgs, TeamDelegateOutput, TeamDelegateTool,
};
pub use disband::{TeamDisbandArgs, TeamDisbandOutput, TeamDisbandTool};
pub use status::{
    MemberInfo, TaskInfo, TeamStatusArgs, TeamStatusOutput, TeamStatusTool,
};
pub use task_read_artifact::{TaskReadArtifactArgs, TaskReadArtifactOutput, TaskReadArtifactTool};
pub use task_submit::{TaskSubmitArgs, TaskSubmitOutput, TaskSubmitTool};
