import fs from 'fs';
const input = process.argv[2] || 'rust-doctor-alephcore.json';
const output = process.argv[3] || 'rust-doctor-alephcore-fixed.json';
let s = fs.readFileSync(input, 'utf8');
let out = '';
const hex = /[0-9a-fA-F]/;
for (let i = 0; i < s.length; i++) {
  const c = s[i];
  if (c === '\\' && i + 1 < s.length) {
    const n = s[i + 1];
    if ('"\\/bfnrt'.includes(n)) {
      out += c + n;
      i++;
      continue;
    }
    if (n === 'u' && i + 5 < s.length && hex.test(s[i+2]) && hex.test(s[i+3]) && hex.test(s[i+4]) && hex.test(s[i+5])) {
      out += s.slice(i, i + 6);
      i += 5;
      continue;
    }
    // Invalid escape: turn the backslash into a literal backslash by doubling it.
    out += '\\\\';
    continue;
  }
  if (c === '\\') {
    // trailing backslash
    out += '\\\\';
    continue;
  }
  out += c;
}
fs.writeFileSync(output, out);
try {
  const j = JSON.parse(out);
  console.log('ok keys', Object.keys(j));
  console.log('score', j.score);
  console.log('diagnostics', j.diagnostics ? j.diagnostics.length : 'none');
  console.log('files', j.files ? j.files.length : 'none');
} catch (e) {
  console.log('parse fail', e.message);
  process.exit(1);
}
