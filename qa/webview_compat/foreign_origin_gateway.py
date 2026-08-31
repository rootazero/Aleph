#!/usr/bin/env python3
"""A stand-in Gateway on a NON-loopback origin, for the `marker-origin` scenario.

The question this fixture exists to answer is *when* the shell's
`SHELL_MARKER_JS` reaches a document the shell did not serve. Two facts make
that answerable without a second machine:

  * `gateway_probe::probe_reachable` only requires `/ready` to answer 200/503,
    so a Gateway can be faked in thirty lines;
  * a WKWebView user script has no origin concept at all, so *any* origin that
    is not the Tauri custom protocol exercises the same code path. The LAN
    address of this machine is a genuinely foreign origin to the webview —
    it does not know the bytes come from localhost.

The served document's first inline `<script>` occupies exactly the slot
`interfaces/webchat/baseline-probe.js` occupies in the real Panel, and reports
`data-shell` / `data-platform` at four phases over a **synchronous** XHR, so
each report is ordered before anything else that phase would do. Phase 1 is
the load-bearing one: if the marker is already present there, no stylesheet
keyed on `[data-shell="aleph-tauri"]` can flash an unstyled first paint.

Reports go to stdout, one `REPORT` line per phase, for run.sh to read.
"""

import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

PAGE = b"""<!doctype html>
<html><head><meta charset="utf-8"><title>shell marker probe</title>
<script>
(function () {
  var el = document.documentElement;
  function rec(phase) {
    var q = '/r?phase=' + phase
      + '&shell=' + encodeURIComponent(String(el.getAttribute('data-shell')))
      + '&platform=' + encodeURIComponent(String(el.getAttribute('data-platform')))
      + '&isTauri=' + (typeof window.isTauri)
      + '&internals=' + (typeof window.__TAURI_INTERNALS__);
    try { var x = new XMLHttpRequest(); x.open('GET', q, false); x.send(); } catch (e) {}
  }
  rec('1-inline-head');
  document.addEventListener('DOMContentLoaded', function () { rec('2-domcontentloaded'); });
  window.addEventListener('load', function () {
    rec('3-load');
    setTimeout(function () { rec('4-after-load'); }, 500);
  });
})();
</script></head>
<body style="font:14px system-ui;padding:2rem">shell marker probe</body></html>
"""

FIELDS = ("phase", "shell", "platform", "isTauri", "internals")


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):  # noqa: N802 - BaseHTTPRequestHandler's spelling
        parsed = urlparse(self.path)
        if parsed.path == "/ready":
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(b'{"ready":true}')
            return
        if parsed.path == "/r":
            q = {k: v[0] for k, v in parse_qs(parsed.query).items()}
            print("REPORT " + " ".join(f"{k}={q.get(k)}" for k in FIELDS), flush=True)
            self.send_response(204)
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.end_headers()
        self.wfile.write(PAGE)

    def log_message(self, *_args):
        """Silence the default per-request stderr log; REPORT lines are the output."""


if __name__ == "__main__":
    # Default to an ephemeral port and announce the one the kernel actually
    # handed out. A fixed port lets a *previous* run's stray server answer the
    # caller's readiness check, so the harness proves a live Gateway and then
    # reads an empty report log — the shell was talking to the other process.
    # Bind FIRST and print after: a "listening" line emitted ahead of the bind
    # is a label that survives its own failure.
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 0
    server = ThreadingHTTPServer(("0.0.0.0", port), Handler)
    print(f"listening on 0.0.0.0:{server.server_address[1]}", flush=True)
    server.serve_forever()
