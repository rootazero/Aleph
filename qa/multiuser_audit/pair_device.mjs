// Mint a bootstrap ticket bound to a principal and redeem it as a device.
//
//     node pair_device.mjs <loopback-ws-url> <remote-ws-url> <user_id> <device_id>
//
// ## Why two URLs
//
// `resolve_connect_auth` returns `Authorized` for a loopback peer on its first
// line, before it ever looks at a `bootstrap_ticket`. That is the single-machine
// zero-credential guarantee and it is correct — but it means a ticket redeemed
// over `127.0.0.1` creates no device row at all, silently and successfully. The
// first version of this fixture assumed otherwise and reported four failures
// that were its own.
//
// So the two halves run over the two peers they belong to: minting needs
// operator (loopback), redeeming is what a remote Panel does (LAN address).
// This is also the only way the fixture exercises the real pairing path rather
// than a loopback-shaped imitation of it.
//
// ## Why JavaScript
//
// This was `pair_device.py` (asyncio + the `websockets` package). On a Windows
// host the only `python3` on PATH is the WindowsApps stub, so the driver could
// not run at all — and `run.sh` reported that as "device pairing driver failed",
// which reads like a server defect. Node's global `WebSocket` needs nothing
// installed, and the sibling fixtures' drivers are already `.mjs`. The argv
// contract, the printed lines and the exit codes are unchanged: `run.sh` reads
// the exit code, and a reader comparing a run against an older log should not
// have to translate.

const [localUrl, remoteUrl, userId, deviceId] = process.argv.slice(2);
if (!localUrl || !remoteUrl || !userId || !deviceId) {
  console.error("usage: pair_device.mjs <loopback-ws-url> <remote-ws-url> <user_id> <device_id>");
  process.exit(2);
}

const CONNECT_BUDGET_MS = 30_000;
const REPLY_BUDGET_MS = 30_000;
const CLOSE_BUDGET_MS = 5_000;

/**
 * Open a socket, hand `body` an `rpc(method, params, id)`, close it either way.
 *
 * The reply is matched on the request id: event frames share this socket, and a
 * driver that took the next frame to arrive would read a `run.*` broadcast as
 * its own answer.
 */
async function withSocket(url, body) {
  const ws = new WebSocket(url);
  const pending = new Map();
  ws.addEventListener("message", (ev) => {
    let msg;
    try {
      msg = JSON.parse(typeof ev.data === "string" ? ev.data : String(ev.data));
    } catch {
      return;
    }
    if (msg.id === undefined || msg.id === null) return;
    const settle = pending.get(msg.id);
    if (!settle) return;
    pending.delete(msg.id);
    settle(msg);
  });
  await new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`connect timeout after ${CONNECT_BUDGET_MS}ms: ${url}`)),
      CONNECT_BUDGET_MS,
    );
    ws.addEventListener(
      "open",
      () => {
        clearTimeout(timer);
        resolve();
      },
      { once: true },
    );
    ws.addEventListener(
      "error",
      () => {
        clearTimeout(timer);
        reject(new Error(`websocket error: ${url}`));
      },
      { once: true },
    );
  });
  const rpc = (method, params, rid) =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pending.delete(rid);
        reject(new Error(`no reply to ${method} within ${REPLY_BUDGET_MS}ms`));
      }, REPLY_BUDGET_MS);
      pending.set(rid, (msg) => {
        clearTimeout(timer);
        resolve(msg);
      });
      ws.send(JSON.stringify({ jsonrpc: "2.0", id: rid, method, params }));
    });
  try {
    return await body(rpc);
  } finally {
    // WAIT for the close to land rather than firing it and walking away. On
    // Windows, tearing the process down while a close handshake is still in
    // flight aborts the runtime — `Assertion failed: !(handle->flags &
    // UV_HANDLE_CLOSING), src\win\async.c` — AFTER every assertion has already
    // passed, so `run.sh` reads a successful pairing as a failed driver and
    // then takes the "nothing was paired" arm of two later assertions. The
    // budget is the same shape as every other wait here: bounded, and it
    // resolves rather than throwing, because a socket that will not close is
    // not a verdict about the pairing path.
    await new Promise((resolve) => {
      const timer = setTimeout(resolve, CLOSE_BUDGET_MS);
      ws.addEventListener(
        "close",
        () => {
          clearTimeout(timer);
          resolve();
        },
        { once: true },
      );
      try {
        ws.close();
      } catch {
        clearTimeout(timer);
        resolve();
      }
    });
  }
}

