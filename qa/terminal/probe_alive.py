#!/usr/bin/env python3
"""Is this program still running N seconds after being started in a PTY?

`command -v` answers "installed", which is not the question. This machine has
a `codex` on PATH whose vendored native binary is missing: it prints an ENOENT
traceback and exits inside a second. Handing that to the `real` stage would
fail the stage for a reason that is not the product's.

A PTY is the point — an interactive agent given a pipe often exits at once,
and the fixture is going to run it under one anyway.

Exit 0 = alive after the wait. Anything else = do not use this candidate.

Usage:  probe_alive.py /path/to/agent [seconds]
"""
import fcntl
import os
import pty
import signal
import sys
import time


def main() -> int:
    program = sys.argv[1]
    settle = float(sys.argv[2]) if len(sys.argv) > 2 else 3.0

    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        try:
            os.execv(program, [program])
        except Exception:
            os._exit(127)

    deadline = time.time() + settle
    alive = True
    try:
        fcntl.fcntl(fd, fcntl.F_SETFL, os.O_NONBLOCK)
    except OSError:
        pass
    while time.time() < deadline:
        # Drain, or a chatty startup fills the pty buffer and the child blocks
        # on write — which would look exactly like "alive and healthy".
        try:
            os.read(fd, 1 << 16)
        except OSError:
            pass
        done, _ = os.waitpid(pid, os.WNOHANG)
        if done == pid:
            alive = False
            break
        time.sleep(0.1)

    if alive:
        try:
            os.kill(pid, signal.SIGKILL)
        except OSError:
            pass
        try:
            os.waitpid(pid, 0)
        except OSError:
            pass
    try:
        os.close(fd)
    except OSError:
        pass
    return 0 if alive else 1


if __name__ == "__main__":
    sys.exit(main())
