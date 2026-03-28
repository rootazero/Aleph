//! Team management tools.

mod create;
mod delegate;
mod disband;
pub mod inbox_read;
pub mod message_send;
pub mod session_collaborate;
pub mod session_read;
pub mod session_turn;
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
pub use session_collaborate::{SessionCollaborateArgs, SessionCollaborateOutput, SessionCollaborateTool};
pub use session_read::{SessionReadArgs, SessionReadOutput, SessionReadTool};
pub use session_turn::{SessionTurnArgs, SessionTurnOutput, SessionTurnTool};
pub use status::{
    MemberInfo, TaskInfo, TeamStatusArgs, TeamStatusOutput, TeamStatusTool,
};
pub use task_read_artifact::{TaskReadArtifactArgs, TaskReadArtifactOutput, TaskReadArtifactTool};
pub use task_submit::{TaskSubmitArgs, TaskSubmitOutput, TaskSubmitTool};
pub use team_digest::{TeamDigestArgs, TeamDigestOutput, TeamDigestTool};
