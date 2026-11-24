#!/usr/bin/env python3
"""Hold an actual Memory model request until the lifecycle probe releases it."""
import io
import json
from pathlib import Path
import sys
import threading
from http.server import ThreadingHTTPServer

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "picker_nav"))
import mock_provider

RELEASE = threading.Event()
LOCK = threading.Lock()
STATE = {"marker": "", "entered": 0, "completed": 0}


class Handler(mock_provider.H):
    def reply(self, body):
        raw = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        with LOCK:
            state = dict(STATE)
        self.reply(state)

    def _do_post(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        if self.path == "/arm":
            with LOCK:
                STATE.update(marker=json.loads(body)["marker"], entered=0, completed=0)
                RELEASE.clear()
            self.reply({"armed": True})
            return
        if self.path == "/release":
            RELEASE.set()
            self.reply({"released": True})
            return
        with LOCK:
            held = bool(STATE["marker"]) and STATE["marker"] in body.decode()
            if held:
                STATE["entered"] += 1
        if held and not RELEASE.wait(timeout=60):
            self.send_error(504, "probe did not release the controlled Memory request")
            return
        original = self.rfile
        self.rfile = io.BytesIO(body)
        try:
            super()._do_post()
        finally:
            self.rfile = original
            if held:
                with LOCK:
                    STATE["completed"] += 1


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