/** Print a failure the way the fixture's log has always shown it, and answer 1. */
const fail = (line, detail) => {
  console.error(line);
  if (detail !== undefined) console.error(detail);
  return 1;
};

async function main() {
  let ticket = null;
  const minted = await withSocket(localUrl, async (rpc) => {
    const hello = await rpc("connect", { client_type: "cli" }, 1);
    if (hello.error) return fail(`FAIL connect(loopback): ${JSON.stringify(hello.error)}`);
    const made = await rpc("gateway.ticket.create", { user_id: userId }, 2);
    if (made.error) return fail(`FAIL ticket.create: ${JSON.stringify(made.error)}`);
    ticket = made.result.ticket;
    return 0;
  });
  if (minted !== 0) return minted;

  const redeemed = await withSocket(remoteUrl, async (rpc) => {
    const reply = await rpc(
      "connect",
      {
        client_type: "panel",
        bootstrap_ticket: ticket,
        device_id: deviceId,
        device_name: "QA Panel",
      },
      1,
    );
    if (reply.error) return fail(`FAIL connect(ticket, remote): ${JSON.stringify(reply.error)}`);
    // A remote connection that was NOT handed a device token means the ticket
    // path did not run — which is exactly the failure the loopback short-circuit
    // produces, and it must not read as success.
    const result = reply.result ?? {};
    if (!result.device_token) {
      return fail(
        "FAIL remote connect returned no device token; the ticket was not exchanged",
        JSON.stringify(result, null, 2).slice(0, 600),
      );
    }
    return 0;
  });
  if (redeemed !== 0) return redeemed;

  return withSocket(localUrl, async (rpc) => {
    await rpc("connect", { client_type: "cli" }, 1);
    const listed = await rpc("gateway.devices.list", {}, 2);
    const devices = listed.result?.devices ?? [];
    const mine = devices.filter((d) => d.device_id === deviceId);
    if (mine.length === 0) {
      return fail(
        `FAIL device ${deviceId} absent after redeeming its ticket`,
        JSON.stringify(devices, null, 2),
      );
    }
    // Round-4 gave this list an owner column; the deactivation receipt and the
    // audit line both claim to name the same principal, so check that the three
    // agree rather than trusting any one of them.
    if (mine[0].user_id !== userId) {
      return fail(
        `FAIL device bound to ${JSON.stringify(mine[0].user_id)}, expected ${JSON.stringify(userId)}`,
      );
    }
    // This driver is the third reader of the inventory, so it checks the wire
    // rather than only the two fields it happens to need: the row is built from
    // `aleph_protocol::devices::PairedDeviceRow`, and a key that appears or
    // disappears here is a contract change no Rust test on either side sees on
    // a live server. `connected` is required for the reason the type states —
    // absent, a client would render an unknown as a claim of "offline".
    const expected = [
      "connected",
      "device_id",
      "device_name",
      "display_name",
      "last_seen_at",
      "user_id",
    ];
    const got = Object.keys(mine[0]).sort();
    if (got.join(",") !== expected.join(",")) {
      return fail(
        `FAIL device row keys ${JSON.stringify(got)}, expected ${JSON.stringify(expected)}`,
      );
    }
    console.log(`OK device ${deviceId} paired and bound to ${userId}`);
    return 0;
  });
}

// `process.exitCode` and NOT `process.exit()`: every socket above is closed by
// the time this runs, and forcing the runtime down anyway is what tripped the
// libuv assertion this driver's teardown comment describes.
main().then(
  (code) => {
    process.exitCode = code;
  },
  (err) => {
    // A thrown budget or transport error is a failure of the pairing path, not
    // of the fixture's optimism: name it and exit 1 like every other arm.
    console.error(`FAIL pair_device: ${err?.message ?? err}`);
    process.exitCode = 1;
  },
);
