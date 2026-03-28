//! Team management tools.

mod create;
mod delegate;
mod disband;
pub mod inbox_read;
pub mod message_send;
mod status;
mod team_digest;
pub mod task_read_artifact;
pub mod task_submit;

pub use create::{
    CreateAgentSpec, EnrolledMember, MemberSpec, TeamCreateArgs, TeamCreateOutput, TeamCreateTool,
};
pub use delegate::{
    DelegateStatus, TeamDelegateArgs, TeamDelegateOutput, TeamDelegateTool,
};
pub use disband::{TeamDisbandArgs, TeamDisbandOutput, TeamDisbandTool};
pub use inbox_read::{InboxReadArgs, InboxReadOutput, InboxReadTool};
pub use message_send::{MessageSendArgs, MessageSendOutput, MessageSendTool};
pub use status::{
    MemberInfo, TaskInfo, TeamStatusArgs, TeamStatusOutput, TeamStatusTool,
};
pub use task_read_artifact::{TaskReadArtifactArgs, TaskReadArtifactOutput, TaskReadArtifactTool};
pub use task_submit::{TaskSubmitArgs, TaskSubmitOutput, TaskSubmitTool};
pub use team_digest::{TeamDigestArgs, TeamDigestOutput, TeamDigestTool};
