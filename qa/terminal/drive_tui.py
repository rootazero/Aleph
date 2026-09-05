#!/usr/bin/env python3
"""Drive the REAL `aleph-tui` binary in a PTY against the booted gateway.

`interfaces/tui/` has never been run against a live server by any fixture.
Its agent panel renders `shared_ui_logic::entry_name` — `program ?? agent ??
label` — from a live `runtime.agents.list` plus an `events.subscribe`, and
every test of it renders a hand-built `AgentPanelData` into a test backend.
That is the same shape the phase-1 defect hid in: the renderer was always
right, and nothing checked that the value reaching it came from the wire.

Three observations and two flips, the shape `stage_quiet` uses:

  1. before `/agentpanel`  the program name is NOT on screen
  2. after                 the header AND the program name are
  3. after again           gone

One observation cannot tell "the panel works" from "that text was on screen
anyway" — the session list, the status bar and the slash-command menu all
print things, and a substring search over a whole TUI frame is exactly the
assertion that would not notice (判据 §2).

Usage:  drive_tui.py <binary> <ws-url> <expected program name>
"""
import fcntl
import os
import pty
import re
import signal
import sys
import time

CSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[()][B0]|\x1b[=>]")

rc = 0


def check(ok, label, detail=""):
    global rc
    print(f"  [{'PASS' if ok else 'FAIL'}] {label}" + (f" — {detail}" if detail else ""))
    if not ok:
        rc = 1


class Tui:
    """A pty running the TUI, read ONE FRAME at a time.

    Reading a time window instead was the first version and it is wrong in
    the direction that matters: the window is a STREAM, so it still holds the
    frames painted before the keystroke landed. `/agentpanel` off therefore
    "failed" while the panel really had gone — the window contained the last
    frame that still had it.

    A frame is forced by changing the window size, which makes the kernel
    deliver SIGWINCH and ratatui repaint everything. The HEIGHT is what
    changes: the agent panel is a fixed 28-column strip, so nudging the width
    would change the layout under the assertion. The same nudge is used for
    every observation, so the three are comparable.
    """

    def __init__(self, binary, url):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.environ["TERM"] = "xterm-256color"
            os.environ["LINES"] = "40"
            os.environ["COLUMNS"] = "120"
            try:
                os.execv(binary, [binary, "--server", url])
            except Exception:
                os._exit(127)
        # A real window size, or ratatui lays out into an 80x24 default and
        # the 28-column agent panel may not fit beside the transcript.
        self.rows = 40
        self._resize(self.rows)
        fcntl.fcntl(self.fd, fcntl.F_SETFL, os.O_NONBLOCK)
        self.buf = ""

    def _resize(self, rows):
        import struct
        import termios

        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, 120, 0, 0))

    def frame(self, settle=2.0, window=3.0):
        """Let the last keystroke land, then force ONE repaint and read it."""
        self.drain(settle)  # discard whatever the keystroke itself produced
        self.rows = 41 if self.rows == 40 else 40
        self._resize(self.rows)
        return self.drain(window)

    def drain(self, seconds):
        """Read for `seconds` and return the PLAIN TEXT seen in that window."""
        self.buf = ""
        end = time.time() + seconds
        while time.time() < end:
            try:
                chunk = os.read(self.fd, 1 << 16)
            except (BlockingIOError, OSError):
                chunk = b""
            if chunk:
                self.buf += chunk.decode("utf-8", "replace")
            else:
                time.sleep(0.05)
        return CSI.sub("", self.buf)

    def type(self, text):
        os.write(self.fd, text.encode())

    def close(self):
        try:
            os.kill(self.pid, signal.SIGKILL)
        except OSError:
            pass
        try:
            os.waitpid(self.pid, 0)
        except OSError:
            pass
        try:
            os.close(self.fd)
        except OSError:
            pass


def main():
    binary, url, program = sys.argv[1], sys.argv[2], sys.argv[3]
    t = Tui(binary, url)
    try:
        first = t.frame(settle=5.0)
        if not first.strip():
            check(False, "the TUI painted anything at all", "empty pty output")
            return 1
        print(f"  ... first frame: {len(first)} chars of plain text")
        check(
            program not in first,
            f"before /agentpanel, {program!r} is NOT on screen",
            f"found it already; the later assertion would prove nothing. "
            f"tail={first[-300:]!r}",
        )

        t.type("/agentpanel\r")
        on = t.frame()
        check("agents" in on, "the agent panel header rendered", f"tail={on[-300:]!r}")
        check(
            program in on,
            f"the panel shows {program!r} — a value that could only come from "
            f"`runtime.agents.list` over the live socket",
            f"tail={on[-400:]!r}",
        )

        t.type("/agentpanel\r")
        off = t.frame()
        check(
            program not in off,
            f"toggling again removes {program!r} — so the first appearance was "
            f"the panel and not some other line of the frame",
            f"tail={off[-300:]!r}",
        )
    finally:
        t.close()
    return rc


if __name__ == "__main__":
    sys.exit(main())
