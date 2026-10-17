fn execute_observers(event: HookEvent, ctx: &Context) {}
fn execute_interceptors(event: HookEvent, ctx: &Context) {}
fn extension_startup() {
    execute_observers(HookEvent::SessionStart, &ctx);
}
