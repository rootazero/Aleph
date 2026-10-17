fn relay(event: HookEvent) {
    executor.execute_observers(event, &ctx);
}
fn fire_startup(event: HookEvent) {
    relay(event);
}
fn startup() {
    fire_startup(HookEvent::SessionStart);
}
fn first_tool() {
    executor.execute_observers(HookEvent::BeforeToolCall, &ctx);
}
fn second_tool() {
    executor.execute_interceptors(HookEvent::BeforeToolCall, &ctx);
}
fn compact() {
    executor.execute_interceptors(HookEvent::AfterCompaction, &ctx);
}
// executor.execute_observers(HookEvent::SessionStart, &ctx);
const DECOY: &str = "executor.execute_observers(HookEvent::SessionStart, &ctx)";
#[cfg(test)]
mod tests {
    fn decoy() {
        executor.execute_observers(HookEvent::SessionStart, &ctx);
    }
}
