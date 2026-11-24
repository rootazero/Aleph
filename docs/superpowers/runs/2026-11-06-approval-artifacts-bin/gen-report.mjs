#!/usr/bin/env node
// Builds the review findings table from the first-pass findings, second-pass findings,
// diff-check concerns and every fix report produced by the fix workers.
import fs from 'node:fs'

const ROOT = '/home/zou/data/workspace/Aleph-wt-review/.review/rd-2026-11-06-approval-artifacts-bin'
const FIX_BASES = [
  'fix-approval', 'fix-artifacts', 'fix-migrate', 'fix-qagateway', 'fix-servercmd',
  'fix-serverstart', 'fix-builder', 'fix-handlers', 'fix-wave2a', 'fix-wave2b',
  'fix-wave3a', 'fix-wave3b', 'fix-wave3c', 'fix-wave3d', 'fix-w4a', 'fix-w4b', 'fix-w4c',
]

function readFix(worktree, file) {
  const p = `/home/zou/data/workspace/Aleph-wt-${worktree}/${ROOT.split('/home/zou')[0] ? '' : ''}.review/rd-2026-11-06-approval-artifacts-bin/fixes/${file}`
  const q = `/home/zou/data/workspace/Aleph-wt-${worktree}/.review/rd-2026-11-06-approval-artifacts-bin/fixes/${file}`
  for (const cand of [q, p]) if (fs.existsSync(cand)) return fs.readFileSync(cand, 'utf8')
  return null
}

