import fs from 'fs';
const modules = [
  'components','config','context','core','discovery','tool_metadata','domain','exec','executor','extension','gateway','generation','group_chat','guardrails','harness','init_unified'
];
const data = JSON.parse(fs.readFileSync('rust-doctor-alephcore-fixed.json','utf8'));
const diags = data.diagnostics || [];
for (const mod of modules) {
  const filtered = diags.filter(d => {
    const p = d.file_path || '';
    return p.includes(`src\\${mod}\\`) || p.includes(`src/${mod}/`);
  });
  const out = { ...data, diagnostics: filtered };
  fs.writeFileSync(`rust-doctor-${mod}.json`, JSON.stringify(out, null, 2));
  console.log(mod, filtered.length);
}
