//! Rhai Engine Configuration for Sandboxed Script Execution

use rhai::Engine;

/// Create a sandboxed Rhai engine with strict limits
pub fn create_sandboxed_engine() -> Engine {
    let mut engine = Engine::new();

    // Limit operations (prevent infinite loops)
    engine.set_max_operations(1000);

    // Limit expression depth (prevent stack overflow)
    engine.set_max_expr_depths(10, 5);

    // Limit function call levels
    engine.set_max_call_levels(5);

    // Disable dangerous features
    engine.disable_symbol("eval");
    engine.on_print(|_| {}); // Disable print

    // Disable loops (only allow iterators)
    engine.disable_symbol("while");
    engine.disable_symbol("loop");
    engine.disable_symbol("for");

    // No module loading
    #[cfg(not(feature = "no_module"))]
    {
        use rhai::module_resolvers::DummyModuleResolver;
        engine.set_module_resolver(DummyModuleResolver::new());
    }

    engine
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
