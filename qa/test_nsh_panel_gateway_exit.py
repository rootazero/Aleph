"""Exercise the real fixture context and child teardown across collector errors."""
import json
import os
from pathlib import Path
import signal
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import nsh_panel_gateway as gateway


class GatewayExitTests(unittest.TestCase):
    def exercise_failure(self, stop_server):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "http-gateway"
            binary.write_text(f"#!{sys.executable}\n" + '''import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"fixture asset")
HTTPServer(("127.0.0.1", int(sys.argv[2])), Handler).serve_forever()
''')
            binary.chmod(0o755)
            output = root / "evidence"
            with self.assertRaisesRegex(ValueError, "collector-failure"):
                with gateway.isolated_gateway(binary, output) as receipt:
                    if stop_server:
                        os.kill(json.loads(receipt.read_text())["pid"], signal.SIGTERM)
                        time.sleep(0.1)
                    raise ValueError("collector-failure")
            cleanup = json.loads((output / "gateway-cleanup.json").read_text())
            self.assertTrue(cleanup["fixture_removed"])
            self.assertTrue(all(not child["forced"] for child in cleanup["children"]))
            if stop_server:
                self.assertIn("panel_assets_error", cleanup)
            else:
                self.assertTrue(cleanup["panel_assets_unchanged"])

    def test_exit_between_poll_and_signal_preserves_primary_and_joins_provider(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "http-gateway"
            binary.write_text(f"#!{sys.executable}\n" + '''import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"fixture asset")
HTTPServer(("127.0.0.1", int(sys.argv[2])), Handler).serve_forever()
''')
            binary.chmod(0o755)
            output, target, seen = root / "evidence", [], []
            poll = gateway.subprocess.Popen.poll
            def exit_after_poll(child):
                if child not in seen:
                    seen.append(child)
                status = poll(child)
                if target and child.pid == target[0] and status is None:
                    os.killpg(child.pid, signal.SIGTERM)
                    child.wait(timeout=5)
                    return None
                return status
            try:
                with patch.object(gateway.subprocess.Popen, "poll", exit_after_poll):
                    with self.assertRaisesRegex(ValueError, "collector-race-failure"):
                        with gateway.isolated_gateway(binary, output) as receipt:
                            target.append(json.loads(receipt.read_text())["pid"])
                            raise ValueError("collector-race-failure")
                cleanup = json.loads((output / "gateway-cleanup.json").read_text())
                self.assertTrue(cleanup["fixture_removed"])
                self.assertEqual(len(cleanup["children"]), 2)
                self.assertTrue(all(child["exit_code"] is not None for child in cleanup["children"]))
            finally:
                for child in seen:
                    if poll(child) is None:
                        os.killpg(child.pid, signal.SIGTERM)
                        child.wait(timeout=5)

    def test_collector_error_still_verifies_assets_and_joins_children(self):
        self.exercise_failure(False)

    def test_asset_error_preserves_collector_error_and_still_joins_children(self):
        self.exercise_failure(True)
