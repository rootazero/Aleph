Gate=Pass(engine=Chrome/154.0.8037.98, os=win32, cases=45/45)
Scope: verdict binds ONLY engine=Chrome/154.0.8037.98 + os=win32 + the cases listed; unmeasured platforms/engines default to NOT enabling dev-preview network grants. Capability probe, not product code.
Run: node .analysis/context-network-probe/probe.mjs ; elapsed=58s (300s hard timeout armed); 2 Chrome instances (hardened + no-proxy baseline as positive control), 1 root CDP conn each
Setup: own loopback receivers only (127.0.0.1/.2/.3, ::1); proxy-side static DNS + Chrome --host-resolver-rules (MAP * ~NOTFOUND, no real DNS); per-context proxyServer + proxyBypassList=<-loopback>; global --proxy-server=deny-all; --disable-quic --webrtc-ip-handling-policy=disable_non_proxied_udp (the --force-webrtc-... spelling did NOT block STUN UDP in exp); TLS self-signed + --ignore-certificate-errors-spki-list
Legend: name[acc=receiver TCP accepts, hit=receiver requests/upgrades]; denied PASS = acc=0,hit=0, proxy saw+denied it, 0 proxy allows
ALLOWED 8/8:
  ok  http-navigation[hit=1]
  ok  http-fetch[acc=1,hit=1]
  ok  ws[acc=1,hit=1]
  ok  https-fetch(CONNECT+TLS)[acc=1,hit=1]
  ok  wss[acc=1,hit=1]
  ok  hmr-ws-longlived[hit=7]
  ok  hmr-wss-longlived[hit=7]
  ok  Y-own-origin-navigation[hit=1]
DENIED 20/20:
  ok  wrong-origin.http[acc=0,hit=0]
  ok  wrong-origin.ws[acc=0,hit=0]
  ok  wrong-origin.ip-literal[acc=0,hit=0]
  ok  wrong-port.http[acc=0,hit=0]
  ok  wrong-port.ws[acc=0,hit=0]
  ok  wrong-protocol.https-to-plain-port[acc=0,hit=0]
  ok  wrong-protocol.wss-to-plain-port[acc=0,hit=0]
  ok  wrong-protocol.http-to-tls-port[acc=0,hit=0]
  ok  wrong-protocol.ws-to-tls-port[acc=0,hit=0]
  ok  resolve-private.http(rebind.test->127.0.0.2)[acc=0,hit=0]
  ok  resolve-private.ws[acc=0,hit=0]
  ok  cross-context.http(Y->X-origin)[acc=0,hit=0]
  ok  cross-context.ws[acc=0,hit=0]
  ok  cross-context.https[acc=0,hit=0]
  ok  cross-context.rebind-origin[acc=0,hit=0]
  ok  cross-context.popup[acc=0,hit=0]
  ok  cross-context.worker(fetch+ws)[acc=0,hit=0]
  ok  no-grant.fetch+ws(about:blank in N)[acc=0,hit=0]
  ok  no-grant.navigation[acc=0,hit=0]
  ok  default-context.navigation(global deny proxy)[acc=0,hit=0]
BYPASS 10/10:
  ok  loopback-127.0.0.1-bait[acc=0,hit=0]
  ok  loopback-localhost-bait[acc=0,hit=0]
  ok  loopback-[::1]-bait[acc=0,hit=0]
  ok  loopback-ws-127.0.0.1-bait[acc=0,hit=0]
  ok  sendBeacon[acc=0,hit=0]
  ok  preconnect+dns-prefetch+prefetch[acc=0,hit=0]
  ok  webrtc-udp+turn-tcp(disable_non_proxied_udp)[acc=0,hit=0]
  ok  webtransport-udp[acc=0,hit=0]
  ok  forced-quic-udp(--disable-quic)[acc=0,hit=0]
  ok  direct-accepts(all receivers, remote port not a proxy upstream)[acc=0,hit=0]
INHERIT 5/5:
  ok  popup-same-context[hit=1]
  ok  worker-same-context(fetch+ws)[hit=2]
  ok  worker-does-not-widen(evil.test)[acc=0,hit=0]
  ok  cross-origin-iframe(https:B)[hit=1]
  ok  cross-context-popup-stays-in-opener-context[hit=0]
REVOKE 2/2:
  ok  existing-ws-closed<=1s[hit=-]
  ok  new-connections-accept=0[acc=0,hit=0]
Revoke: {"pageSideCloseLatencyMs":{"ws":0,"wss":0},"receiverSideCloseLatencyMs":[0,0],"maxMs":0}
Baseline(no proxy, default WebRTC, QUIC allowed; positive control): {"udpW":5,"tcpT":1,"udpWT":6,"udpQ":6,"rtcErr":null,"wt":"timeout"}
Hardened bypass bait counts: {"webrtcStunUdp":0,"webrtcTurnTcp":0,"webTransportUdp":0,"forcedQuicUdp":0} ; direct accepts: {"total":16,"viaProxy":16,"direct":0,"directBy":{}}
Cleanup: {"chromeProcsWithOurProfilePrefix":0,"profilesRemoved":true,"tmpDirs":3,"profilePrefix":"ctxnet-126f92fa-"}
Not tested: QUIC server side (only forced-QUIC UDP egress), service worker, OOPIF beyond one iframe, real DNS leak (blocked by MAP * ~NOTFOUND, not observed), non-win32, non-Chrome. Details: evidence.json.
