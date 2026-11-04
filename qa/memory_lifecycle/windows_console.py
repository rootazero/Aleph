"""Windows-native console control event delivery for the Memory lifecycle probe.

Windows has no SIGTERM: `Popen.send_signal(SIGTERM)` is `TerminateProcess`
(a hard kill) and `send_signal(SIGINT)` raises `ValueError`. The aleph gateway
listens for Ctrl-C only (`tokio::signal::ctrl_c()` in
`src/bin/aleph-server/commands/start/helpers.rs`), and tokio only subscribes to
`CTRL_C_EVENT`, so the Windows equivalent of the Unix SIGTERM path is a real
console control event delivered to the owned server process.

Three Windows specifics make that delivery work:

* `SetConsoleCtrlHandler(NULL, TRUE)` marks a process as ignoring Ctrl-C, and
  that attribute is inherited by children. Installing a handler function does
  not clear it, so the parent must clear the attribute *before* spawning the
  server or the server never sees `CTRL_C_EVENT`.
* A console control event reaches only the processes attached to one console.
  The server is therefore spawned with `CREATE_NEW_CONSOLE` so it owns a
  console of its own, and the event is broadcast from a helper process that
  attaches to that console (group 0) instead of from the probe itself, which
  would otherwise join the server console and interrupt the probe too.
* `CTRL_BREAK_EVENT` is *not* a graceful path here: tokio subscribes to
  `CTRL_C_EVENT` only, so an unhandled `CTRL_BREAK_EVENT` falls through to the
  default handler and terminates the process.
"""

import ctypes
import ctypes.wintypes as wt
import json
import os
import subprocess
import sys

CTRL_C_EVENT = 0
CTRL_BREAK_EVENT = 1
CREATE_NEW_CONSOLE = 0x00000010
CREATE_NEW_PROCESS_GROUP = 0x00000200


def _kernel32():
    library = ctypes.WinDLL("kernel32", use_last_error=True)
    library.SetConsoleCtrlHandler.argtypes = [ctypes.c_void_p, wt.BOOL]
    library.SetConsoleCtrlHandler.restype = wt.BOOL
    library.AttachConsole.argtypes = [wt.DWORD]
    library.AttachConsole.restype = wt.BOOL
    library.FreeConsole.restype = wt.BOOL
    library.GenerateConsoleCtrlEvent.argtypes = [wt.DWORD, wt.DWORD]
    library.GenerateConsoleCtrlEvent.restype = wt.BOOL
    return library


def prepare_console_parent():
    """Re-enable Ctrl-C for this process so children inherit an enabled one."""
    return bool(_kernel32().SetConsoleCtrlHandler(None, 0))


def spawn_creation_flags():
    """Give the owned server its own console and its own process group."""
    return CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP


def _deliver(pid, event):
    library = _kernel32()
    library.FreeConsole()
    attached = library.AttachConsole(pid)
    attach_error = ctypes.get_last_error()
    if not attached:
        return {"attached": False, "attach_error": attach_error}
    # The helper shares the server console, so it must ignore the broadcast it
    # is about to generate.
    library.SetConsoleCtrlHandler(None, 1)
    if event == "ctrl_break":
        generated = library.GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid)
    else:
        generated = library.GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0)
    return {
        "attached": True,
        "attach_error": attach_error,
        "generated": bool(generated),
        "generate_error": ctypes.get_last_error(),
        "event": event,
    }


def deliver_control_event(pid, event="ctrl_c"):
    """Deliver a console control event to `pid` from a dedicated helper process."""
    command = [sys.executable, os.path.abspath(__file__), "--deliver", str(pid), event]
    completed = subprocess.run(command, capture_output=True, text=True)
    try:
        result = json.loads(completed.stdout.strip().splitlines()[-1])
    except (ValueError, IndexError):
        result = {"stdout": completed.stdout, "stderr": completed.stderr}
    result["helper_exit_code"] = completed.returncode
    return result


if __name__ == "__main__":
    if len(sys.argv) >= 4 and sys.argv[1] == "--deliver":
        print(json.dumps(_deliver(int(sys.argv[2]), sys.argv[3])))
    else:
        print("usage: windows_console.py --deliver <pid> <ctrl_c|ctrl_break>", file=sys.stderr)
        raise SystemExit(2)
