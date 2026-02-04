//! Step definitions for scripting engine features

use cucumber::{given, when, then};
use crate::world::{AlephWorld, ScriptingContext};
use alephcore::daemon::dispatcher::scripting::create_sandboxed_engine;

#[given("a sandboxed scripting engine")]
async fn given_sandboxed_engine(w: &mut AlephWorld) {
    let engine = create_sandboxed_engine();
    w.scripting = Some(ScriptingContext {
        engine: Some(engine),
        compile_result: None,
        eval_result: None,
    });
}

#[when(expr = "I try to compile a script containing {string}")]
async fn when_compile_containing(w: &mut AlephWorld, content: String) {
    let ctx = w.scripting.as_mut().expect("Scripting context not initialized");
    let engine = ctx.engine.as_ref().expect("Engine not initialized");

    let script = if content == "eval" {
        "eval(\"malicious\")"
    } else {
        &content
    };

    ctx.compile_result = Some(
        engine
            .compile(script)
            .map_err(|e| e.to_string())
    );
}

#[when(expr = "I compile the script {string}")]
async fn when_compile_script(w: &mut AlephWorld, script: String) {
    let ctx = w.scripting.as_mut().expect("Scripting context not initialized");
    let engine = ctx.engine.as_ref().expect("Engine not initialized");

    ctx.compile_result = Some(
        engine
            .compile(&script)
            .map_err(|e| e.to_string())
    );
}

#[when(expr = "I evaluate the script {string}")]
async fn when_eval_script(w: &mut AlephWorld, script: String) {
    let ctx = w.scripting.as_mut().expect("Scripting context not initialized");
    let engine = ctx.engine.as_ref().expect("Engine not initialized");

    let result: Result<i64, _> = engine.eval(&script);
    ctx.eval_result = Some(result.map_err(|e| e.to_string()));
}

#[then("the compilation should fail")]
async fn then_compile_fails(w: &mut AlephWorld) {
    let ctx = w.scripting.as_ref().expect("Scripting context not initialized");
    let result = ctx.compile_result.as_ref().expect("No compilation attempted");
    assert!(result.is_err(), "Expected compilation to fail, but it succeeded");
}

#[then("the compilation should succeed")]
async fn then_compile_succeeds(w: &mut AlephWorld) {
    let ctx = w.scripting.as_ref().expect("Scripting context not initialized");
    let result = ctx.compile_result.as_ref().expect("No compilation attempted");
    assert!(result.is_ok(), "Expected compilation to succeed, got: {:?}", result);
}

#[then("the evaluation should fail")]
async fn then_eval_fails(w: &mut AlephWorld) {
    let ctx = w.scripting.as_ref().expect("Scripting context not initialized");
    let result = ctx.eval_result.as_ref().expect("No evaluation attempted");
    assert!(result.is_err(), "Expected evaluation to fail due to limits");
}
