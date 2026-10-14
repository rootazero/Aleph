import re
p = 'src/browser/cdp_backend/recording.rs'
s = open(p, encoding='utf-8', newline='').read()
a = s.index("    fn shell_stub(")
b = s.index("    /// Poll `cond` until")
new = open('.analysis/browser-live-runtime/task49-helper.rs', encoding='utf-8', newline='').read()
s = s[:a] + new + s[b:]
s = s.replace('shell_stub(&dir, "wedged-encoder", "#!/bin/sh\\nexec sleep 3600\\n")', 'encoder_stub(&dir, "wedged")')


def rep(name, mode):
    global s
    pat = re.compile(r'shell_stub\(\s*&dir,\s*"%s",\s*"(?:[^"\\]|\\.)*",?\s*\)' % name)
    s, n = pat.subn('encoder_stub(&dir, "%s")' % mode, s)
    assert n == 1, (name, n)


rep("slow-encoder", "slow")
rep("failing-encoder", "failing")
rep("short-lived-encoder", "short-lived")
rep("no-touch-encoder", "no-touch")
assert 'shell_stub' not in s
open(p, 'w', encoding='utf-8', newline='').write(s)
