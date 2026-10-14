const fs = require("fs");
const rd = (p) => fs.readFileSync(p, "utf8");
const nlOf = (s) => (s.includes("\r\n") ? "\r\n" : "\n");
function rep(p, a, b) {
  const s = rd(p);
  if (s.split(a).length - 1 !== 1) throw new Error("anchor count for " + p);
  fs.writeFileSync(p, s.replace(a, () => b));
}
const R = "docs/reports/verify-20261007/";
// window-4 report: add later-update line after first line
{
  const p = R + "task56-native-real-window-4.md";
  let s = rd(p);
  const nl = nlOf(s);
  const i = s.indexOf(nl);
  const add = "**Later update / 后续更新：** the user approved one final fifth window (`MAX_WINDOWS` 4→5); it FAILED at `reveal_browser` (`dom_not_as_expected`), cleanup clean; see `task56-native-real-window-5.md`. Statements below are the window-4 historical state. / 用户批准最终第五窗口，已失败于 `reveal_browser`，见 `task56-native-real-window-5.md`。";
  s = s.slice(0, i) + nl + nl + add + s.slice(i);
  fs.writeFileSync(p, s);
}
rep(R + "task56-real-qa-plan.md", "see `task56-native-real-window-4.md`)", "see `task56-native-real-window-4.md`; a final fifth window, `MAX_WINDOWS=5`, was later user-approved and failed at `reveal_browser`, see `task56-native-real-window-5.md`; budget now exhausted)");
rep(R + "task56-real-harness.md", "see `task56-native-real-window-3.md` and `task56-native-real-window-4.md`.", "see `task56-native-real-window-3.md` and `task56-native-real-window-4.md`; a final user-approved fifth window (`MAX_WINDOWS` 4→5) failed at `reveal_browser`, see `task56-native-real-window-5.md`.");
{
  const p = "docs/reference/BROWSER_LIVE_VIEWER.md";
  const s = rd(p);
  const nl = nlOf(s);
  const lines = s.split(nl);
  const k = lines.findIndex((l) => l.startsWith("> - **Real Native window #56"));
  if (k < 0) throw new Error("no anchor");
  lines.splice(
    k + 1,
    0,
    "> - **Real Native window #56 fifth/final (2026-10-12, user-approved `MAX_WINDOWS` 4→5; narrow, PARTIAL, not production ready):** overall **FAIL** (exit 1, cleanup CLEAN) at `reveal_browser` — `dom_not_as_expected` (`ack=false`, `resourceVisible=false`, `pairingNotice=false`) after a fresh Gateway build/restart (PID 1740); the run never reached the save / reload / same-device phases, root cause not established; Native budget exhausted; see `docs/reports/verify-20261007/task56-native-real-window-5.md`.",
    "> - **真机 Native 窗口 #56 第 5（最终）窗口（2026-10-12，用户批准 `MAX_WINDOWS` 4→5；PARTIAL，not production ready）：** 整体 **FAIL**（退出码 1，cleanup CLEAN），失败于 `reveal_browser`（`dom_not_as_expected`）；未到达保存 / reload / 同设备阶段，根因未确定；Native 预算已耗尽；详见 `docs/reports/verify-20261007/task56-native-real-window-5.md`。"
  );
  fs.writeFileSync(p, lines.join(nl));
}
console.log("ok");
