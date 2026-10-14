import sys

from ed import apply

base = 'D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256/'
new = open(base + '.analysis/browser-live-runtime/newtests.rs.txt', encoding='utf8').read().replace('\r\n', '\n')

# fix tests text
new = new.replace('''        let f = fixture(None, false, 22);
        let entered = Arc::new(tokio::sync::Notify::new());
        let manager = CompanionManager::with_steps(
            f.manager.producer.clone(),
            Some(f.launcher.clone()),
            Arc::new(HangingProver(Arc::clone(&entered))),
            Arc::clone(&f.steps),
            Duration::from_millis(200),
        );
''', '''        let entered = Arc::new(tokio::sync::Notify::new());
        let f = fixture_with(Arc::new(HangingProver(Arc::clone(&entered))), false, 22);
        let manager = Arc::clone(&f.manager);
''')
new = new.replace('''        held.pop();
        assert!(f.manager.reserve().is_ok(), "a dropped slot is reusable");
        // A start with no free slot launches nothing.
        let full: Vec<_> = (0..MAX_LIVE_COMPANIONS - 1)
            .map(|_| f.manager.reserve().expect("slot"))
            .collect();
        assert!(!full.is_empty());
''', '''        held.pop();
        assert!(f.manager.reserve().is_ok(), "a dropped slot is reusable");
        drop(held);
        assert_eq!(f.manager.live_count(), 0, "every slot was released");
        // A start with no free slot launches nothing.
        let full: Vec<_> = (0..MAX_LIVE_COMPANIONS)
            .map(|_| f.manager.reserve().expect("slot"))
            .collect();
        assert_eq!(full.len(), MAX_LIVE_COMPANIONS);
''')

E = []
E.append(('''    fn fixture(prover_fail: Option<ProtocolError>, unsupported: bool, pid: u32) -> Fixture {
        let bus''', '''    fn fixture(prover_fail: Option<ProtocolError>, unsupported: bool, pid: u32) -> Fixture {
        fixture_with(Arc::new(FakeProver { fail: prover_fail }), unsupported, pid)
    }

    fn fixture_with(prover: Arc<dyn EndpointProver>, unsupported: bool, pid: u32) -> Fixture {
        let bus'''))
E.append(('''            Arc::new(FakeProver { fail: prover_fail }),
            Arc::clone(&steps),
            Duration::from_millis(200),''', '''            prover,
            Arc::clone(&steps),
            Duration::from_millis(200),'''))
# append new tests before the final closing brace of mod tests
path = base + 'src/builtin_tools/browser_tools/companion/manager.rs'
raw = open(path, 'rb').read().decode('utf8').replace('\r\n', '\n')
assert raw.rstrip().endswith('}')
idx = raw.rstrip().rfind('}')
raw = raw[:idx] + new.lstrip('\n').join(['\n', '']) + '}\n'
open(path, 'wb').write(raw.replace('\n', '\r\n').encode('utf8'))
apply(path, E)
