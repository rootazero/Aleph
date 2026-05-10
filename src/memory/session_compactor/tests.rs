use super::*;

#[test]
fn extract_depth_d0() {
    assert_eq!(extract_depth("aleph://session/abc/d0/3"), 0);
}

#[test]
fn extract_depth_d1() {
    assert_eq!(extract_depth("aleph://session/abc/d1/0"), 1);
}

#[test]
fn extract_depth_d2() {
    assert_eq!(extract_depth("aleph://session/abc/d2/1"), 2);
}

#[test]
fn extract_depth_missing_returns_zero() {
    assert_eq!(extract_depth("aleph://user/preferences/"), 0);
}

#[test]
fn extract_depth_complex_session_id() {
    assert_eq!(extract_depth("aleph://session/agent:main:main/d1/5"), 1);
}

#[test]
fn compress_result_default_is_zero() {
    let r = CompressResult::default();
    assert_eq!(r.d0_created, 0);
    assert_eq!(r.d1_created, 0);
    assert_eq!(r.d2_created, 0);
}
