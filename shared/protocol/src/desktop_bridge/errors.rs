//! JSON-RPC 2.0 error code constants + Aleph desktop-bridge server-defined codes.

pub const ERR_PARSE: i32 = -32700;
pub const ERR_INVALID_REQUEST: i32 = -32600;
pub const ERR_METHOD_NOT_FOUND: i32 = -32601;
pub const ERR_INVALID_ARGUMENT: i32 = -32602;
pub const ERR_INTERNAL: i32 = -32603;

pub const ERR_PERMISSION_DENIED: i32 = -32001;
pub const ERR_NOT_IMPLEMENTED: i32 = -32002;
pub const ERR_PLATFORM: i32 = -32003;
pub const ERR_TIMEOUT: i32 = -32004;
pub const ERR_HELPER_CRASHED: i32 = -32005;
pub const ERR_BRIDGE_DISABLED: i32 = -32006;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_match_spec() {
        assert_eq!(ERR_PARSE, -32700);
        assert_eq!(ERR_INVALID_REQUEST, -32600);
        assert_eq!(ERR_METHOD_NOT_FOUND, -32601);
        assert_eq!(ERR_INVALID_ARGUMENT, -32602);
        assert_eq!(ERR_INTERNAL, -32603);
        assert_eq!(ERR_PERMISSION_DENIED, -32001);
        assert_eq!(ERR_NOT_IMPLEMENTED, -32002);
        assert_eq!(ERR_PLATFORM, -32003);
        assert_eq!(ERR_TIMEOUT, -32004);
        assert_eq!(ERR_HELPER_CRASHED, -32005);
        assert_eq!(ERR_BRIDGE_DISABLED, -32006);
    }
}
