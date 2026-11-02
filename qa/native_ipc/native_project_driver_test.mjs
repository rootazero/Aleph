import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { createMockPage, extractMonitorScript, renderMonitorScript } from './panel_driver.mjs';
import { buildCommand } from './monitor_contract.mjs';
import { classifyPairingSurface } from './native_project_run.mjs';

const template = extractMonitorScript(fs.readFileSync(new URL('../../desktop/shell/src/main.rs', import.meta.url), 'utf8'));
const script = renderMonitorScript(template, { port: 31234, nonce: 'cd'.repeat(16) });

test('ordinary fresh Panel with no supported auth surface is classified as unavailable without IPC', () => {
  const page = createMockPage({ script });
  page.open();
  const hello = page.events().find(event => event.type === 'hello');
  page.deliver(buildCommand({ run: hello.run, id: 0, action: 'inspect' }));
  page.flush();
  const state = page.events().find(event => event.type === 'dom_state');
  assert.deepEqual(
    { tokenWall: state.tokenWall, tokenInput: state.tokenInput, pairingNotice: state.pairingNotice, pairDesktop: state.pairDesktop },
    { tokenWall: false, tokenInput: false, pairingNotice: false, pairDesktop: false },
  );
  assert.equal(classifyPairingSurface(state), 'pairing_surface_unavailable');

  page.deliver(buildCommand({ run: hello.run, id: 1, action: 'ask_pair' }));
  page.flush();
  assert.equal(page.events().at(-1).ok, false);
  assert.deepEqual(page.invokeLog.calls, []);
});

test('reveal_projects uses the already open real route without toggling it closed', () => {
  const page = createMockPage({ script });
  page.el('[data-panel-nav-toggle]');
  page.el('[data-panel-nav="/projects"]');
  page.open();
  const hello = page.events().find(event => event.type === 'hello');
  page.deliver(buildCommand({ run: hello.run, id: 0, action: 'reveal_projects' }));
  page.flush();
  assert.deepEqual(page.log.clicks, ['[data-panel-nav="/projects"]']);
  assert.equal(page.events().at(-1).ok, true);
  assert.deepEqual(page.invokeLog.calls, []);
});

// A wrong selector or missing click must fail, even if the monitor acknowledges a command.
for (const [action, selector] of [
  ['select_project_a', '[data-panel-project-name="Native QA A"]'],
  ['select_project_b', '[data-panel-project-name="Native QA B"]'],
  ['select_global', '[data-panel-global]'],
  ['open_project_a', '[data-panel-shell] button[data-panel-plugin="project-a"]'],
  ['open_project_b', '[data-panel-shell] button[data-panel-plugin="project-b"]'],
]) {
  test(`${action} clicks only the real bounded DOM control and never invokes IPC`, () => {
    const page = createMockPage({ script });
    page.el(selector);
    page.open();
    const hello = page.events().find(event => event.type === 'hello');
    page.deliver(buildCommand({ run: hello.run, id: 0, action }));
    page.flush();
    assert.deepEqual(page.log.clicks, [selector]);
    assert.equal(page.events().at(-1).ok, true);
    assert.deepEqual(page.invokeLog.calls, []);
  });
  test(`${action} refuses an absent control`, () => {
    const page = createMockPage({ script });
    page.open();
    const hello = page.events().find(event => event.type === 'hello');
    page.deliver(buildCommand({ run: hello.run, id: 0, action }));
    page.flush();
    assert.equal(page.events().at(-1).ok, false);
    assert.deepEqual(page.log.clicks, []);
  });
}
