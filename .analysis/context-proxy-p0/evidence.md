# P0 per-context proxy probe evidence

P0=Supported
Capability only: even if Supported, Task0 Gate remains Blocked; no security Gate validated.
- Engine: Chrome/154.0.8037.98; Node v24.13.0; win32; temp own profile (removed=true); Chrome CDP bound 127.0.0.1 only
- Command: node .analysis/context-proxy-p0/probe.mjs ; exit=0; elapsed=4.1s (300s hard timeout armed before launch)
- Root CDP connections: 1
- CDP methods (tokens redacted): [{"method":"Target.createBrowserContext","params":{"proxyServer":"http://127.0.0.1:50115","proxyBypassList":"<-loopback>","disposeOnDetach":true}},{"method":"Target.createBrowserContext","params":{"proxyServer":"http://127.0.0.1:50116","proxyBypassList":"<-loopback>","disposeOnDetach":true}},{"method":"Target.createTarget","params":{"url":"http://127.0.0.1:50114/T?","browserContextId":"F8C2AD1F95AB95CE831A69B8CF9E238E"}},{"method":"Target.createTarget","params":{"url":"http://127.0.0.1:50114/T?","browserContextId":"F78C0BA35A68E69D81F04DBFE22F3D84"}},{"method":"Target.getTargets","params":{}},{"method":"Target.disposeBrowserContext","params":{"browserContextId":"F8C2AD1F95AB95CE831A69B8CF9E238E"}},{"method":"Target.disposeBrowserContext","params":{"browserContextId":"F78C0BA35A68E69D81F04DBFE22F3D84"}}]
- Contexts: {"X":"F8C2AD..","Y":"F78C0B..","distinct":true}
- getTargets membership: {"X":{"found":true,"browserContextIdMatches":true},"Y":{"found":true,"browserContextIdMatches":true}}
- proxyX received (by token X/Y/other, CONNECT): {"X":1,"Y":0,"other":1,"connect":4}
- proxyY received: {"X":0,"Y":1,"other":1,"connect":3}
- fixture requests: {"X":1,"Y":1,"other":2}
- Assertions: X(proxyX>0,proxyY=0)=true ; Y(proxyY>0,proxyX=0)=true
- Cleanup: {"ownPidGone":true,"procsWithProfileLeft":2,"profileRemoved":true} (own child handle taskkill /T only; own servers closed)
- Not tested: WS/HMR/QUIC/DNS rebind/cookie/popup/revoke. Destinations: own loopback fixture only.
- Post-check: chrome.exe processes carrying p0ctx- profile = see below; procsWithProfileLeft=2 is the cleanup PowerShell query matching its own command line (profile name is in its text), not Chrome.
- Notes: CONNECT hits (X=4,Y=3) are Chrome-internal non-loopback HTTPS attempts routed to each context's own proxy; our proxies answered 403 and forwarded nothing, so no external traffic left the machine. "other" = favicon-style loopback requests (no token). Not an isolation claim beyond the X/Y token assertion.
