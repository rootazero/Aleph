"""HTTP artifact provenance tests use a real local server, not Gateway mocks."""
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import tempfile
import threading
import unittest

import nsh_panel_gateway as gateway


class ServedArtifactTests(unittest.TestCase):
    def test_http_bytes_are_retained_and_a_later_mutation_is_visible(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            public = root / "public"
            public.mkdir()
            for name in ["index.html", "aleph_panel.js", "aleph_panel_bg.wasm", "tailwind.css"]:
                (public / name).write_bytes(("actual-" + name).encode())
            server = ThreadingHTTPServer(("127.0.0.1", 0), partial(SimpleHTTPRequestHandler, directory=str(public)))
            worker = threading.Thread(target=server.serve_forever)
            worker.start()
            try:
                url = f"http://127.0.0.1:{server.server_port}/"
                before = gateway.capture_panel_assets(url, root / "capture")
                self.assertEqual(set(before), {"index.html", "aleph_panel.js", "aleph_panel_bg.wasm", "tailwind.css"})
                for name, item in before.items():
                    self.assertEqual(Path(item["capture"]).read_bytes(), (public / name).read_bytes())
                    self.assertEqual(item["sha256"], gateway.file_sha256(public / name))
                self.assertTrue(gateway.panel_assets_unchanged(url, before))
                (public / "aleph_panel.js").write_text("changed")
                self.assertFalse(gateway.panel_assets_unchanged(url, before))
            finally:
                server.shutdown()
                worker.join()
                server.server_close()


if __name__ == "__main__":
    unittest.main()
