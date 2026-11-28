//! JSON-RPC 2.0 error code constants + Aleph desktop-bridge server-defined codes.

pub const ERR_PERMISSION_DENIED: i32 = -32001;
pub const ERR_NOT_IMPLEMENTED: i32 = -32002;
pub const ERR_PLATFORM: i32 = -32003;
pub const ERR_TIMEOUT: i32 = -32004;
pub const ERR_HELPER_CRASHED: i32 = -32005;
pub const ERR_BRIDGE_DISABLED: i32 = -32006;
/// The recognizer has no model for the requested (or resolved) language.
pub const ERR_UNSUPPORTED_LANGUAGE: i32 = -32007;
/// A native AX reference refusal with structured `AxReferenceErrorData`.
pub const ERR_AX_REFERENCE: i32 = -32008;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_match_spec() {
        assert_eq!(ERR_PERMISSION_DENIED, -32001);
        assert_eq!(ERR_NOT_IMPLEMENTED, -32002);
        assert_eq!(ERR_PLATFORM, -32003);
        assert_eq!(ERR_TIMEOUT, -32004);
        assert_eq!(ERR_HELPER_CRASHED, -32005);
        assert_eq!(ERR_BRIDGE_DISABLED, -32006);
        assert_eq!(ERR_UNSUPPORTED_LANGUAGE, -32007);
        assert_eq!(ERR_AX_REFERENCE, -32008);
    }
}
