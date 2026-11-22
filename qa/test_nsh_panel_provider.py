"""Real HTTP checks for the deterministic model endpoint, not Gateway acceptance."""
import json
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from urllib.request import Request, urlopen


class ProviderWireTests(unittest.TestCase):
    def test_tool_result_finishes_the_model_step_without_republishing(self):
        with tempfile.TemporaryDirectory() as directory:
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                port = listener.getsockname()[1]
            child = subprocess.Popen([sys.executable, str(Path(__file__).with_name("nsh_panel_provider.py")), "--port", str(port), "--record", str(Path(directory) / "requests.jsonl")], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            def post(body):
                request = Request(f"http://127.0.0.1:{port}/v1/messages", data=json.dumps(body).encode(), headers={"content-type": "application/json"})
                with urlopen(request, timeout=2) as response:
                    return response.read().decode()
            try:
                body = {"tools": [{"name": "artifact_publish"}], "messages": [{"role": "user", "content": "proof"}]}
                deadline = time.monotonic() + 3
                while True:
                    try:
                        answer = json.loads(post(body))
                        break
                    except OSError:
                        if time.monotonic() > deadline:
                            raise
                        time.sleep(0.01)
                self.assertEqual(answer["stop_reason"], "tool_use")
                self.assertEqual(answer["content"][1]["name"], "artifact_publish")
                body["messages"].append({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "phase9-artifact", "content": "stored"}]})
                self.assertEqual(json.loads(post(body))["stop_reason"], "end_turn")
                body["stream"] = True
                stream = post(body)
                self.assertIn('event: message_stop', stream)
                self.assertIn('"text": "PHASE9_RUNTIME_PROOF"', stream)
                self.assertNotIn('"type": "tool_use"', stream)
            finally:
                child.terminate()
                child.wait(timeout=5)


if __name__ == "__main__":
    unittest.main()
