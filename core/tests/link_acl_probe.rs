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
    pub mod config_lifecycle;
    pub mod multi_agent_matrix;
    pub mod edge_cases;
}
