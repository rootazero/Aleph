// Group clippy warnings by file/lint and mark files changed on this branch.
const fs = require('fs');
const path = require('path');
const R = __dirname;
const changed = new Set(fs.readFileSync(path.join(R, 'changed.txt'), 'utf8').split(/\r?\n/).filter(Boolean));
for (const p of process.argv.slice(2)) {
  const lines = fs.readFileSync(path.join(R, `clippy-${p}.log`), 'utf8').split(/\r?\n/);
  const hits = new Map();
  let lint = null;
  for (const l of lines) {
    const w = l.match(/^warning: (.*)$/);
    if (w) { lint = w[1]; continue; }
    const loc = l.match(/^\s+--> (.+?):(\d+):\d+$/);
    if (loc && lint) {
      const f = loc[1].replace(/\\/g, '/');
      const key = `${changed.has(f) ? 'CHANGED' : 'old    '} ${f} :: ${lint.slice(0, 70)}`;
      hits.set(key, (hits.get(key) || 0) + 1);
      lint = null;
    }
  }
  console.log(`== ${p}`);
  [...hits].sort().forEach(([k, n]) => console.log(`${n} ${k}`));
}
