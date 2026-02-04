//! Integration test for Rhai sandboxed engine

use aethecore::daemon::dispatcher::scripting::create_sandboxed_engine;

#[test]
fn test_sandboxed_engine_rejects_dangerous_operations() {
    let engine = create_sandboxed_engine();

    // Should reject eval
    let result = engine.compile("eval(\"malicious\")");
    assert!(result.is_err());

    // Should reject while loops
    let result = engine.compile("while true { }");
    assert!(result.is_err());
}

#[test]
fn test_sandboxed_engine_accepts_safe_expressions() {
    let engine = create_sandboxed_engine();

    // Should accept simple expressions
    let result = engine.compile("1 + 1");
    assert!(result.is_ok());

    // Should accept filter/map chains
    let result = engine.compile("[1, 2, 3].filter(|x| x > 1)");
    assert!(result.is_ok());
}

#[test]
fn test_sandboxed_engine_enforces_operation_limit() {
    let engine = create_sandboxed_engine();

    // Should timeout on excessive operations
    let script = "(1..10000).map(|x| x * x).sum()";
    let result: Result<i64, _> = engine.eval(script);
    assert!(result.is_err());
}