function verdictsFrom(text, prefix) {
  const out = {}
  if (!text) return out
  const blocks = text.split(/\n(?=#{1,4} )/)
  let current = null
  for (const block of blocks) {
    const header = block.split('\n')[0]
    const level = (header.match(/^#+/) || [''])[0].length
    const id = (header.match(/^#{1,4}\s+([A-Za-z0-9_-]+)/) || [])[1]
    if (level <= 2 && id && id !== 'Verdict' && id !== 'Verdicts' && id !== 'Summary') current = id
    const seg = block.length > 600 ? block : block
    const vi = seg.toLowerCase().indexOf('verdict')
    const hay = vi >= 0 ? seg.slice(vi + 7, vi + 200) : (level > 2 && current ? seg.slice(0, 200) : '')
    const m = hay.match(/partially[- ]?fixed|not[- ]applicable|deferred|skipped[^.\n]*|\bfixed\b/i)
    if (!m) continue
    if (id && /verdict/i.test(id)) {
      if (current) out[prefix ? `${prefix}:${current}` : current] = m[0].trim()
      continue
    }
    if (current) out[prefix ? `${prefix}:${current}` : current] = m[0].trim()
  }
  return out
}

const V = {}
const absorb = (o) => Object.assign(V, o)
absorb(verdictsFrom(readFix('fix-approval', 'B1-B2-B3.md')))
absorb(verdictsFrom(readFix('fix-artifacts', 'B4.md')))
absorb(verdictsFrom(readFix('fix-migrate', 'B13.md')))
absorb(verdictsFrom(readFix('fix-qagateway', 'B14.md'), 'B14'))
absorb(verdictsFrom(readFix('fix-handlers', 'B11.md')))
absorb(verdictsFrom(readFix('fix-builder', 'B10-B12.md')))
absorb(verdictsFrom(readFix('fix-serverstart', 'B8.md')))
const b679 = verdictsFrom(readFix('fix-servercmd', 'B6-B7-B9.md'))
for (const [k, v] of Object.entries(b679)) V[/^F-/.test(k) ? `B7:${k}` : k] = v
absorb(verdictsFrom(readFix('fix-wave2a', 'wave2a.md')))
absorb(verdictsFrom(readFix('fix-wave2b', 'wave2b.md')))
absorb(verdictsFrom(readFix('fix-wave3a', 'w3a.md')))
absorb(verdictsFrom(readFix('fix-wave3b', 'w3b.md')))
absorb(verdictsFrom(readFix('fix-wave3c', 'w3c.md')))
absorb(verdictsFrom(readFix('fix-wave3d', 'w3d.md')))
absorb(verdictsFrom(readFix('fix-w4a', 'w4a.md')))
absorb(verdictsFrom(readFix('fix-w4b', 'w4b.md')))
absorb(verdictsFrom(readFix('fix-w4c', 'w4c.md')))

// Manually reconciled verdicts (evidence documented in the report).
const MANUAL = {
  'B3-002': 'fixed (wave 2a: node approval is denied when no operator surface received it)',
  'B10-002': 'fixed (wave 3c: config watcher task is captured and torn down)',
  'B10-003': 'pending (wave 4c)',
  'B5-001': 'pending (wave 4b)',
  'B3-003': 'pending (wave 4c)',
  'B3-004': 'pending (wave 4c)',
}
const DEFERRED = new Set([
  'B1-05', 'B7:F-06', 'B8-07', 'B11-02', 'B11-04', 'B12-P3-02', 'B12-P3-03',
  'B14:F-11', 'B10-S2-03',
])

const rows = []
const push = (id, priority, title, batch, location) => {
  const key = batch === 'B7' ? `B7:${id}` : batch === 'B14' ? `B14:${id}` : id
  let verdict = V[key] || MANUAL[id] || null
  if (!verdict && DEFERRED.has(key)) verdict = 'deferred (Jev triage: cosmetic / needs redesign)'
  if (!verdict && DEFERRED.has(id)) verdict = 'deferred (Jev triage: cosmetic / needs redesign)'
  if (!verdict) verdict = 'UNKNOWN'
  const norm = verdict === 'FIXED' ? 'fixed' : verdict
  rows.push({ id, priority, title: title.replace(/\s+/g, ' '), batch, location, verdict: norm })
}

// First pass
const fdir = `${ROOT}/findings`
for (const file of fs.readdirSync(fdir)) {
  const text = fs.readFileSync(`${fdir}/${file}`, 'utf8')
  const lines = text.split('\n')
  const batch = file.replace('.md', '')
  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(/^#{1,4}\s+([A-Za-z0-9_-]+)\s+\[(P[0-3])\]\s+(.*)$/)
    if (!m) continue
    let location = ''
    for (let j = i + 1; j < Math.min(i + 10, lines.length); j++) {
      const l = lines[j].match(/^- Location:\s*(.*)$/)
      if (l) { location = l[1].trim().replace(/\s+/g, ' '); break }
    }
    push(m[1], m[2], m[3], batch, location)
  }
}

// Second pass (adversarial re-review)
const sdir = `${ROOT}/second`
if (fs.existsSync(sdir)) {
  for (const file of fs.readdirSync(sdir)) {
    const text = fs.readFileSync(`${sdir}/${file}`, 'utf8')
    for (const m of text.matchAll(/^#{2,4}\s+([A-Za-z0-9_-]+)\s+\[(P[0-3])\]\s+(.*)$/gm)) {
      const seg = text.slice(text.indexOf(m[0]), text.indexOf(m[0]) + 800)
      const l = seg.match(/^- [Ll]ocation:\s*(.*)$/m)
      push(m[1], m[2], m[3], `second/${file.replace('.md', '')}`, l ? l[1].trim().replace(/\s+/g, ' ') : '')
    }
  }
}

// Diff-check concerns (raised while reviewing the fix diffs)
const ddir = `${ROOT}/diffcheck`
const DC_MAP = {
  'R1-approval': { 'DC-01': 'W3A-DC-01', 'DC-02': 'W3A-DC-02' },
  'R2-artifacts-migrate': { 'DC-01': 'W3B-DC-01', 'DC-02': 'W3B-DC-02', 'DC-03': 'W3B-DC-03', 'DC-04': 'W3B-DC-04' },
  'R3-servercmd': {
    'DC-01': 'W3C-DC-01', 'DC-02': 'W3D-DC-01', 'DC-03': 'W3D-DC-02', 'DC-04': 'W3D-DC-03',
    'DC-05': 'W3D-DC-04', 'DC-06': 'W3C-DC-02', 'DC-07': 'W3D-DC-05', 'DC-08': 'W3D-DC-06',
  },
}
if (fs.existsSync(ddir)) {
  for (const file of fs.readdirSync(ddir)) {
    const text = fs.readFileSync(`${ddir}/${file}`, 'utf8')
    const map = DC_MAP[file.replace('.md', '')] || {}
    for (const m of text.matchAll(/^##\s+(DC-\d+)\s+\[(P[0-3])\]\s+(.*)$/gm)) {
      const target = map[m[1]]
      const verdict = target ? (V[target] || 'UNKNOWN') : 'UNKNOWN'
      rows.push({ id: `${file.replace('.md', '')}/${m[1]}`, priority: m[2], title: m[3], batch: 'diffcheck', location: '', verdict: verdict === 'FIXED' ? 'fixed' : verdict })
    }
  }
}

const counts = {}
for (const r of rows) counts[r.verdict.split(' ')[0]] = (counts[r.verdict.split(' ')[0]] || 0) + 1
const lines = [
  `| # | ID | Batch | P | Finding | Verdict |`,
  `|---|----|-------|---|---------|---------|`,
  ...rows.map((r, i) => `| ${i + 1} | \`${r.id}\` | ${r.batch} | ${r.priority} | ${r.title.slice(0, 150)} | ${r.verdict} |`),
]
fs.writeFileSync('/tmp/findings-table.md', lines.join('\n'))
console.log('rows:', rows.length, JSON.stringify(counts))
console.log('unknown:', rows.filter((r) => r.verdict === 'UNKNOWN').map((r) => `${r.batch}/${r.id}`).join(', ') || 'none')
