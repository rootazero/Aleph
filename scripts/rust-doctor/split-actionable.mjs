import fs from 'fs';
const modules = [
  'components','config','context','core','discovery','tool_metadata','domain','exec','executor','extension','gateway','generation','group_chat','guardrails','harness','init_unified'
];
function isTest(p) {
  return p.includes('\\tests\\') || p.includes('/tests/') || p.endsWith('tests.rs');
}
for (const mod of modules) {
  const data = JSON.parse(fs.readFileSync(`rust-doctor-${mod}.json`, 'utf8'));
  const filtered = data.diagnostics.filter(d => {
    if (isTest(d.file_path)) return false;
    if (d.severity === 'error') return true;
    if (d.rule === 'unwrap-in-production') return true;
    if (['unnecessary-allocation','collect-then-iterate','result-unit-error','unsafe-block-audit','large-enum-variant'].includes(d.rule)) return true;
    return false;
  });
  const out = { ...data, diagnostics: filtered };
  fs.writeFileSync(`rust-doctor-${mod}-actionable.json`, JSON.stringify(out, null, 2));
  console.log(mod, filtered.length);
}
