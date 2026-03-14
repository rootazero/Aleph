//! Link ACL probe integration tests.
//!
//! Tests the agent link access control system end-to-end.

mod link_acl_probe {
    pub mod mock_channel;
    pub mod harness;
    pub mod access_control;
    pub mod message_routing;
    pub mod switch_command;
    pub mod intent_switch;
}
