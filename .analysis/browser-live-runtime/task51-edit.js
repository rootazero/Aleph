const fs=require('fs');
const p='qa/browser_live/tests/viewer.spec.ts';
let s=fs.readFileSync(p,'utf8');
const crlf=s.includes('\r\n'); s=s.replace(/\r\n/g,'\n');
const oldOpen=`async function openViewer(page: Page, name: string) {
  await page.goto(viewerUrl(name), { waitUntil: 'domcontentloaded' });
`;
const newOpen=`async function openViewer(page: Page, name: string) {
  // A failed goto would echo the full launch URL (fragment credential) in the
  // Playwright error, so rethrow a fixed message instead.
  try {
    await page.goto(viewerUrl(name), { waitUntil: 'domcontentloaded' });
  } catch {
    throw new Error(\`navigation to the \${name} viewer URL failed (details withheld: URL carries a credential)\`);
  }
`;
if(!s.includes(oldOpen)) throw new Error('open');
s=s.replace(oldOpen,newOpen);
const a=s.indexOf("test('grant-boundary:"), b=s.indexOf("test('reap:");
const t=fs.readFileSync(process.argv[2],'utf8');
s=s.slice(0,a)+t+s.slice(b);
if(crlf) s=s.replace(/\n/g,'\r\n');
fs.writeFileSync(p,s);
